//! Loss-aware projection of MCP tool results into A3S Code tool output.

use crate::llm::Attachment;
use crate::mcp::protocol::{CallToolResult, ResourceContent, ToolContent};
use crate::tools::{ToolContext, ToolOutput};
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

const MAX_CONTENT_ITEMS: usize = 64;
const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;
const MAX_TOTAL_ARTIFACT_BYTES: usize = 32 * 1024 * 1024;
const MAX_STRUCTURED_CONTENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_PROTOCOL_META_BYTES: usize = 1024 * 1024;
const MAX_CACHED_ARTIFACTS_PER_TOOL: usize = 64;
const MAX_CACHED_ARTIFACT_BYTES_PER_TOOL: u64 = 128 * 1024 * 1024;

/// Convert an MCP result to model-visible text without discarding structured
/// content, decoded images, embedded resources, or protocol metadata.
pub(crate) async fn project_tool_result(
    tool_name: &str,
    result: &CallToolResult,
    context: &ToolContext,
) -> Result<ToolOutput> {
    if result.content.len() > MAX_CONTENT_ITEMS {
        return Err(anyhow!(
            "MCP tool returned {} content items; the limit is {}",
            result.content.len(),
            MAX_CONTENT_ITEMS
        ));
    }
    validate_json_size(
        "structuredContent",
        result.structured_content.as_ref(),
        MAX_STRUCTURED_CONTENT_BYTES,
    )?;
    validate_json_size("_meta", result.meta.as_ref(), MAX_PROTOCOL_META_BYTES)?;

    let mut text_parts = Vec::new();
    let mut images = Vec::new();
    let mut artifacts = Vec::new();
    let mut content = Vec::with_capacity(result.content.len());
    let mut decoded_bytes = 0usize;

    for item in &result.content {
        match item {
            ToolContent::Text { text } => {
                text_parts.push(text.clone());
                content.push(json!({
                    "type": "text",
                    "bytes": text.len(),
                }));
            }
            ToolContent::Image { data, mime_type } => {
                let bytes = decode_bounded(data, &mut decoded_bytes)?;
                let artifact =
                    materialize_artifact(tool_name, None, mime_type, &bytes, context).await?;
                text_parts.push(format!(
                    "[Image: {mime_type}, {} bytes, artifact: {}]",
                    bytes.len(),
                    artifact.path.display()
                ));
                if model_image_mime_type(mime_type) {
                    images.push(Attachment::new(bytes.clone(), mime_type.clone()));
                }
                content.push(json!({
                    "type": "image",
                    "mimeType": mime_type,
                    "bytes": bytes.len(),
                    "sha256": artifact.sha256.clone(),
                    "path": artifact.path.clone(),
                    "attachedToModel": model_image_mime_type(mime_type),
                }));
                artifacts.push(artifact.value("image", None));
            }
            ToolContent::Resource { resource } => {
                let mut projection = ResourceProjection {
                    text_parts: &mut text_parts,
                    images: &mut images,
                    artifacts: &mut artifacts,
                    content: &mut content,
                    decoded_bytes: &mut decoded_bytes,
                };
                project_resource(tool_name, resource, context, &mut projection).await?;
            }
        }
    }

    if text_parts.is_empty() {
        if let Some(structured) = &result.structured_content {
            text_parts.push(
                serde_json::to_string_pretty(structured)
                    .context("failed to format MCP structured content")?,
            );
        }
    }

    let mut metadata = serde_json::Map::new();
    if let Some(structured) = &result.structured_content {
        metadata.insert("structuredContent".to_string(), structured.clone());
    }
    if let Some(meta) = &result.meta {
        metadata.insert("_meta".to_string(), meta.clone());
    }
    if !content.is_empty() {
        metadata.insert("content".to_string(), Value::Array(content));
    }
    if !artifacts.is_empty() {
        metadata.insert("artifacts".to_string(), Value::Array(artifacts));
    }
    metadata.insert("isError".to_string(), Value::Bool(result.is_error));

    let text = text_parts.join("\n");
    let output = if result.is_error {
        ToolOutput::error(text)
    } else {
        ToolOutput::success(text)
    }
    .with_metadata(json!({ "mcp": Value::Object(metadata) }))
    .with_images(images);
    Ok(output)
}

struct ResourceProjection<'a> {
    text_parts: &'a mut Vec<String>,
    images: &'a mut Vec<Attachment>,
    artifacts: &'a mut Vec<Value>,
    content: &'a mut Vec<Value>,
    decoded_bytes: &'a mut usize,
}

async fn project_resource(
    tool_name: &str,
    resource: &ResourceContent,
    context: &ToolContext,
    projection: &mut ResourceProjection<'_>,
) -> Result<()> {
    let mut descriptor = json!({
        "type": "resource",
        "uri": resource.uri,
        "mimeType": resource.mime_type,
        "hasText": resource.text.is_some(),
        "hasBlob": resource.blob.is_some(),
    });

    if let Some(text) = &resource.text {
        projection.text_parts.push(text.clone());
        descriptor["textBytes"] = json!(text.len());
    }
    if let Some(blob) = &resource.blob {
        let bytes = decode_bounded(blob, projection.decoded_bytes)?;
        let mime_type = resource
            .mime_type
            .as_deref()
            .unwrap_or("application/octet-stream");
        let artifact =
            materialize_artifact(tool_name, Some(&resource.uri), mime_type, &bytes, context)
                .await?;
        projection.text_parts.push(format!(
            "[Resource: {}, {mime_type}, {} bytes, artifact: {}]",
            resource.uri,
            bytes.len(),
            artifact.path.display()
        ));
        if model_image_mime_type(mime_type) {
            projection
                .images
                .push(Attachment::new(bytes.clone(), mime_type.to_string()));
        }
        descriptor["blobBytes"] = json!(bytes.len());
        descriptor["sha256"] = json!(artifact.sha256.clone());
        descriptor["path"] = json!(artifact.path.clone());
        descriptor["attachedToModel"] = json!(model_image_mime_type(mime_type));
        projection
            .artifacts
            .push(artifact.value("resource", Some(&resource.uri)));
    } else if resource.text.is_none() {
        projection
            .text_parts
            .push(format!("[Resource: {}]", resource.uri));
    }
    projection.content.push(descriptor);
    Ok(())
}

fn decode_bounded(data: &str, decoded_total: &mut usize) -> Result<Vec<u8>> {
    let max_encoded = MAX_ARTIFACT_BYTES.div_ceil(3) * 4 + 4;
    if data.len() > max_encoded {
        return Err(anyhow!(
            "MCP base64 artifact exceeds the {} MiB per-item limit",
            MAX_ARTIFACT_BYTES / (1024 * 1024)
        ));
    }
    let bytes = BASE64_STANDARD
        .decode(data)
        .context("MCP tool returned invalid base64 artifact data")?;
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err(anyhow!(
            "MCP decoded artifact exceeds the {} MiB per-item limit",
            MAX_ARTIFACT_BYTES / (1024 * 1024)
        ));
    }
    *decoded_total = decoded_total
        .checked_add(bytes.len())
        .ok_or_else(|| anyhow!("MCP artifact byte count overflowed"))?;
    if *decoded_total > MAX_TOTAL_ARTIFACT_BYTES {
        return Err(anyhow!(
            "MCP decoded artifacts exceed the {} MiB per-call limit",
            MAX_TOTAL_ARTIFACT_BYTES / (1024 * 1024)
        ));
    }
    Ok(bytes)
}

fn validate_json_size(label: &str, value: Option<&Value>, limit: usize) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    let bytes = serde_json::to_vec(value)
        .with_context(|| format!("failed to encode MCP {label} for bounded projection"))?;
    if bytes.len() > limit {
        return Err(anyhow!(
            "MCP {label} exceeds the {} MiB projection limit",
            limit / (1024 * 1024)
        ));
    }
    Ok(())
}

struct MaterializedArtifact {
    path: PathBuf,
    media_type: String,
    size: usize,
    sha256: String,
}

impl MaterializedArtifact {
    fn value(&self, kind: &str, source_uri: Option<&str>) -> Value {
        json!({
            "kind": kind,
            "path": self.path,
            "mediaType": self.media_type,
            "size": self.size,
            "sha256": self.sha256,
            "sourceUri": source_uri,
        })
    }
}

async fn materialize_artifact(
    tool_name: &str,
    _source_uri: Option<&str>,
    media_type: &str,
    bytes: &[u8],
    context: &ToolContext,
) -> Result<MaterializedArtifact> {
    let digest = sha256::digest(bytes);
    let session = context.session_id.as_deref().unwrap_or("anonymous");
    let root = artifact_root(context)
        .join(sanitize_segment(session))
        .join(sanitize_segment(tool_name));
    tokio::fs::create_dir_all(&root).await.with_context(|| {
        format!(
            "failed to create MCP artifact directory '{}'",
            root.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .await
            .with_context(|| {
                format!(
                    "failed to secure MCP artifact directory '{}'",
                    root.display()
                )
            })?;
    }
    let path = root.join(format!("{digest}.{}", extension_for_media_type(media_type)));
    match tokio::fs::symlink_metadata(&path).await {
        Ok(_) => verify_existing_artifact(&path, bytes, &digest).await?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_private_file(&path, bytes, &digest).await?;
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect MCP artifact '{}'", path.display()));
        }
    }
    prune_artifact_directory(&root, &path).await;
    Ok(MaterializedArtifact {
        path,
        media_type: media_type.to_string(),
        size: bytes.len(),
        sha256: digest,
    })
}

async fn prune_artifact_directory(directory: &Path, current: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return;
    };
    let mut files = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(metadata) = entry.metadata().await else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata
            .modified()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        files.push((entry.path(), metadata.len(), modified));
    }
    files.sort_by_key(|(_, _, modified)| *modified);
    let mut count = files.len();
    let mut total = files
        .iter()
        .fold(0_u64, |sum, (_, size, _)| sum.saturating_add(*size));
    for (path, size, _) in files {
        if count <= MAX_CACHED_ARTIFACTS_PER_TOOL && total <= MAX_CACHED_ARTIFACT_BYTES_PER_TOOL {
            break;
        }
        if path == current {
            continue;
        }
        if tokio::fs::remove_file(&path).await.is_ok() {
            count = count.saturating_sub(1);
            total = total.saturating_sub(size);
        }
    }
}

fn artifact_root(context: &ToolContext) -> PathBuf {
    #[cfg(test)]
    {
        context.workspace.join(".a3s-test-mcp-artifacts")
    }
    #[cfg(not(test))]
    {
        let _ = context;
        dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("a3s-code")
            .join("mcp-artifacts")
    }
}

async fn verify_existing_artifact(path: &Path, bytes: &[u8], digest: &str) -> Result<()> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .with_context(|| format!("failed to inspect MCP artifact '{}'", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(anyhow!(
            "existing MCP artifact '{}' is not a regular file",
            path.display()
        ));
    }
    let existing = tokio::fs::read(path)
        .await
        .with_context(|| format!("failed to verify MCP artifact '{}'", path.display()))?;
    if existing.len() != bytes.len() || sha256::digest(&existing) != digest {
        return Err(anyhow!(
            "existing MCP artifact '{}' does not match its content digest",
            path.display()
        ));
    }
    Ok(())
}

async fn write_private_file(path: &Path, bytes: &[u8], digest: &str) -> Result<()> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    match options.open(path).await {
        Ok(mut file) => {
            if let Err(error) = file.write_all(bytes).await {
                drop(file);
                let _ = tokio::fs::remove_file(path).await;
                return Err(error)
                    .with_context(|| format!("failed to write MCP artifact '{}'", path.display()));
            }
            if let Err(error) = file.flush().await {
                drop(file);
                let _ = tokio::fs::remove_file(path).await;
                return Err(error)
                    .with_context(|| format!("failed to flush MCP artifact '{}'", path.display()));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                    .await
                    .with_context(|| {
                        format!("failed to secure MCP artifact '{}'", path.display())
                    })?;
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            verify_existing_artifact(path, bytes, digest).await
        }
        Err(error) => Err(error)
            .with_context(|| format!("failed to create MCP artifact '{}'", path.display())),
    }
}

fn sanitize_segment(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(96)
        .collect::<String>();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn extension_for_media_type(media_type: &str) -> &'static str {
    match media_type.split(';').next().unwrap_or(media_type).trim() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "application/pdf" => "pdf",
        "application/json" => "json",
        "text/plain" => "txt",
        "text/markdown" => "md",
        _ => "bin",
    }
}

fn model_image_mime_type(media_type: &str) -> bool {
    matches!(
        media_type.split(';').next().unwrap_or(media_type).trim(),
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    )
}

/// Convert MCP tool result to a compact text representation.
///
/// Runtime tool execution uses [`project_tool_result`] so image bytes and
/// structured data are retained. This helper remains for diagnostics and
/// compatibility callers that explicitly need text only.
pub fn tool_result_to_string(result: &CallToolResult) -> String {
    let mut output = Vec::new();
    for content in &result.content {
        match content {
            ToolContent::Text { text } => output.push(text.clone()),
            ToolContent::Image { mime_type, .. } => {
                output.push(format!("[Image: {mime_type}]"));
            }
            ToolContent::Resource { resource } => {
                if let Some(text) = &resource.text {
                    output.push(text.clone());
                }
                if resource.blob.is_some() || resource.text.is_none() {
                    output.push(format!("[Resource: {}]", resource.uri));
                }
            }
        }
    }
    if output.is_empty() {
        if let Some(structured) = &result.structured_content {
            return serde_json::to_string_pretty(structured)
                .unwrap_or_else(|_| structured.to_string());
        }
    }
    output.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::protocol::CallToolResult;

    #[tokio::test]
    async fn projects_structured_content_and_materializes_an_image() {
        let temp = tempfile::tempdir().unwrap();
        let context =
            ToolContext::new(temp.path().to_path_buf()).with_session_id("mcp-result-test");
        let png = b"\x89PNG\r\n\x1a\nfixture";
        let result = CallToolResult {
            content: vec![ToolContent::Image {
                data: BASE64_STANDARD.encode(png),
                mime_type: "image/png".to_string(),
            }],
            structured_content: Some(json!({"text": "A3S"})),
            is_error: false,
            meta: Some(json!({"requestId": "one"})),
        };

        let output = project_tool_result("mcp__use_ocr__ocr_extract", &result, &context)
            .await
            .unwrap();
        assert!(output.success);
        assert_eq!(output.images.len(), 1);
        assert_eq!(output.images[0].data, png);
        assert_eq!(
            output.metadata.as_ref().unwrap()["mcp"]["structuredContent"]["text"],
            "A3S"
        );
        assert_eq!(
            output.metadata.as_ref().unwrap()["mcp"]["_meta"]["requestId"],
            "one"
        );
        let path = output.metadata.as_ref().unwrap()["mcp"]["artifacts"][0]["path"]
            .as_str()
            .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), png);
    }

    #[tokio::test]
    async fn rejects_oversized_or_invalid_base64_without_losing_error_status() {
        let temp = tempfile::tempdir().unwrap();
        let context = ToolContext::new(temp.path().to_path_buf());
        let invalid = CallToolResult {
            content: vec![ToolContent::Image {
                data: "***".to_string(),
                mime_type: "image/png".to_string(),
            }],
            ..CallToolResult::default()
        };
        assert!(project_tool_result("mcp__test__image", &invalid, &context)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn rejects_a_preexisting_artifact_that_does_not_match_its_digest() {
        let temp = tempfile::tempdir().unwrap();
        let context = ToolContext::new(temp.path().to_path_buf());
        let png = b"\x89PNG\r\n\x1a\nexpected";
        let digest = sha256::digest(png);
        let root = artifact_root(&context)
            .join("anonymous")
            .join("mcp__test__image");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(root.join(format!("{digest}.png")), b"different")
            .await
            .unwrap();
        let result = CallToolResult {
            content: vec![ToolContent::Image {
                data: BASE64_STANDARD.encode(png),
                mime_type: "image/png".to_string(),
            }],
            ..CallToolResult::default()
        };

        let error = project_tool_result("mcp__test__image", &result, &context)
            .await
            .expect_err("content-addressed artifacts must reject a conflicting file");
        assert!(
            error
                .to_string()
                .contains("does not match its content digest"),
            "{error:#}"
        );
    }
}
