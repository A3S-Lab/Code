//! Grep tool - Search file contents with regex

use crate::tools::pagination::{PageRequest, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::tools::MAX_OUTPUT_SIZE;
use crate::workspace::{WorkspaceGrepRequest, WorkspaceGrepResult, WorkspacePath};
use anyhow::Result;
use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use regex::Regex;
use std::collections::{HashMap, HashSet};

const MAX_GREP_SOURCE_ANCHORS: usize = 64;
const MAX_GREP_FALLBACK_CANDIDATES: usize = MAX_GREP_SOURCE_ANCHORS * 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GrepOutputMode {
    Content,
    FilesWithMatches,
    Count,
    Summary,
}

impl GrepOutputMode {
    fn parse(args: &serde_json::Value) -> std::result::Result<Self, String> {
        match args
            .get("output_mode")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("content")
        {
            "content" => Ok(Self::Content),
            "files_with_matches" => Ok(Self::FilesWithMatches),
            "count" => Ok(Self::Count),
            "summary" => Ok(Self::Summary),
            _ => Err(
                "output_mode must be 'content', 'files_with_matches', 'count', or 'summary'"
                    .to_string(),
            ),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::FilesWithMatches => "files_with_matches",
            Self::Count => "count",
            Self::Summary => "summary",
        }
    }

    fn paginated(self) -> bool {
        matches!(self, Self::FilesWithMatches | Self::Count)
    }
}

fn has_non_neutral_page_controls(args: &serde_json::Value) -> bool {
    let limit_is_non_neutral = match args.get("limit") {
        None | Some(serde_json::Value::Null) => false,
        Some(value) => value.as_u64() != Some(DEFAULT_PAGE_LIMIT as u64),
    };
    let cursor_is_non_neutral = match args.get("cursor") {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::String(value)) => !value.trim().is_empty(),
        Some(_) => true,
    };
    limit_is_non_neutral || cursor_is_non_neutral
}

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents with regex. Return matching content, a paginated file list, per-file matching-line counts, or a compact full-scan summary."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Required. Regular expression pattern to search for. Always provide this exact field name: 'pattern'."
                },
                "path": {
                    "type": "string",
                    "description": "Optional. Directory or file to search in. Default: workspace root."
                },
                "glob": {
                    "type": "string",
                    "description": "Optional. Glob pattern to filter files, for example '*.rs' or '*.{ts,tsx}'."
                },
                "context": {
                    "type": "integer",
                    "description": "Optional. Number of context lines to show before and after matches."
                },
                "-i": {
                    "type": "boolean",
                    "description": "Optional. Case insensitive search."
                },
                "output_mode": {
                    "type": "string",
                    "enum": ["content", "files_with_matches", "count", "summary"],
                    "description": "Optional. content returns matching lines (default); files_with_matches returns lexically paginated paths; count returns lexically paginated matching-line counts per file; summary returns only full-scan totals."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_PAGE_LIMIT,
                    "description": "Optional for files_with_matches/count. Maximum files to return. Default: 200; maximum: 1000."
                },
                "cursor": {
                    "type": "string",
                    "description": "Optional for files_with_matches/count. Copy the exact cursor from the previous result."
                }
            },
            "required": ["pattern"],
            "examples": [
                {
                    "pattern": "TODO"
                },
                {
                    "pattern": "fn main",
                    "path": "src",
                    "glob": "*.rs",
                    "context": 2,
                    "output_mode": "content"
                },
                {
                    "pattern": "TODO",
                    "output_mode": "files_with_matches",
                    "limit": 100
                }
            ]
        })
    }

    fn capabilities(&self, args: &serde_json::Value) -> crate::tools::ToolCapabilities {
        if GrepOutputMode::parse(args).is_ok_and(GrepOutputMode::paginated) {
            crate::tools::ToolCapabilities::read_only_paginated(16)
        } else {
            crate::tools::ToolCapabilities::parallel_safe_read(16)
        }
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let pattern_str = match args.get("pattern").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("pattern parameter is required")),
        };
        let output_mode = match GrepOutputMode::parse(args) {
            Ok(mode) => mode,
            Err(error) => return Ok(ToolOutput::error(error)),
        };
        let page_request = if output_mode.paginated() {
            match PageRequest::parse(args, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT) {
                Ok(request) => Some(request),
                Err(error) => return Ok(ToolOutput::error(error)),
            }
        } else {
            // Structured-output providers can materialize omitted optional
            // fields with their neutral schema defaults. Do not reject a
            // content/summary call merely because it carries limit=200 and an
            // empty cursor; neither value changes the non-paginated result.
            if has_non_neutral_page_controls(args) {
                return Ok(ToolOutput::error(
                    "non-default limit and non-empty cursor are supported only with output_mode='files_with_matches' or 'count'",
                ));
            }
            None
        };

        let case_insensitive = args.get("-i").and_then(|v| v.as_bool()).unwrap_or(false);

        let regex_pattern = if case_insensitive {
            format!("(?i){}", pattern_str)
        } else {
            pattern_str.to_string()
        };

        let regex = match Regex::new(&regex_pattern) {
            Ok(regex) => regex,
            Err(e) => {
                return Ok(ToolOutput::error(format!(
                    "Invalid regex pattern '{}': {}",
                    pattern_str, e
                )))
            }
        };

        let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let base = match ctx.resolve_workspace_path(path_str) {
            Ok(path) => path,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to resolve path: {}", e))),
        };

        let glob_filter = args.get("glob").and_then(|v| v.as_str());
        let context_lines = args.get("context").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

        let Some(search) = ctx.workspace_services.search() else {
            return Ok(ToolOutput::error(
                "grep is not available: this workspace backend did not provide search",
            ));
        };
        let request = WorkspaceGrepRequest {
            base,
            pattern: pattern_str.to_string(),
            glob: glob_filter.map(str::to_string),
            context_lines,
            case_insensitive,
            // Metadata-only modes ask built-in backends to count all matches
            // without constructing content that the caller does not want.
            max_output_size: if output_mode == GrepOutputMode::Content {
                MAX_OUTPUT_SIZE
            } else {
                0
            },
        };
        let anchor_request = request.clone();
        let outcome = match ctx
            .workspace_services
            .run_with_timeout(
                "grep",
                async move { search.grep_with_sources(request).await },
            )
            .await
        {
            Ok(result) => result,
            Err(e) => return Ok(ToolOutput::error(format!("Grep search failed: {}", e))),
        };

        let result = outcome.result;
        let matched_paths = outcome.matched_paths;

        if output_mode == GrepOutputMode::Summary {
            let mut content = format!(
                "{} matching line(s) in {} file(s)",
                result.match_count, result.file_count
            );
            if result.truncated {
                content.push_str(" (scan truncated by backend limits)");
            }
            return Ok(
                ToolOutput::success(content).with_metadata(serde_json::json!({
                    "output_mode": output_mode.as_str(),
                    "search": grep_search_metadata(&result),
                })),
            );
        }

        if output_mode.paginated() {
            let Some(page_request) = page_request else {
                return Ok(ToolOutput::error(
                    "Unable to construct the requested grep page",
                ));
            };
            let Some(matched_paths) = matched_paths else {
                return Ok(ToolOutput::error(format!(
                    "output_mode='{}' requires structured match paths from the workspace backend; use output_mode='content' with this backend",
                    output_mode.as_str()
                ))
                .with_metadata(serde_json::json!({
                    "output_mode": output_mode.as_str(),
                    "search": grep_search_metadata(&result),
                })));
            };
            return render_paginated_grep(
                output_mode,
                page_request,
                matched_paths,
                result,
                &regex,
                ctx,
            )
            .await;
        }

        let source_anchors = grep_source_anchors(
            &result,
            matched_paths.as_deref(),
            &anchor_request,
            &regex,
            ctx,
        )
        .await;
        let content = if result.match_count == 0 {
            format!("No matches found for pattern: {}", pattern_str)
        } else if result.truncated {
            format!(
                "{}\n... (output truncated)\nFound {} matches in {} files (output truncated)",
                result.output, result.match_count, result.file_count
            )
        } else {
            format!(
                "{}\n{} match(es) in {} file(s)",
                result.output, result.match_count, result.file_count
            )
        };
        let output = ToolOutput::success(content);
        if source_anchors.is_empty() {
            Ok(output)
        } else {
            Ok(output.with_metadata(serde_json::json!({
                "source_anchors": source_anchors,
            })))
        }
    }
}

async fn render_paginated_grep(
    output_mode: GrepOutputMode,
    page_request: PageRequest,
    matched_paths: Vec<WorkspacePath>,
    result: WorkspaceGrepResult,
    regex: &Regex,
    ctx: &ToolContext,
) -> Result<ToolOutput> {
    let mut seen = HashSet::new();
    let mut paths = matched_paths
        .into_iter()
        .filter_map(|path| ctx.resolve_workspace_path(path.as_str()).ok())
        .filter(|path| !path.is_root() && seen.insert(path.as_str().to_string()))
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    let page = match page_request.page(paths) {
        Ok(page) => page,
        Err(error) => return Ok(ToolOutput::error(error)),
    };
    let page_metadata = page.metadata();
    let next_cursor = page.next_cursor.clone();
    let total_items = page.total_items;
    let source_anchors = page
        .items
        .iter()
        .map(|path| path.as_str().to_string())
        .collect::<Vec<_>>();

    let (mut content, failed_reads) = match output_mode {
        GrepOutputMode::FilesWithMatches => (source_anchors.join("\n"), 0usize),
        GrepOutputMode::Count => count_matching_lines(page.items, regex, ctx).await,
        GrepOutputMode::Content | GrepOutputMode::Summary => {
            return Ok(ToolOutput::error(
                "Only files_with_matches and count support grep pagination",
            ));
        }
    };
    if content.is_empty() {
        content = "No matches found".to_string();
    } else {
        content.push_str(&format!(
            "\n\n{} of {total_items} matching file(s) shown",
            source_anchors.len()
        ));
        if let Some(cursor) = next_cursor {
            content.push_str(&format!(
                "\nMore files available; continue with cursor={cursor}"
            ));
        }
    }
    if result.truncated {
        content.push_str(
            "\nWarning: the workspace backend reached its scan limit; totals and paths may be incomplete.",
        );
    }

    Ok(
        ToolOutput::success(content).with_metadata(serde_json::json!({
            "output_mode": output_mode.as_str(),
            "sort": "path",
            "source_anchors": source_anchors,
            "page": page_metadata,
            "search": grep_search_metadata(&result),
            "failed_reads": failed_reads,
        })),
    )
}

async fn count_matching_lines(
    paths: Vec<WorkspacePath>,
    regex: &Regex,
    ctx: &ToolContext,
) -> (String, usize) {
    let calls = paths.into_iter().map(|path| {
        let services = ctx.workspace_services.clone();
        let fs = services.fs();
        let regex = regex.clone();
        async move {
            let read_path = path.clone();
            let result = services
                .run_with_timeout(
                    "grep count read",
                    async move { fs.read_text(&read_path).await },
                )
                .await;
            (path, result, regex)
        }
    });
    let results = stream::iter(calls).buffered(16).collect::<Vec<_>>().await;
    let mut rows = Vec::with_capacity(results.len());
    let mut failed_reads = 0usize;
    for (path, content, regex) in results {
        match content {
            Ok(content) => {
                let count = content.lines().filter(|line| regex.is_match(line)).count();
                rows.push(format!("{}: {count} matching line(s)", path.as_str()));
            }
            Err(error) => {
                failed_reads += 1;
                rows.push(format!("{}: unable to recount ({error})", path.as_str()));
            }
        }
    }
    (rows.join("\n"), failed_reads)
}

fn grep_search_metadata(result: &WorkspaceGrepResult) -> serde_json::Value {
    serde_json::json!({
        "matching_lines": result.match_count,
        "matching_files": result.file_count,
        "truncated": result.truncated,
    })
}

async fn grep_source_anchors(
    result: &WorkspaceGrepResult,
    matched_paths: Option<&[WorkspacePath]>,
    request: &WorkspaceGrepRequest,
    regex: &Regex,
    ctx: &ToolContext,
) -> Vec<String> {
    let mut anchors = Vec::new();
    let mut seen = HashSet::new();
    if let Some(matched_paths) = matched_paths {
        for path in matched_paths {
            let Ok(path) = ctx.resolve_workspace_path(path.as_str()) else {
                continue;
            };
            if !path.is_root() {
                let path = path.as_str().to_string();
                if !seen.insert(path.clone()) {
                    continue;
                }
                anchors.push(path);
                if anchors.len() >= MAX_GREP_SOURCE_ANCHORS {
                    break;
                }
            }
        }
        return anchors;
    }

    // Legacy/custom backends may provide only display output. Treat it as
    // untrusted: parse only actual-match lines, bound delimiter scanning, and
    // verify every candidate against the original request and file contents.
    let mut candidates: Vec<(String, Vec<usize>)> = Vec::new();
    let mut candidate_indices: HashMap<String, usize> = HashMap::new();
    let mut scanned_candidates = 0usize;
    'lines: for line in result.output.lines() {
        let Some(line) = line.strip_prefix('>') else {
            continue;
        };
        for (delimiter, _) in line.match_indices(':') {
            if scanned_candidates >= MAX_GREP_FALLBACK_CANDIDATES {
                break 'lines;
            }
            scanned_candidates += 1;

            let remainder = &line[delimiter + 1..];
            let digit_count = remainder.bytes().take_while(u8::is_ascii_digit).count();
            if digit_count == 0 || remainder.as_bytes().get(digit_count) != Some(&b':') {
                continue;
            }
            let candidate = &line[..delimiter];
            if candidate.is_empty() || candidate.chars().any(char::is_control) {
                continue;
            }
            let Ok(line_number) = remainder[..digit_count].parse::<usize>() else {
                continue;
            };
            if line_number == 0 {
                continue;
            }
            let Ok(path) = ctx.resolve_workspace_path(candidate) else {
                continue;
            };
            if path.is_root() || !path_matches_grep_request(&path, request) {
                continue;
            }
            let path = path.as_str().to_string();
            if let Some(index) = candidate_indices.get(&path).copied() {
                candidates[index].1.push(line_number);
            } else {
                candidate_indices.insert(path.clone(), candidates.len());
                candidates.push((path, vec![line_number]));
            }
        }
    }

    let fs = ctx.workspace_services.fs();
    for (path, line_numbers) in candidates {
        let workspace_path = WorkspacePath::from_normalized(path.clone());
        let Ok(content) = ctx
            .workspace_services
            .run_with_timeout("grep source verification", fs.read_text(&workspace_path))
            .await
        else {
            continue;
        };
        if line_numbers.into_iter().any(|line_number| {
            content
                .lines()
                .nth(line_number - 1)
                .is_some_and(|line| regex.is_match(line))
        }) {
            if !seen.insert(path.clone()) {
                continue;
            }
            anchors.push(path);
            if anchors.len() >= MAX_GREP_SOURCE_ANCHORS {
                break;
            }
        }
    }
    anchors
}

fn path_matches_grep_request(path: &WorkspacePath, request: &WorkspaceGrepRequest) -> bool {
    let relative = if request.base.is_root() {
        path.as_str()
    } else if path == &request.base {
        path.as_str().rsplit('/').next().unwrap_or(path.as_str())
    } else {
        let Some(relative) = path
            .as_str()
            .strip_prefix(request.base.as_str())
            .and_then(|suffix| suffix.strip_prefix('/'))
        else {
            return false;
        };
        relative
    };

    let Some(glob) = request.glob.as_deref() else {
        return true;
    };
    let Ok(pattern) = glob::Pattern::new(glob) else {
        return false;
    };
    if glob.contains('/') {
        pattern.matches(relative)
    } else {
        pattern.matches(relative.rsplit('/').next().unwrap_or(relative))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_grep_find_pattern() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("a.txt"),
            "hello world\nfoo bar\nhello again",
        )
        .unwrap();
        std::fs::write(temp.path().join("b.txt"), "no match here").unwrap();

        let tool = GrepTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(&serde_json::json!({"pattern": "hello"}), &ctx)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("hello world"));
        assert!(result.content.contains("hello again"));
        assert!(result.content.contains("2 match(es)"));
        assert_eq!(
            result.metadata.unwrap()["source_anchors"],
            serde_json::json!(["a.txt"])
        );
    }

    #[tokio::test]
    async fn test_grep_no_match() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("a.txt"), "hello").unwrap();

        let tool = GrepTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(&serde_json::json!({"pattern": "xyz"}), &ctx)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("No matches found"));
    }

    #[tokio::test]
    async fn test_grep_case_insensitive() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("a.txt"), "Hello World\nhello world").unwrap();

        let tool = GrepTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(&serde_json::json!({"pattern": "hello", "-i": true}), &ctx)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("2 match(es)"));
    }

    #[tokio::test]
    async fn test_grep_files_mode_is_paginated_without_rendering_matches() {
        let temp = tempfile::tempdir().unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(temp.path().join(name), format!("needle in {name}")).unwrap();
        }
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let first = GrepTool
            .execute(
                &serde_json::json!({
                    "pattern": "needle",
                    "output_mode": "files_with_matches",
                    "limit": 2
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(first.success, "{}", first.content);
        assert!(!first.content.contains("needle in"));
        let page = &first.metadata.as_ref().unwrap()["page"];
        assert_eq!(page["returned_items"], 2);
        assert_eq!(page["total_items"], 3);
        assert_eq!(page["next_cursor"], "2");

        let second = GrepTool
            .execute(
                &serde_json::json!({
                    "pattern": "needle",
                    "output_mode": "files_with_matches",
                    "limit": 2,
                    "cursor": "2"
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(second.success, "{}", second.content);
        assert_eq!(second.metadata.unwrap()["page"]["returned_items"], 1);
    }

    #[tokio::test]
    async fn test_grep_paginated_modes_sort_paths_before_applying_cursor() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(temp.path().to_path_buf());
        let result = render_paginated_grep(
            GrepOutputMode::FilesWithMatches,
            PageRequest {
                offset: 0,
                requested_limit: 2,
                limit: 2,
            },
            ["c.txt", "a.txt", "b.txt"]
                .into_iter()
                .map(WorkspacePath::from_normalized)
                .collect(),
            WorkspaceGrepResult {
                output: String::new(),
                match_count: 3,
                file_count: 3,
                truncated: false,
            },
            &Regex::new("needle").unwrap(),
            &ctx,
        )
        .await
        .unwrap();

        assert!(result.success, "{}", result.content);
        assert_eq!(
            result.metadata.unwrap()["source_anchors"],
            serde_json::json!(["a.txt", "b.txt"])
        );
    }

    #[tokio::test]
    async fn test_grep_count_mode_reports_matching_lines_per_file() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("a.txt"), "needle\nnone\nneedle\n").unwrap();
        std::fs::write(temp.path().join("b.txt"), "needle\n").unwrap();
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = GrepTool
            .execute(
                &serde_json::json!({
                    "pattern": "needle",
                    "output_mode": "count"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success, "{}", result.content);
        assert!(result.content.contains("a.txt: 2 matching line(s)"));
        assert!(result.content.contains("b.txt: 1 matching line(s)"));
        assert!(!result.content.contains(">a.txt"));
    }

    #[tokio::test]
    async fn test_grep_summary_counts_full_scan_without_rendering_content() {
        let temp = tempfile::tempdir().unwrap();
        let content = (0..80)
            .map(|index| format!("needle-{index}-{}", "x".repeat(2_000)))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(temp.path().join("large.txt"), content).unwrap();
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = GrepTool
            .execute(
                &serde_json::json!({
                    "pattern": "needle",
                    "output_mode": "summary"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success, "{}", result.content);
        assert!(result.content.contains("80 matching line(s) in 1 file(s)"));
        assert!(!result.content.contains("needle-0"));
        assert_eq!(result.metadata.unwrap()["search"]["truncated"], false);
    }

    #[tokio::test]
    async fn test_grep_non_paginated_mode_accepts_materialized_neutral_page_defaults() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("a.txt"), "needle\n").unwrap();
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = GrepTool
            .execute(
                &serde_json::json!({
                    "pattern": "needle",
                    "output_mode": "content",
                    "limit": DEFAULT_PAGE_LIMIT,
                    "cursor": ""
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success, "{}", result.content);
        assert!(result.content.contains("needle"));
    }

    #[tokio::test]
    async fn test_grep_non_paginated_mode_rejects_effective_page_controls() {
        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        for controls in [
            serde_json::json!({"limit": 2}),
            serde_json::json!({"cursor": "2"}),
        ] {
            let mut args = serde_json::json!({
                "pattern": "needle",
                "output_mode": "summary"
            });
            args.as_object_mut()
                .unwrap()
                .extend(controls.as_object().unwrap().clone());
            let result = GrepTool.execute(&args, &ctx).await.unwrap();
            assert!(!result.success);
            assert!(result.content.contains("supported only"));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_grep_source_anchor_preserves_newline_filename_without_injection() {
        let temp = tempfile::tempdir().unwrap();
        let filename = "actual\n>injected.txt";
        std::fs::write(temp.path().join(filename), "needle").unwrap();
        let tool = GrepTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(&serde_json::json!({"pattern": "needle"}), &ctx)
            .await
            .unwrap();

        assert!(result.content.contains(r"actual\n>injected.txt"));
        assert!(!result
            .content
            .lines()
            .any(|line| line.starts_with(">injected.txt:")));
        assert_eq!(
            result.metadata.unwrap()["source_anchors"],
            serde_json::json!([filename])
        );
    }

    #[tokio::test]
    #[cfg(not(windows))]
    async fn test_grep_source_anchor_preserves_colon_filename() {
        let temp = tempfile::tempdir().unwrap();
        let filename = "notes:2026.txt";
        std::fs::write(temp.path().join(filename), "needle").unwrap();
        let tool = GrepTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(&serde_json::json!({"pattern": "needle"}), &ctx)
            .await
            .unwrap();

        assert_eq!(
            result.metadata.unwrap()["source_anchors"],
            serde_json::json!([filename])
        );
    }

    #[tokio::test]
    async fn test_grep_source_anchor_rejects_nonexistent_injected_path() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(temp.path().to_path_buf());
        let request = WorkspaceGrepRequest {
            base: WorkspacePath::root(),
            pattern: "needle".to_string(),
            glob: None,
            context_lines: 0,
            case_insensitive: false,
            max_output_size: MAX_OUTPUT_SIZE,
        };
        let result = WorkspaceGrepResult {
            output: ">ghost.txt:1: needle\n".to_string(),
            match_count: 1,
            file_count: 1,
            truncated: false,
        };

        assert!(grep_source_anchors(
            &result,
            None,
            &request,
            &Regex::new("needle").unwrap(),
            &ctx
        )
        .await
        .is_empty());
    }

    #[tokio::test]
    async fn test_grep_invalid_regex() {
        let tool = GrepTool;
        let ctx = ToolContext::new(PathBuf::from("/tmp"));

        let result = tool
            .execute(&serde_json::json!({"pattern": "[invalid"}), &ctx)
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.content.contains("Invalid regex"));
    }

    #[tokio::test]
    async fn test_grep_missing_pattern() {
        let tool = GrepTool;
        let ctx = ToolContext::new(PathBuf::from("/tmp"));

        let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();
        assert!(!result.success);
    }

    #[test]
    fn test_grep_schema_is_canonical() {
        let tool = GrepTool;
        let params = tool.parameters();
        assert_eq!(params["additionalProperties"], false);
        assert_eq!(params["required"], serde_json::json!(["pattern"]));
        let examples = params["examples"].as_array().unwrap();
        assert_eq!(examples[0]["pattern"], "TODO");
        assert!(examples[0].get("query").is_none());
        assert_eq!(
            params["properties"]["output_mode"]["enum"],
            serde_json::json!(["content", "files_with_matches", "count", "summary"])
        );
    }
}
