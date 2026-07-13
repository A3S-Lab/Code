//! Patch tool - Apply unified diff patches to files

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::workspace::WorkspaceError;
use anyhow::Result;
use async_trait::async_trait;

pub struct PatchTool;

/// A parsed hunk from a unified diff
#[derive(Debug)]
struct Hunk {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    operations: Vec<HunkOperation>,
    old_no_newline: bool,
    new_no_newline: bool,
}

#[derive(Debug)]
enum HunkOperation {
    Context(String),
    Remove(String),
    Add(String),
}

/// Parse a unified diff into hunks
fn parse_hunks(diff: &str) -> Result<Vec<Hunk>, String> {
    let mut hunks = Vec::new();
    let lines = diff
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect::<Vec<_>>();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("@@") {
            let hunk = parse_single_hunk(&lines, &mut i)?;
            hunks.push(hunk);
        } else {
            if !hunks.is_empty() && !line.is_empty() {
                return Err(format!(
                    "Unexpected line outside a hunk at diff line {}: {:?}",
                    i + 1,
                    line
                ));
            }
            i += 1;
        }
    }

    if hunks.is_empty() {
        return Err("No @@ hunk headers found in diff".to_string());
    }

    Ok(hunks)
}

fn parse_single_hunk(lines: &[&str], i: &mut usize) -> Result<Hunk, String> {
    let header = lines[*i];
    let (old_start, old_count, new_start, new_count) = parse_hunk_header(header)?;

    *i += 1;
    let mut operations = Vec::new();
    let mut old_no_newline = false;
    let mut new_no_newline = false;
    let mut previous_kind = None;

    while *i < lines.len() {
        let line = lines[*i];
        if line.starts_with("@@") {
            break;
        }
        if line == "\\ No newline at end of file" {
            match previous_kind {
                Some('-') => old_no_newline = true,
                Some('+') => new_no_newline = true,
                Some(' ') => {
                    old_no_newline = true;
                    new_no_newline = true;
                }
                _ => {
                    return Err(format!(
                        "No-newline marker has no preceding hunk line at diff line {}",
                        *i + 1
                    ))
                }
            }
            *i += 1;
            continue;
        }

        if let Some(content) = line.strip_prefix(' ') {
            operations.push(HunkOperation::Context(content.to_string()));
            previous_kind = Some(' ');
        } else if let Some(content) = line.strip_prefix('-') {
            operations.push(HunkOperation::Remove(content.to_string()));
            previous_kind = Some('-');
        } else if let Some(content) = line.strip_prefix('+') {
            operations.push(HunkOperation::Add(content.to_string()));
            previous_kind = Some('+');
        } else {
            if line.is_empty() && *i + 1 == lines.len() {
                break;
            }
            return Err(format!(
                "Invalid hunk line prefix at diff line {}: {:?}",
                *i + 1,
                line
            ));
        }
        *i += 1;
    }

    let actual_old_count = operations
        .iter()
        .filter(|operation| !matches!(operation, HunkOperation::Add(_)))
        .count();
    let actual_new_count = operations
        .iter()
        .filter(|operation| !matches!(operation, HunkOperation::Remove(_)))
        .count();
    if actual_old_count != old_count || actual_new_count != new_count {
        return Err(format!(
            "Hunk count mismatch in {header:?}: header declares old={old_count}, new={new_count}, but body contains old={actual_old_count}, new={actual_new_count}"
        ));
    }

    Ok(Hunk {
        old_start,
        old_count,
        new_start,
        new_count,
        operations,
        old_no_newline,
        new_no_newline,
    })
}

/// Parse @@ -old_start,old_count +new_start,new_count @@ line
fn parse_hunk_header(header: &str) -> Result<(usize, usize, usize, usize), String> {
    let stripped = header
        .strip_prefix("@@")
        .and_then(|s| s.find("@@").map(|end| &s[..end]))
        .ok_or_else(|| format!("Invalid hunk header: {}", header))?
        .trim();

    let parts: Vec<&str> = stripped.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(format!("Invalid hunk header: {}", header));
    }
    let (old_start, old_count) = parse_hunk_range(parts[0], '-', header)?;
    let (new_start, new_count) = parse_hunk_range(parts[1], '+', header)?;
    Ok((old_start, old_count, new_start, new_count))
}

fn parse_hunk_range(value: &str, prefix: char, header: &str) -> Result<(usize, usize), String> {
    let value = value
        .strip_prefix(prefix)
        .ok_or_else(|| format!("Invalid range in hunk header: {header}"))?;
    let (start, count) = value
        .split_once(',')
        .map(|(start, count)| (start, Some(count)))
        .unwrap_or((value, None));
    let start = start
        .parse::<usize>()
        .map_err(|_| format!("Invalid range start in hunk header: {header}"))?;
    let count = count
        .unwrap_or("1")
        .parse::<usize>()
        .map_err(|_| format!("Invalid range count in hunk header: {header}"))?;
    Ok((start, count))
}

struct TextDocument {
    lines: Vec<String>,
    line_ending: &'static str,
    trailing_newline: bool,
}

fn parse_document(content: &str) -> Result<TextDocument, String> {
    let has_crlf = content.contains("\r\n");
    let has_bare_lf = content.as_bytes().iter().enumerate().any(|(index, byte)| {
        *byte == b'\n' && (index == 0 || content.as_bytes()[index - 1] != b'\r')
    });
    if has_crlf && has_bare_lf {
        return Err("Cannot safely patch a file with mixed LF and CRLF line endings".to_string());
    }
    if content.contains('\r') && !has_crlf {
        return Err("Cannot safely patch a file with unsupported CR-only line endings".to_string());
    }

    let trailing_newline = content.ends_with('\n');
    let mut lines = if content.is_empty() {
        Vec::new()
    } else {
        content
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
            .collect::<Vec<_>>()
    };
    if trailing_newline {
        lines.pop();
    }
    Ok(TextDocument {
        lines,
        line_ending: if has_crlf { "\r\n" } else { "\n" },
        trailing_newline,
    })
}

/// Apply hunks strictly from top to bottom against exact source lines.
fn apply_hunks(content: &str, hunks: &[Hunk]) -> Result<String, String> {
    let document = parse_document(content)?;
    let mut output = Vec::new();
    let mut old_cursor = 0usize;
    let mut trailing_newline = document.trailing_newline;

    for hunk in hunks {
        let old_target = if hunk.old_count == 0 {
            hunk.old_start
        } else {
            hunk.old_start.saturating_sub(1)
        };
        let new_target = if hunk.new_count == 0 {
            hunk.new_start
        } else {
            hunk.new_start.saturating_sub(1)
        };
        if old_target < old_cursor || old_target > document.lines.len() {
            return Err(format!(
                "Hunk old range starts outside the remaining source at line {}",
                hunk.old_start
            ));
        }
        output.extend_from_slice(&document.lines[old_cursor..old_target]);
        if output.len() != new_target {
            return Err(format!(
                "Hunk new range starts at line {}, but prior hunks produce line {}",
                hunk.new_start,
                output.len() + 1
            ));
        }

        let mut source_index = old_target;
        for operation in &hunk.operations {
            match operation {
                HunkOperation::Context(expected) => {
                    require_exact_line(&document.lines, source_index, expected, "context")?;
                    output.push(expected.clone());
                    source_index += 1;
                }
                HunkOperation::Remove(expected) => {
                    require_exact_line(&document.lines, source_index, expected, "removal")?;
                    source_index += 1;
                }
                HunkOperation::Add(line) => output.push(line.clone()),
            }
        }
        old_cursor = source_index;

        let touches_old_eof = old_cursor == document.lines.len();
        if touches_old_eof {
            if hunk.new_no_newline {
                trailing_newline = false;
            } else if hunk.old_no_newline {
                trailing_newline = true;
            }
        }
        if hunk.old_no_newline && document.trailing_newline && touches_old_eof {
            return Err(
                "Patch claims the source has no final newline, but the file does".to_string(),
            );
        }
    }

    output.extend_from_slice(&document.lines[old_cursor..]);
    let mut rendered = output.join(document.line_ending);
    if trailing_newline {
        rendered.push_str(document.line_ending);
    }
    Ok(rendered)
}

fn require_exact_line(
    lines: &[String],
    index: usize,
    expected: &str,
    kind: &str,
) -> Result<(), String> {
    let Some(actual) = lines.get(index) else {
        return Err(format!(
            "Hunk {kind} expected line {} to be {:?}, but the file ended",
            index + 1,
            expected
        ));
    };
    if actual != expected {
        return Err(format!(
            "Hunk {kind} mismatch at line {}: expected {:?}, found {:?}",
            index + 1,
            expected,
            actual
        ));
    }
    Ok(())
}

#[async_trait]
impl Tool for PatchTool {
    fn name(&self) -> &str {
        "patch"
    }

    fn description(&self) -> &str {
        "Apply a unified diff patch to a file. Use this for complex multi-line edits where the edit tool would be cumbersome. The diff must be in unified diff format with @@ hunk headers."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Required. Path to the file to patch. Always provide this exact field name: 'file_path'."
                },
                "diff": {
                    "type": "string",
                    "description": "Required. Unified diff content with @@ hunk headers."
                }
            },
            "required": ["file_path", "diff"],
            "examples": [
                {
                    "file_path": "src/lib.rs",
                    "diff": "@@ -1,3 +1,3 @@\n line1\n-old_line\n+new_line\n line3"
                }
            ]
        })
    }

    fn capabilities(&self, _args: &serde_json::Value) -> crate::tools::ToolCapabilities {
        let mut capabilities = crate::tools::ToolCapabilities::conservative();
        capabilities.output_kind = crate::tools::ToolOutputKind::Diff;
        capabilities
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let file_path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("file_path parameter is required")),
        };

        let diff = match args.get("diff").and_then(|v| v.as_str()) {
            Some(d) => d,
            None => return Ok(ToolOutput::error("diff parameter is required")),
        };

        let workspace_path = match ctx.resolve_workspace_path(file_path) {
            Ok(path) => path,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to resolve path: {}", e))),
        };
        let display_path = ctx.workspace_services.display_path(&workspace_path);

        let (content, version) = match ctx.workspace_services.read_for_edit(&workspace_path).await {
            Ok(pair) => pair,
            Err(e) => {
                return Ok(ToolOutput::error(format!(
                    "Failed to read file {}: {}",
                    display_path, e
                )))
            }
        };

        let hunks = match parse_hunks(diff) {
            Ok(h) => h,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to parse diff: {}", e))),
        };

        let new_content = match apply_hunks(&content, &hunks) {
            Ok(c) => c,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to apply patch: {}", e))),
        };

        let final_content = new_content;

        match ctx
            .workspace_services
            .write_for_edit(&workspace_path, &final_content, version.as_deref())
            .await
        {
            Ok(_) => {
                let mut metadata = serde_json::Map::new();
                metadata.insert("file_path".to_string(), serde_json::json!(file_path));
                metadata.insert("before".to_string(), serde_json::json!(content));
                metadata.insert("after".to_string(), serde_json::json!(final_content));
                Ok(ToolOutput::success(format!(
                    "Applied {} hunk(s) to {}",
                    hunks.len(),
                    display_path
                ))
                .with_metadata(serde_json::Value::Object(metadata)))
            }
            Err(e) => {
                let typed = crate::tools::ToolErrorKind::from_workspace_error(&e);
                let out = if matches!(e, WorkspaceError::VersionConflict(_)) {
                    ToolOutput::error(format!(
                        "Concurrent modification detected on {}: the file changed between read and write. Re-read the file and retry the patch.",
                        display_path
                    ))
                } else {
                    ToolOutput::error(format!(
                        "Failed to write patched file {}: {}",
                        display_path, e
                    ))
                };
                Ok(match typed {
                    Some(kind) => out.with_error_kind(kind),
                    None => out,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hunk_header() {
        assert_eq!(parse_hunk_header("@@ -1,3 +1,3 @@").unwrap(), (1, 3, 1, 3));
        assert_eq!(
            parse_hunk_header("@@ -10,5 +12,7 @@").unwrap(),
            (10, 5, 12, 7)
        );
        assert_eq!(
            parse_hunk_header("@@ -1 +1 @@ function name").unwrap(),
            (1, 1, 1, 1)
        );
    }

    #[test]
    fn test_parse_hunks_simple() {
        let diff = "@@ -1,3 +1,3 @@\n line1\n-old_line\n+new_line\n line3";
        let hunks = parse_hunks(diff).unwrap();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[0].old_count, 3);
        assert_eq!(hunks[0].new_count, 3);
    }

    #[test]
    fn test_apply_hunks_simple() {
        let content = "line1\nold_line\nline3";
        let diff = "@@ -1,3 +1,3 @@\n line1\n-old_line\n+new_line\n line3";
        let hunks = parse_hunks(diff).unwrap();
        let result = apply_hunks(content, &hunks).unwrap();
        assert_eq!(result, "line1\nnew_line\nline3");
    }

    #[test]
    fn strict_patch_preserves_interleaved_operation_order() {
        let content = "a\nold-one\nmiddle\nold-two\nz\n";
        let diff = "@@ -1,5 +1,5 @@\n a\n-old-one\n+new-one\n middle\n-old-two\n+new-two\n z";
        let hunks = parse_hunks(diff).unwrap();

        let result = apply_hunks(content, &hunks).unwrap();

        assert_eq!(result, "a\nnew-one\nmiddle\nnew-two\nz\n");
    }

    #[test]
    fn strict_patch_rejects_context_and_whitespace_mismatches() {
        let context = parse_hunks("@@ -1,2 +1,2 @@\n wrong\n-old\n+new").unwrap();
        let context_error = apply_hunks("actual\nold\n", &context).unwrap_err();
        assert!(
            context_error.contains("context mismatch"),
            "{context_error}"
        );

        let whitespace = parse_hunks("@@ -1 +1 @@\n-value\n+new").unwrap();
        let whitespace_error = apply_hunks(" value\n", &whitespace).unwrap_err();
        assert!(
            whitespace_error.contains("removal mismatch"),
            "{whitespace_error}"
        );
    }

    #[test]
    fn strict_patch_preserves_crlf_line_endings() {
        let hunks = parse_hunks("@@ -1,2 +1,2 @@\n first\n-second\n+changed").unwrap();

        let result = apply_hunks("first\r\nsecond\r\n", &hunks).unwrap();

        assert_eq!(result, "first\r\nchanged\r\n");
    }

    #[test]
    fn strict_patch_rejects_incorrect_hunk_counts() {
        let error = parse_hunks("@@ -1,2 +1,2 @@\n-old\n+new").unwrap_err();
        assert!(error.contains("Hunk count mismatch"), "{error}");
    }

    #[tokio::test]
    async fn test_patch_tool() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("test.txt"), "line1\nold_line\nline3\n").unwrap();

        let tool = PatchTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "diff": "@@ -1,3 +1,3 @@\n line1\n-old_line\n+new_line\n line3"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        let metadata = result.metadata.expect("patch diff metadata");
        assert_eq!(metadata["file_path"], "test.txt");
        assert_eq!(metadata["before"], "line1\nold_line\nline3\n");
        assert_eq!(metadata["after"], "line1\nnew_line\nline3\n");
        let content = std::fs::read_to_string(temp.path().join("test.txt")).unwrap();
        assert!(content.contains("new_line"));
        assert!(!content.contains("old_line"));
    }

    #[test]
    fn test_parse_hunks_no_header() {
        let diff = "just some text\nwithout hunks";
        assert!(parse_hunks(diff).is_err());
    }

    #[tokio::test]
    async fn test_patch_missing_params() {
        let tool = PatchTool;
        let ctx = ToolContext::new(std::path::PathBuf::from("/tmp"));

        let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();
        assert!(!result.success);
    }

    #[test]
    fn test_patch_schema_is_canonical() {
        let tool = PatchTool;
        let params = tool.parameters();
        assert_eq!(params["additionalProperties"], false);
        assert_eq!(params["required"], serde_json::json!(["file_path", "diff"]));
        let examples = params["examples"].as_array().unwrap();
        assert_eq!(examples[0]["file_path"], "src/lib.rs");
        assert!(examples[0].get("path").is_none());
    }
}
