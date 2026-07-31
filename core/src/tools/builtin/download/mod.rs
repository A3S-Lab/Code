//! Binary-safe local workspace downloads.

mod path;
mod transfer;

use self::path::{infer_filename, prepare_destination, Destination};
use self::transfer::{
    connection_count, download_parallel, download_sequential, probe_server, DownloadFailure,
    FailureKind, ParallelDownloadOptions, ProbeMode, MAX_CONNECTIONS,
};
use super::safe_http::{explicit_web_proxy_from_env, parse_http_url, system_web_proxy};
use crate::tools::types::{
    Tool, ToolCapabilities, ToolContext, ToolErrorKind, ToolOutput, ToolOutputKind,
};
use crate::workspace::WorkspacePath;
use anyhow::Result;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

const DEFAULT_MAX_BYTES: u64 = 512 * 1024 * 1024;
const ABSOLUTE_MAX_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const DEFAULT_TIMEOUT_SECONDS: u64 = 300;
const MAX_TIMEOUT_SECONDS: u64 = 3_600;

pub struct DownloadTool;

#[async_trait]
impl Tool for DownloadTool {
    fn name(&self) -> &str {
        "download"
    }

    fn description(&self) -> &str {
        "Download an HTTP(S) resource into the local workspace. Streams binary data to an adjacent temporary file, verifies Range responses and optional SHA-256, then atomically promotes the completed file. Supports adaptive 1-4 connection downloads, bounded retries, cancellation, and sequential fallback."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Required. Public http:// or https:// URL. Signed query parameters are preserved for the request but never returned in metadata."
                },
                "file_path": {
                    "type": "string",
                    "description": "Optional workspace-relative destination. When omitted, a safe filename is inferred from Content-Disposition, URL query/path, or download.bin."
                },
                "overwrite": {
                    "type": "boolean",
                    "default": false,
                    "description": "Optional. Replace an existing regular file only after the new download is complete and verified. Default: false."
                },
                "connections": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_CONNECTIONS,
                    "description": "Optional. Requested parallel Range connections (1-4). Omit for adaptive selection based on file size."
                },
                "max_bytes": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": ABSOLUTE_MAX_BYTES,
                    "default": DEFAULT_MAX_BYTES,
                    "description": "Optional hard limit for declared and streamed bytes. Default: 536870912 (512 MiB); maximum: 8589934592 (8 GiB)."
                },
                "timeout": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_TIMEOUT_SECONDS,
                    "default": DEFAULT_TIMEOUT_SECONDS,
                    "description": "Optional total timeout in seconds, including retries, verification, and atomic promotion. Default: 300; maximum: 3600."
                },
                "expected_sha256": {
                    "type": "string",
                    "pattern": "^[A-Fa-f0-9]{64}$",
                    "description": "Optional expected lowercase or uppercase SHA-256 hex digest. The final file is not promoted when verification fails."
                }
            },
            "required": ["url"],
            "examples": [
                {"url": "https://example.com/archive.zip"},
                {
                    "url": "https://example.com/model.bin?signature=...",
                    "file_path": "artifacts/model.bin",
                    "connections": 4,
                    "expected_sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                }
            ]
        })
    }

    fn capabilities(&self, _args: &serde_json::Value) -> ToolCapabilities {
        let mut capabilities = ToolCapabilities::conservative();
        capabilities.cancellation_safe = true;
        capabilities.output_kind = ToolOutputKind::Mixed;
        capabilities
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let parsed = match DownloadArgs::parse(args, ctx) {
            Ok(parsed) => parsed,
            Err(message) => return Ok(invalid_argument(&message)),
        };
        let Some(local_root) = ctx.workspace_services.local_root().map(Path::to_path_buf) else {
            return Ok(typed_error(
                "download is available only for local workspace backends",
                ToolErrorKind::Unsupported {
                    message: "Binary download sinks are not available for this workspace backend"
                        .to_string(),
                },
            ));
        };

        let mut proxy_url = ctx
            .search_config
            .as_ref()
            .and_then(|config| config.headless.as_ref())
            .and_then(|config| config.proxy_url.clone())
            .or_else(explicit_web_proxy_from_env);
        if proxy_url.is_none() {
            proxy_url = system_web_proxy().await;
        }

        let timeout = parsed.timeout;
        let parent_cancellation = ctx.cancellation_token();
        let operation_cancellation = parent_cancellation.child_token();
        let operation = run_download(
            parsed,
            ctx.clone(),
            local_root,
            proxy_url,
            operation_cancellation.clone(),
        );
        tokio::pin!(operation);
        tokio::select! {
            biased;
            result = &mut operation => match result {
                Ok(output) => Ok(output),
                Err(error) => Ok(failure_output(error)),
            },
            _ = parent_cancellation.cancelled() => {
                operation_cancellation.cancel();
                match tokio::time::timeout(Duration::from_secs(5), &mut operation).await {
                    Ok(Ok(output)) => Ok(output),
                    Ok(Err(error)) => Ok(failure_output(error)),
                    Err(_) => Ok(cancelled_output()),
                }
            },
            _ = tokio::time::sleep(timeout) => {
                operation_cancellation.cancel();
                match tokio::time::timeout(Duration::from_secs(5), &mut operation).await {
                    Ok(Ok(output)) => Ok(output),
                    Ok(Err(error)) if error.kind != FailureKind::Cancelled => {
                        Ok(failure_output(error))
                    }
                    _ => Ok(timeout_output(timeout)),
                }
            },
        }
    }
}

struct DownloadArgs {
    url: reqwest::Url,
    explicit_path: Option<WorkspacePath>,
    overwrite: bool,
    connections: Option<usize>,
    max_bytes: u64,
    timeout: Duration,
    expected_sha256: Option<String>,
}

impl DownloadArgs {
    fn parse(args: &serde_json::Value, ctx: &ToolContext) -> std::result::Result<Self, String> {
        let Some(raw_url) = args.get("url").and_then(serde_json::Value::as_str) else {
            return Err("url parameter is required".to_string());
        };
        let mut url = parse_http_url(raw_url)?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err("URL user information is not supported".to_string());
        }
        url.set_fragment(None);

        let explicit_path = match args.get("file_path") {
            None => None,
            Some(value) => {
                let Some(value) = value.as_str().filter(|value| !value.trim().is_empty()) else {
                    return Err("file_path must be a non-empty string".to_string());
                };
                Some(
                    ctx.resolve_workspace_path(value)
                        .map_err(|error| format!("Invalid file_path: {error}"))?,
                )
            }
        };
        let overwrite = optional_bool(args, "overwrite", false)?;
        let connections = match args.get("connections") {
            None => None,
            Some(value) => match value.as_u64().and_then(|value| usize::try_from(value).ok()) {
                Some(value @ 1..=MAX_CONNECTIONS) => Some(value),
                _ => return Err("connections must be an integer from 1 to 4".to_string()),
            },
        };
        let max_bytes = optional_u64(args, "max_bytes", DEFAULT_MAX_BYTES)?;
        if !(1..=ABSOLUTE_MAX_BYTES).contains(&max_bytes) {
            return Err("max_bytes must be between 1 and 8589934592".to_string());
        }
        let timeout_seconds = optional_u64(args, "timeout", DEFAULT_TIMEOUT_SECONDS)?;
        if !(1..=MAX_TIMEOUT_SECONDS).contains(&timeout_seconds) {
            return Err("timeout must be between 1 and 3600 seconds".to_string());
        }
        let expected_sha256 = match args.get("expected_sha256") {
            None => None,
            Some(value) => {
                let Some(value) = value.as_str() else {
                    return Err("expected_sha256 must be a string".to_string());
                };
                if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(
                        "expected_sha256 must contain exactly 64 hexadecimal characters"
                            .to_string(),
                    );
                }
                Some(value.to_ascii_lowercase())
            }
        };

        Ok(Self {
            url,
            explicit_path,
            overwrite,
            connections,
            max_bytes,
            timeout: Duration::from_secs(timeout_seconds),
            expected_sha256,
        })
    }
}

async fn run_download(
    args: DownloadArgs,
    ctx: ToolContext,
    local_root: PathBuf,
    proxy_url: Option<String>,
    cancellation: CancellationToken,
) -> Result<ToolOutput, DownloadFailure> {
    if cancellation.is_cancelled() {
        return Err(DownloadFailure::cancelled());
    }

    let mut destination = match args.explicit_path.clone() {
        Some(path) => Some(
            prepare_destination_async(local_root.clone(), path, args.overwrite)
                .await
                .map_err(path_failure)?,
        ),
        None => None,
    };
    let probe = probe_server(
        args.url.clone(),
        proxy_url.as_deref(),
        args.max_bytes,
        &cancellation,
    )
    .await?;

    if destination.is_none() {
        let filename = infer_filename(probe.content_disposition.as_deref(), &probe.final_url);
        let workspace_path = ctx
            .resolve_workspace_path(&filename)
            .map_err(|error| path_failure(format!("Invalid inferred filename: {error}")))?;
        destination = Some(
            prepare_destination_async(local_root, workspace_path, args.overwrite)
                .await
                .map_err(path_failure)?,
        );
    }
    let destination = destination.ok_or_else(|| DownloadFailure {
        kind: FailureKind::InvalidArgument,
        message: "Download destination could not be determined".to_string(),
    })?;
    let (file, temp_path) = create_temp_file(&destination.parent).await?;

    let range_supported = probe.range_supported;
    let final_url = probe.final_url.clone();
    let total_size = probe.total_size;
    let content_type = probe.content_type.clone();
    let (file, bytes, strategy, connections) = match probe.mode {
        ProbeMode::Empty => (file, 0, "sequential", 1),
        ProbeMode::Sequential(response) => {
            let (file, bytes) = download_sequential(
                file,
                Some(response),
                final_url.clone(),
                proxy_url.as_deref(),
                args.max_bytes,
                total_size,
                &cancellation,
            )
            .await?;
            (file, bytes, "sequential", 1)
        }
        ProbeMode::Parallel => {
            let total_size = total_size.ok_or_else(|| DownloadFailure {
                kind: FailureKind::Protocol,
                message: "Range probe did not provide a resource size".to_string(),
            })?;
            // Without a validator, independently fetched ranges could belong
            // to different resource revisions. Prefer one coherent response.
            let connections = if probe.validator.is_some() {
                connection_count(total_size, args.connections)
            } else {
                1
            };
            if connections == 1 {
                let (file, bytes) = download_sequential(
                    file,
                    None,
                    final_url.clone(),
                    proxy_url.as_deref(),
                    args.max_bytes,
                    Some(total_size),
                    &cancellation,
                )
                .await?;
                (file, bytes, "sequential", 1)
            } else {
                match download_parallel(
                    file,
                    ParallelDownloadOptions {
                        url: final_url.clone(),
                        proxy_url: proxy_url.clone(),
                        total_size,
                        requested_connections: connections,
                        validator: probe.validator,
                        max_bytes: args.max_bytes,
                    },
                    &cancellation,
                )
                .await
                {
                    Ok((file, used_connections)) => {
                        (file, total_size, "parallel_range", used_connections)
                    }
                    Err(error)
                        if matches!(error.kind, FailureKind::Network | FailureKind::Protocol) =>
                    {
                        let reopened = tokio::fs::OpenOptions::new()
                            .read(true)
                            .write(true)
                            .open(temp_path.as_ref() as &Path)
                            .await
                            .map_err(|open_error| {
                                DownloadFailure {
                                    kind: FailureKind::Io,
                                    message: format!(
                                        "Range download failed ({}) and the temporary file could not be reopened: {open_error}",
                                        error.message
                                    ),
                                }
                            })?;
                        let (file, bytes) = download_sequential(
                            reopened,
                            None,
                            final_url.clone(),
                            proxy_url.as_deref(),
                            args.max_bytes,
                            None,
                            &cancellation,
                        )
                        .await?;
                        (file, bytes, "sequential_fallback", 1)
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    };

    file.sync_all().await.map_err(|error| DownloadFailure {
        kind: FailureKind::Io,
        message: format!("Failed to sync temporary download: {error}"),
    })?;
    drop(file);

    let sha256 = match args.expected_sha256.as_deref() {
        Some(expected) => {
            let actual = sha256_file(temp_path.as_ref(), &cancellation).await?;
            if actual != expected {
                return Err(DownloadFailure {
                    kind: FailureKind::Protocol,
                    message: format!(
                        "SHA-256 mismatch: expected {expected}, downloaded {actual}; destination was not changed"
                    ),
                });
            }
            Some(actual)
        }
        None => None,
    };

    if cancellation.is_cancelled() {
        return Err(DownloadFailure::cancelled());
    }

    persist_temp_file(temp_path, &destination, args.overwrite).await?;
    sync_parent_directory(&destination.parent).await?;

    let mut source_anchors = Vec::new();
    if let Some(initial_anchor) = super::safe_http_source_url(args.url.as_str()) {
        source_anchors.push(initial_anchor);
    }
    if let Some(final_anchor) = super::safe_http_source_url(final_url.as_str()) {
        if source_anchors.first() != Some(&final_anchor) {
            source_anchors.push(final_anchor);
        }
    }
    let mut metadata = serde_json::json!({
        "file_path": destination.workspace_path.as_str(),
        "bytes": bytes,
        "content_type": content_type,
        "strategy": strategy,
        "connections": connections,
        "range_supported": range_supported,
        "overwritten": destination.existed,
        "source_anchors": source_anchors,
    });
    if let Some(sha256) = sha256 {
        metadata["sha256"] = serde_json::Value::String(sha256);
    }
    Ok(ToolOutput::success(format!(
        "Downloaded {bytes} bytes to {} using {strategy}",
        ctx.workspace_services
            .display_path(&destination.workspace_path)
    ))
    .with_metadata(metadata))
}

async fn prepare_destination_async(
    root: PathBuf,
    path: WorkspacePath,
    overwrite: bool,
) -> Result<Destination, String> {
    tokio::task::spawn_blocking(move || prepare_destination(&root, path, overwrite))
        .await
        .map_err(|error| format!("Download path worker failed: {error}"))?
}

async fn create_temp_file(
    parent: &Path,
) -> Result<(tokio::fs::File, tempfile::TempPath), DownloadFailure> {
    let parent = parent.to_path_buf();
    let named = tokio::task::spawn_blocking(move || {
        tempfile::Builder::new()
            .prefix(".a3s-download-")
            .suffix(".part")
            .tempfile_in(parent)
    })
    .await
    .map_err(|error| DownloadFailure {
        kind: FailureKind::Io,
        message: format!("Temporary file worker failed: {error}"),
    })?
    .map_err(|error| DownloadFailure {
        kind: FailureKind::Io,
        message: format!("Failed to create adjacent temporary file: {error}"),
    })?;
    let (file, path) = named.into_parts();
    Ok((tokio::fs::File::from_std(file), path))
}

async fn sha256_file(
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<String, DownloadFailure> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| DownloadFailure {
            kind: FailureKind::Io,
            message: format!("Failed to open temporary download for SHA-256: {error}"),
        })?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 128 * 1024];
    loop {
        let read = tokio::select! {
            _ = cancellation.cancelled() => return Err(DownloadFailure::cancelled()),
            result = file.read(&mut buffer) => result,
        }
        .map_err(|error| DownloadFailure {
            kind: FailureKind::Io,
            message: format!("Failed to hash temporary download: {error}"),
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

async fn persist_temp_file(
    temp_path: tempfile::TempPath,
    destination: &Destination,
    overwrite: bool,
) -> Result<(), DownloadFailure> {
    let destination = destination.absolute_path.clone();
    tokio::task::spawn_blocking(move || {
        if overwrite {
            temp_path.persist(&destination)
        } else {
            temp_path.persist_noclobber(&destination)
        }
        .map_err(|error| error.error.to_string())
    })
    .await
    .map_err(|error| DownloadFailure {
        kind: FailureKind::Io,
        message: format!("Atomic promotion worker failed: {error}"),
    })?
    .map_err(|error| DownloadFailure {
        kind: FailureKind::Io,
        message: format!("Failed to promote completed download: {error}"),
    })
}

#[cfg(unix)]
async fn sync_parent_directory(parent: &Path) -> Result<(), DownloadFailure> {
    let parent = parent.to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::File::open(parent)?.sync_all())
        .await
        .map_err(|error| DownloadFailure {
            kind: FailureKind::Io,
            message: format!("Directory sync worker failed: {error}"),
        })?
        .map_err(|error| DownloadFailure {
            kind: FailureKind::Io,
            message: format!("Failed to sync download directory: {error}"),
        })
}

#[cfg(not(unix))]
async fn sync_parent_directory(_parent: &Path) -> Result<(), DownloadFailure> {
    Ok(())
}

fn optional_bool(
    args: &serde_json::Value,
    field: &str,
    default: bool,
) -> std::result::Result<bool, String> {
    match args.get(field) {
        None => Ok(default),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| format!("{field} must be a boolean")),
    }
}

fn optional_u64(
    args: &serde_json::Value,
    field: &str,
    default: u64,
) -> std::result::Result<u64, String> {
    match args.get(field) {
        None => Ok(default),
        Some(value) => value
            .as_u64()
            .ok_or_else(|| format!("{field} must be a positive integer")),
    }
}

fn path_failure(message: impl Into<String>) -> DownloadFailure {
    DownloadFailure {
        kind: FailureKind::InvalidArgument,
        message: message.into(),
    }
}

fn failure_output(error: DownloadFailure) -> ToolOutput {
    match error.kind {
        FailureKind::Cancelled => cancelled_output(),
        FailureKind::RateLimited(retry_after_ms) => {
            typed_error(error.message, ToolErrorKind::RateLimited { retry_after_ms })
        }
        FailureKind::TooLarge | FailureKind::InvalidArgument => invalid_argument(&error.message),
        FailureKind::Protocol => ToolOutput::error(error.message),
        FailureKind::Network | FailureKind::Io => ToolOutput::error(error.message),
    }
}

fn invalid_argument(message: &str) -> ToolOutput {
    typed_error(
        message,
        ToolErrorKind::InvalidArgument {
            message: message.to_string(),
        },
    )
}

fn cancelled_output() -> ToolOutput {
    typed_error(
        "Download cancelled",
        ToolErrorKind::Cancelled {
            op: "download".to_string(),
        },
    )
}

fn timeout_output(timeout: Duration) -> ToolOutput {
    typed_error(
        format!("Download timed out after {} seconds", timeout.as_secs()),
        ToolErrorKind::Timeout {
            op: "download".to_string(),
            duration_ms: timeout.as_millis() as u64,
        },
    )
}

fn typed_error(message: impl Into<String>, kind: ToolErrorKind) -> ToolOutput {
    ToolOutput::error(message).with_error_kind(kind)
}

#[cfg(test)]
mod tests;
