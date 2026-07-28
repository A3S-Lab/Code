//! Read tool - Read file contents with line numbering

use crate::text::truncate_utf8;
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::tools::{MAX_LINE_LENGTH, MAX_OUTPUT_SIZE, MAX_READ_LINES};
use crate::workspace::{escape_control_chars_for_display, WorkspaceTextRange};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const MAX_BATCH_READ_FILES: usize = 32;
const MIN_BATCH_READ_OUTPUT_BYTES: usize = 1_024;
const DEFAULT_BATCH_READ_OUTPUT_BYTES: usize = 64 * 1_024;
const MAX_BATCH_READ_OUTPUT_BYTES: usize = MAX_OUTPUT_SIZE - (4 * 1_024);
const MAX_BATCH_READ_PATH_BYTES: usize = 4 * 1_024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BatchReadEntry {
    path: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    offset: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read one text or image file, or pack 1-32 text files into one bounded response. Returns line-numbered text and exact continuation arguments when more content remains."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to one file, absolute or relative to the workspace. Use exactly one of file_path or files."
                },
                "offset": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Optional. Line number to start reading from. 0-indexed. Default: 0."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_READ_LINES,
                    "description": "Optional. Maximum number of lines to read. Default and maximum: 2000."
                },
                "files": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_BATCH_READ_FILES,
                    "description": "Read several independent text ranges in request order under one shared output budget. A failed member is reported in its own segment without discarding successful members.",
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "path": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": MAX_BATCH_READ_PATH_BYTES,
                                "description": "Path to a text file, absolute or relative to the workspace."
                            },
                            "offset": {
                                "type": "integer",
                                "minimum": 0,
                                "description": "Optional 0-based starting line. Default: 0."
                            },
                            "limit": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": MAX_READ_LINES,
                                "description": "Optional maximum lines for this file. Default and maximum: 2000."
                            }
                        },
                        "required": ["path"]
                    }
                },
                "max_output_bytes": {
                    "type": "integer",
                    "minimum": MIN_BATCH_READ_OUTPUT_BYTES,
                    "maximum": MAX_BATCH_READ_OUTPUT_BYTES,
                    "description": "Optional shared byte budget for a files read. Default: 65536; maximum: 98304. The returned continuation is included inside this budget."
                }
            },
            "oneOf": [
                {"required": ["file_path"]},
                {"required": ["files"]}
            ],
            "examples": [
                {
                    "file_path": "src/main.rs"
                },
                {
                    "file_path": "src/main.rs",
                    "offset": 40,
                    "limit": 80
                },
                {
                    "files": [
                        {"path": "src/lib.rs"},
                        {"path": "src/config.rs", "offset": 20, "limit": 60}
                    ]
                }
            ]
        })
    }

    fn capabilities(&self, _args: &serde_json::Value) -> crate::tools::ToolCapabilities {
        crate::tools::ToolCapabilities::read_only_paginated(16)
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        if args.get("files").is_some() {
            if args.get("file_path").is_some() {
                return Ok(ToolOutput::error(
                    "file_path and files are mutually exclusive; provide exactly one",
                ));
            }
            return execute_batch_read(args, ctx).await;
        }

        let file_path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("file_path parameter is required")),
        };

        let offset = match args.get("offset") {
            Some(value) => match value.as_u64().and_then(|value| usize::try_from(value).ok()) {
                Some(value) => value,
                None => return Ok(ToolOutput::error("offset must be a non-negative integer")),
            },
            None => 0,
        };

        let requested_limit = match args.get("limit") {
            Some(value) => match value.as_u64().and_then(|value| usize::try_from(value).ok()) {
                Some(0) | None => return Ok(ToolOutput::error("limit must be a positive integer")),
                Some(value) => value,
            },
            None => MAX_READ_LINES,
        };
        let limit = requested_limit.min(MAX_READ_LINES);

        let workspace_path = match ctx.resolve_workspace_path(file_path) {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to resolve path: {}", e))),
        };
        let range = match read_range(ctx, &workspace_path, offset, limit).await {
            Ok(range) => range,
            Err(e) => {
                return Ok(ToolOutput::error(format!(
                    "Failed to read file {}: {}",
                    ctx.workspace_services.display_path(&workspace_path),
                    e
                )))
            }
        };
        if range.lines.is_empty()
            && range.eof
            && range.total_lines.is_some_and(|total| offset > total)
        {
            let total_lines = range.total_lines.unwrap_or_default();
            return Ok(ToolOutput::error(format!(
                "Offset {} exceeds file length ({} lines)",
                offset, total_lines
            )));
        }
        let mut metadata = serde_json::json!({
            "source_anchors": [workspace_path.as_str()],
            "range": {
                "offset": offset,
                "requested_limit": requested_limit,
                "applied_limit": limit,
                "returned_lines": range.lines.len(),
                "next_offset": range.next_offset,
                "eof": range.eof,
                "total_lines": range.total_lines,
                "limit_clamped": requested_limit != limit,
            }
        });
        if range.lines.is_empty() && range.eof {
            return Ok(ToolOutput::success(format!(
                "(end of file: offset {} equals file length)\n",
                offset
            ))
            .with_metadata(metadata));
        }

        let mut output = String::new();
        for (i, line) in range.lines.iter().enumerate() {
            let line_num = offset + i + 1; // 1-indexed
            let truncated = truncate_utf8(line, MAX_LINE_LENGTH);
            output.push_str(&format!("{:>6}\t{}\n", line_num, truncated));
        }

        if let Some(next_offset) = range.next_offset {
            output.push_str(&format!(
                "\n... (more lines available; continue with offset={next_offset})\n"
            ));
        }
        metadata["range"]["output_bytes"] = serde_json::json!(output.len());
        Ok(ToolOutput::success(output).with_metadata(metadata))
    }
}

async fn execute_batch_read(args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
    for parameter in ["offset", "limit"] {
        if args.get(parameter).is_some() {
            return Ok(ToolOutput::error(format!(
                "{parameter} cannot be combined with files; put it on each files entry"
            )));
        }
    }

    let raw_entries = match args.get("files").and_then(serde_json::Value::as_array) {
        Some(entries) if !entries.is_empty() && entries.len() <= MAX_BATCH_READ_FILES => entries,
        Some(entries) if entries.is_empty() => {
            return Ok(ToolOutput::error("files must contain at least one entry"))
        }
        Some(_) => {
            return Ok(ToolOutput::error(format!(
                "files accepts at most {MAX_BATCH_READ_FILES} entries"
            )))
        }
        None => return Ok(ToolOutput::error("files parameter must be an array")),
    };
    let entries = match raw_entries
        .iter()
        .cloned()
        .map(serde_json::from_value::<BatchReadEntry>)
        .collect::<std::result::Result<Vec<_>, _>>()
    {
        Ok(entries) => entries,
        Err(error) => return Ok(ToolOutput::error(format!("Invalid files entry: {error}"))),
    };
    if let Some(error) = validate_batch_entries(&entries, ctx) {
        return Ok(ToolOutput::error(error));
    }

    let max_output_bytes = match args.get("max_output_bytes") {
        Some(value) => match value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
        {
            Some(value)
                if (MIN_BATCH_READ_OUTPUT_BYTES..=MAX_BATCH_READ_OUTPUT_BYTES).contains(&value) =>
            {
                value
            }
            _ => {
                return Ok(ToolOutput::error(format!(
                    "max_output_bytes must be between {MIN_BATCH_READ_OUTPUT_BYTES} and {MAX_BATCH_READ_OUTPUT_BYTES}"
                )))
            }
        },
        None => DEFAULT_BATCH_READ_OUTPUT_BYTES,
    };

    let total = entries.len();
    let mut progress = entries.iter().cloned().map(Some).collect::<Vec<_>>();
    let mut segments = Vec::new();
    let mut source_anchors = Vec::new();
    let mut successful_files = 0usize;
    let mut failed_files = 0usize;
    let mut returned_lines = 0usize;

    for (index, entry) in entries.iter().enumerate() {
        let workspace_path = match ctx.resolve_workspace_path(&entry.path) {
            Ok(path) => path,
            Err(error) => {
                let display_path = escape_control_chars_for_display(&entry.path);
                let segment = format!("=== {display_path} ===\nFailed to resolve path: {error}");
                if !accept_batch_message(
                    &mut segments,
                    &mut progress,
                    index,
                    segment,
                    total,
                    max_output_bytes,
                ) {
                    if segments.is_empty() {
                        return Ok(batch_budget_too_small(max_output_bytes));
                    }
                    break;
                }
                failed_files += 1;
                continue;
            }
        };
        // Batch headers stay workspace-relative so repeated roots do not consume
        // the shared context budget. Source anchors carry the same canonical path.
        let display_path = escape_control_chars_for_display(workspace_path.as_str());
        let limit = entry.limit.unwrap_or(MAX_READ_LINES);
        let range = match read_range(ctx, &workspace_path, entry.offset, limit).await {
            Ok(range) => range,
            Err(error) => {
                let segment =
                    format!("=== {display_path} ===\nFailed to read file {display_path}: {error}");
                if !accept_batch_message(
                    &mut segments,
                    &mut progress,
                    index,
                    segment,
                    total,
                    max_output_bytes,
                ) {
                    if segments.is_empty() {
                        return Ok(batch_budget_too_small(max_output_bytes));
                    }
                    break;
                }
                failed_files += 1;
                continue;
            }
        };
        if range.lines.is_empty()
            && range.eof
            && range
                .total_lines
                .is_some_and(|total_lines| entry.offset > total_lines)
        {
            let total_lines = range.total_lines.unwrap_or_default();
            let segment = format!(
                "=== {display_path} ===\nOffset {} exceeds file length ({total_lines} lines)",
                entry.offset
            );
            if !accept_batch_message(
                &mut segments,
                &mut progress,
                index,
                segment,
                total,
                max_output_bytes,
            ) {
                if segments.is_empty() {
                    return Ok(batch_budget_too_small(max_output_bytes));
                }
                break;
            }
            failed_files += 1;
            continue;
        }

        if range.lines.is_empty() && range.eof {
            let segment = format!(
                "=== {display_path} ===\n(end of file: offset {} equals file length)",
                entry.offset
            );
            if !accept_batch_message(
                &mut segments,
                &mut progress,
                index,
                segment,
                total,
                max_output_bytes,
            ) {
                if segments.is_empty() {
                    return Ok(batch_budget_too_small(max_output_bytes));
                }
                break;
            }
            successful_files += 1;
            source_anchors.push(workspace_path.as_str().to_string());
            continue;
        }

        let lines = range
            .lines
            .iter()
            .enumerate()
            .map(|(line_index, line)| {
                format!(
                    "{:>6}\t{}",
                    entry.offset + line_index + 1,
                    truncate_utf8(line, MAX_LINE_LENGTH)
                )
            })
            .collect::<Vec<_>>();
        let shown = largest_batch_prefix(
            &segments,
            &progress,
            index,
            entry,
            &range,
            &display_path,
            &lines,
            total,
            max_output_bytes,
        );
        if shown == 0 {
            if segments.is_empty() {
                return Ok(batch_budget_too_small(max_output_bytes));
            }
            break;
        }

        progress[index] = batch_progress_after(entry, &range, shown);
        segments.push(batch_content_segment(&display_path, &lines[..shown]));
        returned_lines += shown;
        successful_files += 1;
        source_anchors.push(workspace_path.as_str().to_string());
        if progress[index].is_some() {
            break;
        }
    }

    let content = render_batch_response(&segments, &progress, total);
    debug_assert!(content.len() <= max_output_bytes);
    let continuation = batch_continuation(&progress);
    let truncated = !continuation.is_empty();
    let metadata = serde_json::json!({
        "source_anchors": source_anchors,
        "batch": {
            "status": if truncated {
                "partial"
            } else if failed_files > 0 {
                "partial_failure"
            } else {
                "complete"
            },
            "requested_files": total,
            "successful_files": successful_files,
            "failed_files": failed_files,
            "completed_files": total.saturating_sub(continuation.len()),
            "returned_lines": returned_lines,
            "max_output_bytes": max_output_bytes,
            "output_bytes": content.len(),
            "truncated": truncated,
            "continuation": continuation,
        }
    });

    if successful_files > 0 || truncated {
        Ok(ToolOutput::success(content).with_metadata(metadata))
    } else {
        Ok(ToolOutput::error(content).with_metadata(metadata))
    }
}

fn validate_batch_entries(entries: &[BatchReadEntry], ctx: &ToolContext) -> Option<String> {
    let mut seen = HashSet::new();
    for entry in entries {
        if entry.path.is_empty() || entry.path.len() > MAX_BATCH_READ_PATH_BYTES {
            return Some(format!(
                "files path must contain 1-{MAX_BATCH_READ_PATH_BYTES} bytes"
            ));
        }
        if entry
            .limit
            .is_some_and(|limit| limit == 0 || limit > MAX_READ_LINES)
        {
            return Some(format!(
                "files limit must be between 1 and {MAX_READ_LINES}"
            ));
        }
        let Ok(path) = ctx.resolve_workspace_path(&entry.path) else {
            continue;
        };
        let key = path.as_str().to_string();
        if !seen.insert(key.clone()) {
            let display_path = escape_control_chars_for_display(&key);
            return Some(format!(
                "Duplicate path in files: {display_path}. List each file once."
            ));
        }
    }
    None
}

fn accept_batch_message(
    segments: &mut Vec<String>,
    progress: &mut [Option<BatchReadEntry>],
    index: usize,
    segment: String,
    total: usize,
    budget: usize,
) -> bool {
    let mut proposed = progress.to_vec();
    proposed[index] = None;
    if !batch_candidate_fits(segments, &segment, &proposed, total, budget) {
        return false;
    }
    segments.push(segment);
    progress[index] = None;
    true
}

#[allow(clippy::too_many_arguments)]
fn largest_batch_prefix(
    segments: &[String],
    progress: &[Option<BatchReadEntry>],
    index: usize,
    entry: &BatchReadEntry,
    range: &WorkspaceTextRange,
    display_path: &str,
    lines: &[String],
    total: usize,
    budget: usize,
) -> usize {
    let fits = |shown: usize| {
        let mut proposed = progress.to_vec();
        proposed[index] = batch_progress_after(entry, range, shown);
        let segment = batch_content_segment(display_path, &lines[..shown]);
        batch_candidate_fits(segments, &segment, &proposed, total, budget)
    };

    if fits(lines.len()) {
        return lines.len();
    }
    if lines.len() <= 1 {
        return 0;
    }
    let mut low = 1usize;
    let mut high = lines.len() - 1;
    let mut best = 0usize;
    while low <= high {
        let middle = low + ((high - low) / 2);
        if fits(middle) {
            best = middle;
            low = middle + 1;
        } else if middle == 1 {
            break;
        } else {
            high = middle - 1;
        }
    }
    best
}

fn batch_progress_after(
    entry: &BatchReadEntry,
    range: &WorkspaceTextRange,
    shown: usize,
) -> Option<BatchReadEntry> {
    let offset = entry.offset.saturating_add(shown);
    if shown < range.lines.len() {
        return Some(BatchReadEntry {
            path: entry.path.clone(),
            offset,
            limit: entry.limit.map(|limit| limit.saturating_sub(shown)),
        });
    }

    if entry.limit.is_some() {
        return None;
    }
    range.next_offset.map(|next_offset| BatchReadEntry {
        path: entry.path.clone(),
        offset: next_offset,
        limit: None,
    })
}

fn batch_content_segment(display_path: &str, lines: &[String]) -> String {
    format!("=== {display_path} ===\n{}", lines.join("\n"))
}

fn batch_candidate_fits(
    segments: &[String],
    segment: &str,
    progress: &[Option<BatchReadEntry>],
    total: usize,
    budget: usize,
) -> bool {
    let mut proposed = segments.to_vec();
    proposed.push(segment.to_string());
    render_batch_response(&proposed, progress, total).len() <= budget
}

fn render_batch_response(
    segments: &[String],
    progress: &[Option<BatchReadEntry>],
    total: usize,
) -> String {
    let mut output = segments.join("\n\n");
    let continuation = batch_continuation(progress);
    let footer = if continuation.is_empty() {
        format!("(Complete: all {total} file(s) processed.)")
    } else {
        let encoded = serde_json::to_string(&continuation).unwrap_or_else(|_| "[]".to_string());
        format!(
            "(Partial: {} of {total} file(s) processed. Continue with files={encoded}.)",
            total.saturating_sub(continuation.len())
        )
    };
    if !output.is_empty() {
        output.push_str("\n\n");
    }
    output.push_str(&footer);
    output
}

fn batch_continuation(progress: &[Option<BatchReadEntry>]) -> Vec<BatchReadEntry> {
    progress.iter().flatten().cloned().collect()
}

fn batch_budget_too_small(budget: usize) -> ToolOutput {
    ToolOutput::error(format!(
        "max_output_bytes={budget} is too small to return one line plus a lossless files continuation; increase the budget or shorten the files list"
    ))
}

async fn read_range(
    ctx: &ToolContext,
    path: &crate::workspace::WorkspacePath,
    offset: usize,
    limit: usize,
) -> crate::workspace::WorkspaceResult<WorkspaceTextRange> {
    if let Some(reader) = ctx.workspace_services.text_reader() {
        let path = path.clone();
        return ctx
            .workspace_services
            .run_with_timeout("read_text_range", async move {
                reader.read_text_range(&path, offset, limit).await
            })
            .await;
    }

    let fs = ctx.workspace_services.fs();
    let path = path.clone();
    let content = ctx
        .workspace_services
        .run_with_timeout("read_text", async move { fs.read_text(&path).await })
        .await?;
    let lines = content.lines().collect::<Vec<_>>();
    if offset >= lines.len() {
        return Ok(WorkspaceTextRange {
            lines: Vec::new(),
            next_offset: None,
            eof: true,
            total_lines: Some(lines.len()),
        });
    }
    let end = offset.saturating_add(limit).min(lines.len());
    Ok(WorkspaceTextRange {
        lines: lines[offset..end]
            .iter()
            .map(|line| (*line).to_string())
            .collect(),
        next_offset: (end < lines.len()).then_some(end),
        eof: end == lines.len(),
        total_lines: Some(lines.len()),
    })
}

#[cfg(test)]
#[path = "read/batch_tests.rs"]
mod batch_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{
        WorkspaceDirEntry, WorkspaceError, WorkspaceFileSystem, WorkspacePath, WorkspaceRef,
        WorkspaceResult, WorkspaceServices, WorkspaceTextReader, WorkspaceWriteOutcome,
    };
    use async_trait::async_trait;
    use std::path::PathBuf;
    use std::sync::Arc;

    struct RangeOnlyBackend;

    #[async_trait]
    impl WorkspaceFileSystem for RangeOnlyBackend {
        async fn read_text(&self, _path: &WorkspacePath) -> WorkspaceResult<String> {
            panic!("range-capable read must not fall back to whole-file read_text")
        }

        async fn write_text(
            &self,
            _path: &WorkspacePath,
            _content: &str,
        ) -> WorkspaceResult<WorkspaceWriteOutcome> {
            Err(WorkspaceError::InvalidArgument {
                message: "write_text is unsupported".to_string(),
            })
        }

        async fn list_dir(&self, _path: &WorkspacePath) -> WorkspaceResult<Vec<WorkspaceDirEntry>> {
            Err(WorkspaceError::InvalidArgument {
                message: "list_dir is unsupported".to_string(),
            })
        }
    }

    #[async_trait]
    impl WorkspaceTextReader for RangeOnlyBackend {
        async fn read_text_range(
            &self,
            _path: &WorkspacePath,
            offset: usize,
            limit: usize,
        ) -> WorkspaceResult<WorkspaceTextRange> {
            assert_eq!(offset, 7);
            assert_eq!(limit, 2);
            Ok(WorkspaceTextRange {
                lines: vec!["eight".to_string(), "nine".to_string()],
                next_offset: Some(9),
                eof: false,
                total_lines: None,
            })
        }
    }

    #[tokio::test]
    async fn test_read_file() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("test.txt");
        std::fs::write(&file, "line1\nline2\nline3\n").unwrap();

        let tool = ReadTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());
        let result = tool
            .execute(&serde_json::json!({"file_path": "test.txt"}), &ctx)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("line1"));
        assert!(result.content.contains("line2"));
        assert!(result.content.contains("line3"));
        assert_eq!(
            result.metadata.unwrap()["source_anchors"],
            serde_json::json!(["test.txt"])
        );
    }

    #[tokio::test]
    async fn test_read_with_offset_and_limit() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("test.txt");
        std::fs::write(&file, "a\nb\nc\nd\ne\n").unwrap();

        let tool = ReadTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());
        let result = tool
            .execute(
                &serde_json::json!({"file_path": "test.txt", "offset": 1, "limit": 2}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("b"));
        assert!(result.content.contains("c"));
        assert!(!result.content.contains("\ta\n"));
    }

    #[tokio::test]
    async fn test_read_uses_streaming_range_capability_without_whole_file_read() {
        let backend = Arc::new(RangeOnlyBackend);
        let fs: Arc<dyn WorkspaceFileSystem> = backend.clone();
        let reader: Arc<dyn WorkspaceTextReader> = backend;
        let services = WorkspaceServices::builder(WorkspaceRef::new("range", "range://ws"), fs)
            .text_reader(reader)
            .build();
        let ctx = ToolContext::new(std::env::temp_dir()).with_workspace_services(services);

        let result = ReadTool
            .execute(
                &serde_json::json!({"file_path": "large.txt", "offset": 7, "limit": 2}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success, "{}", result.content);
        assert!(result.content.contains("eight"));
        assert!(result.content.contains("offset=9"));
    }

    #[tokio::test]
    async fn test_read_clamps_oversized_limit_and_reports_metadata() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("test.txt"), "one\ntwo\n").unwrap();
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = ReadTool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "limit": MAX_READ_LINES + 1,
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success, "{}", result.content);
        let range = &result.metadata.unwrap()["range"];
        assert_eq!(range["applied_limit"], MAX_READ_LINES);
        assert_eq!(range["limit_clamped"], true);
    }

    #[tokio::test]
    async fn test_read_at_eof_is_successful_empty_tail() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("test.txt");
        std::fs::write(&file, "a\nb\nc\n").unwrap();

        let tool = ReadTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());
        let result = tool
            .execute(
                &serde_json::json!({"file_path": "test.txt", "offset": 3, "limit": 20}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("end of file"));
    }

    #[tokio::test]
    async fn test_read_missing_file() {
        let temp = tempfile::tempdir().unwrap();
        let tool = ReadTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());
        let result = tool
            .execute(&serde_json::json!({"file_path": "nonexistent.txt"}), &ctx)
            .await
            .unwrap();

        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_read_missing_param() {
        let tool = ReadTool;
        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();

        assert!(!result.success);
        assert!(result.content.contains("file_path"));
    }

    #[test]
    fn test_read_schema_is_canonical() {
        let tool = ReadTool;
        let params = tool.parameters();
        assert_eq!(params["additionalProperties"], false);
        assert_eq!(params["oneOf"].as_array().unwrap().len(), 2);
        assert_eq!(
            params["properties"]["files"]["maxItems"],
            serde_json::json!(32)
        );
        let examples = params["examples"].as_array().unwrap();
        assert_eq!(examples[0]["file_path"], "src/main.rs");
        assert!(examples[0].get("path").is_none());
        assert_eq!(examples[2]["files"][0]["path"], "src/lib.rs");
    }

    #[test]
    fn test_read_schema_accepts_exactly_one_input_shape() {
        let schema = ReadTool.parameters();
        let validator = jsonschema::draft202012::options().build(&schema).unwrap();

        assert!(validator.is_valid(&serde_json::json!({"file_path": "README.md"})));
        assert!(validator.is_valid(&serde_json::json!({
            "files": [{"path": "README.md"}]
        })));
        assert!(!validator.is_valid(&serde_json::json!({})));
        assert!(!validator.is_valid(&serde_json::json!({
            "file_path": "README.md",
            "files": [{"path": "README.md"}]
        })));
    }

    #[tokio::test]
    async fn test_read_truncation_at_utf8_boundary() {
        // Regression test: truncation at byte 2000 should not panic
        // when byte 2000 falls inside a multibyte UTF-8 character.
        // "频" is 3 bytes (bytes 1999..2002). When byte 2000 is
        // inside '频', truncation must find a valid char boundary.
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("boundary.txt");
        // 1999 ASCII bytes + one 3-byte UTF-8 char + trailing ASCII.
        // Byte 2000 is inside the '频' character (bytes 1999..2002).
        let content = "a".repeat(1999) + "频" + &"z".repeat(20);
        std::fs::write(&file, &content).unwrap();

        let tool = ReadTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());
        // Should not panic
        let result = tool
            .execute(&serde_json::json!({"file_path": "boundary.txt"}), &ctx)
            .await
            .unwrap();

        assert!(
            result.success,
            "read should succeed, got error: {}",
            result.content
        );
        // Verify the truncated content is valid UTF-8
        assert!(!result.content.contains("byte index"));
    }
}
