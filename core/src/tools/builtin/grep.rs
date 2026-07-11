//! Grep tool - Search file contents with regex

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::tools::MAX_OUTPUT_SIZE;
use crate::workspace::{WorkspaceGrepRequest, WorkspaceGrepResult, WorkspacePath};
use anyhow::Result;
use async_trait::async_trait;
use regex::Regex;
use std::collections::{HashMap, HashSet};

const MAX_GREP_SOURCE_ANCHORS: usize = 64;
const MAX_GREP_FALLBACK_CANDIDATES: usize = MAX_GREP_SOURCE_ANCHORS * 4;

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search for a pattern in files using ripgrep. Returns matching lines with file paths and line numbers."
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
                    "context": 2
                }
            ]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let pattern_str = match args.get("pattern").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("pattern parameter is required")),
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
            max_output_size: MAX_OUTPUT_SIZE,
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

        let source_anchors = grep_source_anchors(
            &outcome.result,
            outcome.matched_paths.as_deref(),
            &anchor_request,
            &regex,
            ctx,
        )
        .await;
        let result = outcome.result;
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
    }
}
