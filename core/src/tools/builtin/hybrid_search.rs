//! Session-scoped hybrid workspace search rendering.

use crate::text::truncate_utf8;
use crate::tools::types::{Tool, ToolCapabilities, ToolContext, ToolOutput};
use crate::workspace::{
    escape_control_chars_for_display, WorkspaceHybridFallbackReason, WorkspaceHybridSearchHit,
    WorkspaceHybridSearchRequest, WorkspaceHybridSearchResult, WorkspaceRetrievalError,
};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashSet;

const DEFAULT_LIMIT: usize = 10;
const MAX_LIMIT: usize = 25;
const MAX_QUERY_BYTES: usize = 2_048;
const MAX_RENDERED_LINES: usize = 16;
const MAX_RENDERED_LINE_BYTES: usize = 500;

pub(super) struct HybridSearchTool;

#[async_trait]
impl Tool for HybridSearchTool {
    fn name(&self) -> &str {
        "hybrid"
    }

    fn description(&self) -> &str {
        "Fuse exact, BM25, symbol, and semantic evidence over current verified workspace source."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "query": { "type": "string", "minLength": 1, "maxLength": MAX_QUERY_BYTES },
                "path": { "type": "string" },
                "include": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIMIT }
            },
            "required": ["query"]
        })
    }

    fn capabilities(&self, _args: &serde_json::Value) -> ToolCapabilities {
        ToolCapabilities::parallel_safe_read(2)
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let query = match args.get("query").and_then(serde_json::Value::as_str) {
            Some(query) if !query.trim().is_empty() => query.trim(),
            _ => return Ok(ToolOutput::error("query parameter is required")),
        };
        if query.len() > MAX_QUERY_BYTES {
            return Ok(ToolOutput::error(format!(
                "query exceeds the {MAX_QUERY_BYTES}-byte limit"
            )));
        }
        let limit = match bounded_limit(args) {
            Ok(limit) => limit,
            Err(error) => return Ok(ToolOutput::error(error)),
        };
        let mut request = WorkspaceHybridSearchRequest::new(query).with_limit(limit);
        if let Some(path) = args
            .get("path")
            .and_then(serde_json::Value::as_str)
            .filter(|path| !path.trim().is_empty())
        {
            request.path = Some(path.to_owned());
        }
        if let Some(include) = args
            .get("include")
            .and_then(serde_json::Value::as_str)
            .filter(|include| !include.trim().is_empty())
        {
            request.include = Some(include.to_owned());
        }

        match ctx
            .workspace_services
            .hybrid_search(request, ctx.cancellation_token())
            .await
        {
            Ok(result) => Ok(render_result(result)),
            Err(WorkspaceRetrievalError::Cancelled) => {
                Ok(ToolOutput::error("hybrid search was cancelled"))
            }
            Err(error) => Ok(ToolOutput::error(format!("hybrid search failed: {error}"))),
        }
    }
}

fn bounded_limit(args: &serde_json::Value) -> std::result::Result<usize, String> {
    let Some(value) = args.get("limit") else {
        return Ok(DEFAULT_LIMIT);
    };
    let Some(value) = value.as_u64().and_then(|value| usize::try_from(value).ok()) else {
        return Err(format!("limit must be an integer from 1 to {MAX_LIMIT}"));
    };
    if !(1..=MAX_LIMIT).contains(&value) {
        return Err(format!("limit must be from 1 to {MAX_LIMIT}"));
    }
    Ok(value)
}

fn render_result(result: WorkspaceHybridSearchResult) -> ToolOutput {
    let metadata = result_metadata(&result);
    if result.hits.is_empty() {
        let mut content = "Hybrid search returned no verified matches.".to_owned();
        if let Some(fallback) = result.fallback {
            content.push('\n');
            content.push_str(fallback_message(fallback));
        }
        return ToolOutput::success(content).with_metadata(metadata);
    }

    let mut content = "Hybrid results\n\n".to_owned();
    content.push_str(
        &result
            .hits
            .iter()
            .enumerate()
            .map(|(rank, hit)| render_hit(rank + 1, hit))
            .collect::<Vec<_>>()
            .join("\n\n"),
    );
    content.push_str(&format!(
        "\n\n{} current-source-verified result(s)",
        result.hits.len()
    ));
    if let Some(fallback) = result.fallback {
        content.push_str("\nWarning: ");
        content.push_str(fallback_message(fallback));
    }
    ToolOutput::success(content).with_metadata(metadata)
}

fn render_hit(rank: usize, hit: &WorkspaceHybridSearchHit) -> String {
    let chunk = &hit.chunk;
    let safe_path = escape_control_chars_for_display(chunk.path.as_ref());
    let rendered_line_count = chunk.text.lines().count().min(MAX_RENDERED_LINES);
    let rendered_end_line = chunk
        .start_line
        .saturating_add(rendered_line_count.saturating_sub(1));
    let evidence = hit
        .channels
        .iter()
        .map(|channel| format!("{:?}#{}", channel.channel, channel.rank).to_lowercase())
        .collect::<Vec<_>>()
        .join(", ");
    let mut content = format!(
        "{rank}. {safe_path}:{}-{} (rrf {:.5}; {evidence})",
        chunk.start_line, rendered_end_line, hit.fused_score
    );
    for (offset, line) in chunk.text.lines().take(MAX_RENDERED_LINES).enumerate() {
        let safe_line = escape_control_chars_for_display(line);
        let rendered = truncate_utf8(&safe_line, MAX_RENDERED_LINE_BYTES);
        content.push_str(&format!(
            "\n{:>6} | {rendered}",
            chunk.start_line.saturating_add(offset)
        ));
        if rendered.len() < safe_line.len() {
            content.push_str("...");
        }
    }
    if chunk.text.lines().count() > MAX_RENDERED_LINES {
        content.push_str("\n       | ...");
    }
    content
}

fn result_metadata(result: &WorkspaceHybridSearchResult) -> serde_json::Value {
    let mut seen_paths = HashSet::new();
    let source_anchors = result
        .hits
        .iter()
        .filter(|hit| seen_paths.insert(hit.chunk.path.to_string()))
        .map(|hit| hit.chunk.path.as_ref())
        .collect::<Vec<_>>();
    let results = result
        .hits
        .iter()
        .map(|hit| {
            serde_json::json!({
                "chunk_id": hit.chunk.id.as_str(),
                "path": hit.chunk.path.as_ref(),
                "start_line": hit.chunk.start_line,
                "end_line": hit.chunk.end_line,
                "source_revision": hit.chunk.source_revision,
                "fused_score": hit.fused_score,
                "exact_identifier": hit.exact_identifier,
                "channels": &hit.channels,
                "digest_verified": true,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "algorithm": "rrf_k60",
        "catalog_revision": result.catalog_revision,
        "source_revision": result.source_revision,
        "semantic_status": &result.semantic_status,
        "channels": &result.channels,
        "fallback": &result.fallback,
        "truncated": result.truncated,
        "source_anchors": source_anchors,
        "results": results,
        "returned_results": results.len(),
    })
}

fn fallback_message(reason: WorkspaceHybridFallbackReason) -> &'static str {
    match reason {
        WorkspaceHybridFallbackReason::Unavailable => {
            "one optional retrieval channel is unavailable."
        }
        WorkspaceHybridFallbackReason::Building => {
            "one retrieval channel is still building; results use partial evidence."
        }
        WorkspaceHybridFallbackReason::Degraded => {
            "one retrieval channel is degraded; results use partial evidence."
        }
        WorkspaceHybridFallbackReason::QueryEmbeddingFailed => {
            "query embedding failed; exact, BM25, and symbol evidence was retained."
        }
        WorkspaceHybridFallbackReason::VectorSearchFailed => {
            "vector search failed; exact, BM25, and symbol evidence was retained."
        }
        WorkspaceHybridFallbackReason::StructuralQueryFailed => {
            "symbol search failed; other channel evidence was retained."
        }
        WorkspaceHybridFallbackReason::RevisionChanged => {
            "the workspace changed during this query; retry for a coherent result."
        }
        WorkspaceHybridFallbackReason::FilteredStaleHits => {
            "stale or unreadable candidates were removed."
        }
    }
}
