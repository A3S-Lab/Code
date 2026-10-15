//! Patch tool - Apply unified diff patches to files

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::workspace::WorkspaceError;
use anyhow::Result;
use async_trait::async_trait;

/// Upper bound on files in one `patches` batch: a coordinated multi-file
/// edit is a handful of files, not a whole-tree rewrite.
const MAX_PATCH_ENTRIES: usize = 64;

/// Upper bound on a batch's total staged content (the before side of every
/// entry summed). It mirrors the single-file read bound — a batch may
/// inspect at most as much content as one max-size file — so staged memory
/// stays bounded no matter the entry count; without it, N entries of
/// max-size files stage `before`+`after` without limit.
const MAX_PATCH_BATCH_BYTES: usize = 16 * 1024 * 1024;

pub struct PatchTool;

struct StagedPatch {
    file_path: String,
    display_path: String,
    workspace_path: crate::workspace::WorkspacePath,
    before: String,
    after: String,
    version: Option<String>,
}

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
        "Apply a unified diff patch to one file (file_path + diff), or several files in ONE \
         all-or-nothing call (patches: [{file_path, diff}, ...] — every diff is validated \
         in memory before any write, so a bad hunk anywhere changes nothing). Prefer the \
         patches form when editing two or more files in the same step."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Single-file mode: path to the file to patch. Use exactly this field name."
                },
                "diff": {
                    "type": "string",
                    "description": "Unified diff content with @@ hunk headers for file_path."
                },
                "patches": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 64,
                    "description": "Multi-file mode: one entry per file, applied all-or-nothing. \
                     Takes precedence over file_path/diff.",
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "file_path": {
                                "type": "string",
                                "description": "Path to the file this entry patches."
                            },
                            "diff": {
                                "type": "string",
                                "description": "Unified diff with @@ hunk headers for this file only."
                            }
                        },
                        "required": ["file_path", "diff"]
                    }
                }
            },
            "examples": [
                {
                    "file_path": "src/lib.rs",
                    "diff": "@@ -1,3 +1,3 @@\n line1\n-old_line\n+new_line\n line3"
                },
                {
                    "patches": [
                        {"file_path": "a.txt", "diff": "@@ -1 +1 @@\n-old\n+new"},
                        {"file_path": "b.txt", "diff": "@@ -1 +1 @@\n-old\n+new"}
                    ]
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
        // Multi-file mode: `patches: [{file_path, diff}, ...]`. Validation is
        // all-or-nothing — every file is read and every diff applied in
        // memory before ANY write, so a bad hunk anywhere changes nothing.
        // The write phase is sequential CAS; on a mid-sequence failure the
        // output names exactly which paths wrote (the workspace backend has
        // no cross-file transaction, and pretending otherwise would hide
        // partial state).
        if let Some(entries) = args.get("patches").and_then(|v| v.as_array()) {
            return self.execute_patches(entries, ctx).await;
        }
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
        let session_id = ctx.session_id.as_deref().filter(|id| !id.trim().is_empty());
        let newly_acquired = session_id.is_some_and(|id| {
            !crate::external_observation::session_owns_write(id, &ctx.workspace, file_path)
        });
        if let Err(error) = crate::external_observation::claim_bound_write(
            ctx.session_id.as_deref(),
            &ctx.workspace,
            file_path,
        ) {
            return Ok(ToolOutput::error(error));
        }

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
                if newly_acquired {
                    if let Some(id) = session_id {
                        crate::external_observation::release_write_claim(
                            id,
                            &ctx.workspace,
                            file_path,
                        );
                    }
                }
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

impl PatchTool {
    async fn execute_patches(
        &self,
        entries: &[serde_json::Value],
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        if entries.is_empty() {
            return Ok(ToolOutput::error("patches must contain at least one entry"));
        }
        if entries.len() > MAX_PATCH_ENTRIES {
            return Ok(ToolOutput::error(format!(
                "patches accepts at most {MAX_PATCH_ENTRIES} entries per call (got {}); \
                 split the batch",
                entries.len()
            )));
        }
        // Phase 1 — validate everything in memory. The per-entry chain
        // (resolve → read → parse diff → apply) is prefetched
        // CONCURRENTLY: entries are independent up to the write phase,
        // and awaiting each chain one by one made the batch's wall clock
        // the sum of per-file reads (each up to the text bound). The
        // assembly below walks the results in ENTRY ORDER, so the
        // first-error-wins semantics, the byte bound, and every error
        // message are exactly the sequential loop's — only unobservable
        // extra I/O on later entries after an early failure differs.
        enum PatchFetch {
            MissingFilePath,
            MissingDiff {
                file_path: String,
            },
            ResolveErr {
                file_path: String,
                error: String,
            },
            ReadErr {
                display_path: String,
                error: String,
            },
            ParseErr {
                display_path: String,
                error: String,
            },
            ApplyErr {
                display_path: String,
                error: String,
            },
            Ready {
                file_path: String,
                display_path: String,
                workspace_path: crate::workspace::WorkspacePath,
                before: String,
                after: String,
                version: Option<String>,
            },
        }
        let fetched: Vec<PatchFetch> =
            futures::future::join_all(entries.iter().map(|entry| async {
                let Some(file_path) = entry.get("file_path").and_then(|v| v.as_str()) else {
                    return PatchFetch::MissingFilePath;
                };
                let Some(diff) = entry.get("diff").and_then(|v| v.as_str()) else {
                    return PatchFetch::MissingDiff {
                        file_path: file_path.to_string(),
                    };
                };
                let workspace_path = match ctx.resolve_workspace_path(file_path) {
                    Ok(path) => path,
                    Err(e) => {
                        return PatchFetch::ResolveErr {
                            file_path: file_path.to_string(),
                            error: e.to_string(),
                        }
                    }
                };
                let display_path = ctx.workspace_services.display_path(&workspace_path);
                let (before, version) =
                    match ctx.workspace_services.read_for_edit(&workspace_path).await {
                        Ok(pair) => pair,
                        Err(e) => {
                            return PatchFetch::ReadErr {
                                display_path,
                                error: e.to_string(),
                            }
                        }
                    };
                let hunks = match parse_hunks(diff) {
                    Ok(hunks) => hunks,
                    Err(e) => {
                        return PatchFetch::ParseErr {
                            display_path,
                            error: e.to_string(),
                        }
                    }
                };
                let after = match apply_hunks(&before, &hunks) {
                    Ok(content) => content,
                    Err(e) => {
                        return PatchFetch::ApplyErr {
                            display_path,
                            error: e.to_string(),
                        }
                    }
                };
                PatchFetch::Ready {
                    file_path: file_path.to_string(),
                    display_path,
                    workspace_path,
                    before,
                    after,
                    version,
                }
            }))
            .await;

        let mut staged: Vec<StagedPatch> = Vec::with_capacity(entries.len());
        let mut staged_bytes = 0usize;
        for fetch in fetched {
            match fetch {
                PatchFetch::MissingFilePath => {
                    return Ok(ToolOutput::error("every patches entry requires file_path"));
                }
                PatchFetch::MissingDiff { file_path } => {
                    return Ok(ToolOutput::error(format!(
                        "patches entry for {file_path} requires diff"
                    )));
                }
                PatchFetch::ResolveErr { file_path, error } => {
                    return Ok(ToolOutput::error(format!(
                        "Failed to resolve path {file_path}: {error}"
                    )));
                }
                PatchFetch::ReadErr {
                    display_path,
                    error,
                } => {
                    return Ok(ToolOutput::error(format!(
                        "Failed to read {display_path}: {error} (nothing written)"
                    )));
                }
                PatchFetch::ParseErr {
                    display_path,
                    error,
                } => {
                    return Ok(ToolOutput::error(format!(
                        "Failed to parse diff for {display_path}: {error} (nothing written)"
                    )));
                }
                PatchFetch::ApplyErr {
                    display_path,
                    error,
                } => {
                    return Ok(ToolOutput::error(format!(
                        "Failed to apply patch to {display_path}: {error} (nothing written)"
                    )));
                }
                PatchFetch::Ready {
                    file_path,
                    display_path,
                    workspace_path,
                    before,
                    after,
                    version,
                } => {
                    staged_bytes += before.len();
                    if staged_bytes > MAX_PATCH_BATCH_BYTES {
                        return Ok(ToolOutput::error(format!(
                            "patches would stage {staged_bytes} bytes of file content across the \
                             batch (bound {MAX_PATCH_BATCH_BYTES}); split the batch (nothing written)"
                        )));
                    }
                    staged.push(StagedPatch {
                        file_path,
                        display_path,
                        workspace_path,
                        before,
                        after,
                        version,
                    });
                }
            }
        }
        // Phase 2 — sequential writes with precise partial-state reporting.
        for patch in &staged {
            if let Err(error) = crate::external_observation::claim_bound_write(
                ctx.session_id.as_deref(),
                &ctx.workspace,
                &patch.file_path,
            ) {
                return Ok(ToolOutput::error(error));
            }
        }
        let mut written: Vec<&StagedPatch> = Vec::new();
        for patch in &staged {
            match ctx
                .workspace_services
                .write_for_edit(
                    &patch.workspace_path,
                    &patch.after,
                    patch.version.as_deref(),
                )
                .await
            {
                Ok(_) => written.push(patch),
                Err(e) => {
                    let typed = crate::tools::ToolErrorKind::from_workspace_error(&e);
                    let applied: Vec<&str> = written.iter().map(|p| p.file_path.as_str()).collect();
                    let out = ToolOutput::error(format!(
                        "Multi-file patch stopped at {}: {e}. Written before the failure:                          [{}]. Unwritten: [{}]. Re-read the failed file and re-apply the                          remainder.",
                        patch.display_path,
                        applied.join(", "),
                        staged[written.len() + 1..]
                            .iter()
                            .map(|p| p.file_path.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                    ));
                    return Ok(match typed {
                        Some(kind) => out.with_error_kind(kind),
                        None => out,
                    });
                }
            }
        }
        let mut metadata = serde_json::Map::new();
        metadata.insert(
            "files".to_string(),
            serde_json::json!(staged
                .iter()
                .map(|p| serde_json::json!({
                    "file_path": p.file_path,
                    "before": p.before,
                    "after": p.after,
                }))
                .collect::<Vec<_>>()),
        );
        let hunks_total: usize = entries.len();
        Ok(
            ToolOutput::success(format!("Applied multi-file patch: {hunks_total} file(s)"))
                .with_metadata(serde_json::Value::Object(metadata)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_advertises_the_multi_file_patches_mode() {
        // The mode was once unreachable: execute understood `patches`
        // while the schema declared only file_path/diff with
        // additionalProperties false — a conforming model could never
        // discover the shape (the PAR-PATCH live rail caught the model
        // retrying multi-file diffs into the single-file mode). This
        // pin keeps the advertised surface and the dispatched surface
        // from drifting apart again.
        let parameters = PatchTool.parameters();
        let properties = parameters["properties"].as_object().unwrap();
        assert!(
            properties.contains_key("patches"),
            "the patches array is advertised"
        );
        let items = properties["patches"]["items"].as_object().unwrap();
        let item_properties = items["properties"].as_object().unwrap();
        assert!(item_properties.contains_key("file_path") && item_properties.contains_key("diff"));
        assert_eq!(items["required"], serde_json::json!(["file_path", "diff"]));
        assert_eq!(properties["patches"]["maxItems"], serde_json::json!(64));
        assert!(
            !parameters["additionalProperties"].as_bool().unwrap_or(true),
            "the closed surface stays closed"
        );
    }

    use crate::tools::types::ToolContext as PatchToolContext;
    use crate::workspace::{
        WorkspaceDirEntry, WorkspaceFileSystem, WorkspacePath, WorkspaceRef, WorkspaceResult,
        WorkspaceServices, WorkspaceWriteOutcome,
    };

    /// Writes pass through; reads carry an artificial latency — a
    /// deterministic overlap oracle for the concurrent patch staging
    /// prefetch (the write phase stays sequential by design).
    struct SlowReadBackend {
        latency: std::time::Duration,
    }

    #[async_trait]
    impl WorkspaceFileSystem for SlowReadBackend {
        async fn read_text(&self, _path: &WorkspacePath) -> WorkspaceResult<String> {
            tokio::time::sleep(self.latency).await;
            Ok("alpha-1\nalpha-2\n".to_string())
        }

        async fn write_text(
            &self,
            _path: &WorkspacePath,
            _content: &str,
        ) -> WorkspaceResult<WorkspaceWriteOutcome> {
            Ok(WorkspaceWriteOutcome { bytes: 0, lines: 0 })
        }

        async fn list_dir(&self, _path: &WorkspacePath) -> WorkspaceResult<Vec<WorkspaceDirEntry>> {
            Ok(Vec::new())
        }
    }

    /// Three staged reads of 60 ms must overlap: the whole batch lands
    /// well under their 180 ms sequential sum. The 150 ms bound sits at
    /// 2.5x the concurrent floor — wide margin, sleeps only, no spawns.
    #[tokio::test]
    async fn patch_staging_prefetch_overlaps_per_file_latency() {
        let backend = std::sync::Arc::new(SlowReadBackend {
            latency: std::time::Duration::from_millis(60),
        });
        let fs: std::sync::Arc<dyn WorkspaceFileSystem> = backend;
        let services =
            WorkspaceServices::builder(WorkspaceRef::new("slow", "slow://ws"), fs).build();
        let ctx = PatchToolContext::new(std::env::temp_dir())
            .with_workspace_services(services)
            .with_session_id("patch-overlap");
        let diff = "@@ -1,2 +1,2 @@\n alpha-1\n-alpha-2\n+alpha-two";

        let started = std::time::Instant::now();
        let result = PatchTool
            .execute(
                &serde_json::json!({
                    "patches": [
                        {"file_path": "a.txt", "diff": diff},
                        {"file_path": "b.txt", "diff": diff},
                        {"file_path": "c.txt", "diff": diff}
                    ]
                }),
                &ctx,
            )
            .await
            .unwrap();
        let elapsed = started.elapsed();
        assert!(result.success, "the batch must succeed: {}", result.content);
        assert!(
            elapsed < std::time::Duration::from_millis(150),
            "three 60ms staged reads took {elapsed:?} — the prefetch is not overlapping"
        );
    }

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
        let ctx = ToolContext::new(temp.path().to_path_buf()).with_session_id("patch-test");

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
        crate::external_observation::release_session("patch-test");
        let metadata = result.metadata.expect("patch diff metadata");
        assert_eq!(metadata["file_path"], "test.txt");
        assert_eq!(metadata["before"], "line1\nold_line\nline3\n");
        assert_eq!(metadata["after"], "line1\nnew_line\nline3\n");
        let content = std::fs::read_to_string(temp.path().join("test.txt")).unwrap();
        assert!(content.contains("new_line"));
        assert!(!content.contains("old_line"));
    }

    #[tokio::test]
    async fn patch_without_a_session_does_not_apply_or_claim_an_owner() {
        let temp = tempfile::tempdir().unwrap();
        let original = "line1\nold_line\nline3\n";
        std::fs::write(temp.path().join("test.txt"), original).unwrap();
        let tool = PatchTool;
        let unbound = ToolContext::new(temp.path().to_path_buf());
        let diff = "@@ -1,3 +1,3 @@\n line1\n-old_line\n+new_line\n line3";

        for session_id in [None, Some("   ")] {
            let ctx = match session_id {
                Some(id) => unbound.clone().with_session_id(id),
                None => unbound.clone(),
            };
            let result = tool
                .execute(
                    &serde_json::json!({
                        "file_path": "test.txt",
                        "diff": diff
                    }),
                    &ctx,
                )
                .await
                .unwrap();
            assert!(!result.success, "{session_id:?}");
            assert_eq!(
                std::fs::read_to_string(temp.path().join("test.txt")).unwrap(),
                original,
                "{session_id:?}"
            );
        }

        let applied = tool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "diff": diff
                }),
                &unbound.with_session_id("patch-after-unbound"),
            )
            .await
            .unwrap();
        assert!(applied.success, "{}", applied.content);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("test.txt")).unwrap(),
            "line1\nnew_line\nline3\n"
        );
        crate::external_observation::release_session("patch-after-unbound");
    }

    #[tokio::test]
    async fn rejected_hunk_does_not_write_or_open_the_ledger() {
        let temp = tempfile::tempdir().unwrap();
        let original = "line1\nold_line\nline3\n";
        std::fs::write(temp.path().join("test.txt"), original).unwrap();
        let tool = PatchTool;
        let ctx = ToolContext::new(temp.path().to_path_buf()).with_session_id("patch-reject");

        let result = tool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "diff": "@@ -1,3 +1,3 @@\n wrong\n-old_line\n+new_line\n line3"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.success, "{result:?}");
        assert_eq!(
            std::fs::read_to_string(temp.path().join("test.txt")).unwrap(),
            original
        );
        let mut ledger = crate::harness_loop::MutationLedger::default();
        ledger.observe_tool("patch", 1, result.metadata.as_ref());
        assert!(
            ledger.is_empty(),
            "a rejected hunk opened the completion gate: {}",
            ledger.digest()
        );
    }

    #[test]
    fn test_parse_hunks_no_header() {
        let diff = "just some text\nwithout hunks";
        assert!(parse_hunks(diff).is_err());
    }

    #[tokio::test]
    async fn test_patch_missing_params() {
        let tool = PatchTool;
        let ctx = ToolContext::new(std::path::PathBuf::from("/tmp")).with_session_id("patch-test");

        let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn rejected_patch_does_not_own_a_file_that_was_not_written() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("guest.txt"), "keep").unwrap();
        let tool = PatchTool;
        let ctx = ToolContext::new(temp.path().to_path_buf()).with_session_id("patch-reject-owner");
        let result = tool
            .execute(&serde_json::json!({"file_path": "guest.txt"}), &ctx)
            .await
            .unwrap();
        assert!(!result.success);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("guest.txt")).unwrap(),
            "keep"
        );
        assert!(
            crate::external_observation::claim_bound_write(
                Some("patch-reject-other"),
                temp.path(),
                "guest.txt",
            )
            .is_ok(),
            "a rejected patch must not own a file it did not write"
        );
        crate::external_observation::release_session("patch-reject-owner");
        crate::external_observation::release_session("patch-reject-other");
    }

    #[test]
    fn test_patch_schema_is_canonical() {
        let tool = PatchTool;
        let params = tool.parameters();
        assert_eq!(params["additionalProperties"], false);
        // Two modes, either/or: single-file (file_path+diff) or the
        // multi-file patches array — so the top level carries NO
        // required list; each patches ITEM requires its own pair.
        assert!(params.get("required").is_none());
        let examples = params["examples"].as_array().unwrap();
        assert_eq!(examples[0]["file_path"], "src/lib.rs");
        assert!(examples[0].get("path").is_none());
        assert!(
            examples[1]["patches"].is_array(),
            "the multi-file example is present"
        );
    }

    #[tokio::test]
    async fn multi_file_patch_is_all_or_nothing_on_validation() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("a.txt"), "alpha-1\nalpha-2\n").unwrap();
        std::fs::write(temp.path().join("b.txt"), "beta-1\nbeta-2\n").unwrap();

        let tool = PatchTool;
        let ctx = ToolContext::new(temp.path().to_path_buf()).with_session_id("patch-multi");

        // The second entry's hunk does not match — nothing may change.
        let result = tool
            .execute(
                &serde_json::json!({
                    "patches": [
                        {"file_path": "a.txt", "diff": "@@ -1,2 +1,2 @@\n alpha-1\n-alpha-2\n+alpha-two"},
                        {"file_path": "b.txt", "diff": "@@ -1,2 +1,2 @@\n beta-1\n-no-such-line\n+beta-two"}
                    ]
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!result.success, "the mismatched entry must fail the batch");
        assert!(
            result.content.contains("nothing written"),
            "the failure must name atomicity: {}",
            result.content
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("a.txt")).unwrap(),
            "alpha-1\nalpha-2\n",
            "the GOOD entry must not be written when another fails validation"
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("b.txt")).unwrap(),
            "beta-1\nbeta-2\n"
        );
        crate::external_observation::release_session("patch-multi");
    }

    #[tokio::test]
    async fn patch_batch_refuses_more_than_the_entry_cap() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("a.txt"), "alpha-1\nalpha-2\n").unwrap();
        let tool = PatchTool;
        let ctx = ToolContext::new(temp.path().to_path_buf()).with_session_id("patch-cap");
        let diff = "@@ -1,2 +1,2 @@\n alpha-1\n-alpha-2\n+alpha-two";
        let entries: Vec<_> = (0..=MAX_PATCH_ENTRIES)
            .map(|_| serde_json::json!({"file_path": "a.txt", "diff": diff}))
            .collect();

        let result = tool
            .execute(&serde_json::json!({ "patches": entries }), &ctx)
            .await
            .unwrap();
        assert!(!result.success, "the entry cap must refuse the batch");
        assert!(
            result.content.contains("at most")
                && result.content.contains(&MAX_PATCH_ENTRIES.to_string()),
            "the refusal names the bound: {}",
            result.content
        );
        // The cap fires before any staging: the file is untouched.
        assert_eq!(
            std::fs::read_to_string(temp.path().join("a.txt")).unwrap(),
            "alpha-1\nalpha-2\n"
        );
        crate::external_observation::release_session("patch-cap");
    }

    #[tokio::test]
    async fn patch_batch_refuses_past_the_staged_byte_bound() {
        let temp = tempfile::tempdir().unwrap();
        // Two ~14 MiB files: each alone fits the per-file read bound, but
        // the second entry pushes the batch's staged before-side past the
        // 16 MiB bound mid-staging — the refusal must land before any
        // write, keeping the batch atomic.
        let filler = "filler\n".repeat(2 * 1024 * 1024); // ~14 MiB
        let body = format!("alpha-1\nalpha-2\n{filler}");
        std::fs::write(temp.path().join("big-a.txt"), &body).unwrap();
        std::fs::write(temp.path().join("big-b.txt"), &body).unwrap();
        let tool = PatchTool;
        let ctx = ToolContext::new(temp.path().to_path_buf()).with_session_id("patch-bytes");
        let diff = "@@ -1,2 +1,2 @@\n alpha-1\n-alpha-2\n+alpha-two";

        let result = tool
            .execute(
                &serde_json::json!({
                    "patches": [
                        {"file_path": "big-a.txt", "diff": diff},
                        {"file_path": "big-b.txt", "diff": diff}
                    ]
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(
            !result.success,
            "the staged-byte bound must refuse the batch"
        );
        assert!(
            result.content.contains("nothing written")
                && result.content.contains(&MAX_PATCH_BATCH_BYTES.to_string()),
            "the refusal names the bound and atomicity: {}",
            result.content
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("big-a.txt")).unwrap(),
            body,
            "the staged-but-refused entry must not be written"
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("big-b.txt")).unwrap(),
            body
        );
        crate::external_observation::release_session("patch-bytes");
    }

    #[tokio::test]
    async fn multi_file_patch_applies_every_file() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("a.txt"), "alpha-1\nalpha-2\n").unwrap();
        std::fs::write(temp.path().join("b.txt"), "beta-1\nbeta-2\n").unwrap();

        let tool = PatchTool;
        let ctx = ToolContext::new(temp.path().to_path_buf()).with_session_id("patch-multi-ok");

        let result = tool
            .execute(
                &serde_json::json!({
                    "patches": [
                        {"file_path": "a.txt", "diff": "@@ -1,2 +1,2 @@\n alpha-1\n-alpha-2\n+alpha-two"},
                        {"file_path": "b.txt", "diff": "@@ -1,2 +1,2 @@\n beta-1\n-beta-2\n+beta-two"}
                    ]
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.success, "output: {}", result.content);
        assert!(std::fs::read_to_string(temp.path().join("a.txt"))
            .unwrap()
            .contains("alpha-two"));
        assert!(std::fs::read_to_string(temp.path().join("b.txt"))
            .unwrap()
            .contains("beta-two"));
        let metadata = result.metadata.expect("multi-file metadata");
        assert_eq!(metadata["files"].as_array().unwrap().len(), 2);
        crate::external_observation::release_session("patch-multi-ok");
    }
}
