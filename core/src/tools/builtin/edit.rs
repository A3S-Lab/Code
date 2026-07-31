//! Edit tool - Edit files by string replacement

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::workspace::WorkspaceError;
use anyhow::Result;
use async_trait::async_trait;

pub struct EditTool;

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing a specific string with another. The old_string must be unique in the file unless replace_all is true."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Required. Path to the file to edit. Always provide this exact field name: 'file_path'."
                },
                "old_string": {
                    "type": "string",
                    "description": "Required. The exact string to replace. It must be unique unless replace_all=true."
                },
                "new_string": {
                    "type": "string",
                    "description": "Required. The replacement string."
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Optional. Replace all occurrences. Default: false."
                },
                "dry_run": {
                    "type": "boolean",
                    "description": "Optional. Preview the exact diff and replacement count without writing. Default: false."
                },
                "max_replacements": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional safety ceiling. Reject the edit before writing when it would replace more occurrences."
                },
                "expected_replacements": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional compare guard. Reject the edit before writing unless the observed replacement count is exactly this value."
                }
            },
            "required": ["file_path", "old_string", "new_string"],
            "examples": [
                {
                    "file_path": "src/lib.rs",
                    "old_string": "old_value",
                    "new_string": "new_value"
                },
                {
                    "file_path": "src/lib.rs",
                    "old_string": "foo",
                    "new_string": "bar",
                    "replace_all": true,
                    "dry_run": true,
                    "max_replacements": 20
                }
            ]
        })
    }

    fn capabilities(&self, args: &serde_json::Value) -> crate::tools::ToolCapabilities {
        let mut capabilities = if args
            .get("dry_run")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            crate::tools::ToolCapabilities::parallel_safe_read(16)
        } else {
            crate::tools::ToolCapabilities::conservative()
        };
        capabilities.output_kind = crate::tools::ToolOutputKind::Diff;
        capabilities
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let file_path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("file_path parameter is required")),
        };

        let old_string = match args.get("old_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return Ok(ToolOutput::error("old_string parameter is required")),
        };

        let new_string = match args.get("new_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return Ok(ToolOutput::error("new_string parameter is required")),
        };
        if old_string.is_empty() {
            return Ok(ToolOutput::error("old_string must not be empty"));
        }

        let replace_all = args
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let max_replacements = match positive_usize_arg(args, "max_replacements") {
            Ok(value) => value,
            Err(error) => return Ok(ToolOutput::error(error)),
        };
        let expected_replacements = match positive_usize_arg(args, "expected_replacements") {
            Ok(value) => value,
            Err(error) => return Ok(ToolOutput::error(error)),
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

        let count = content.matches(old_string).count();

        if count == 0 {
            return Ok(ToolOutput::error(format!(
                "old_string not found in {}",
                display_path
            )));
        }

        if count > 1 && !replace_all {
            return Ok(ToolOutput::error(format!(
                "old_string found {} times in {}. Use replace_all=true to replace all occurrences, or provide a more specific string.",
                count,
                display_path
            )));
        }

        let replacement_count = if replace_all { count } else { 1 };
        if let Some(expected) = expected_replacements {
            if replacement_count != expected {
                return Ok(ToolOutput::error(format!(
                    "Replacement count mismatch in {display_path}: expected {expected} replacement(s), found {replacement_count}. Re-run dry_run to inspect the current file."
                ))
                .with_metadata(serde_json::json!({
                    "file_path": file_path,
                    "dry_run": dry_run,
                    "expected_replacements": expected,
                    "replacement_count": replacement_count,
                })));
            }
        }
        if let Some(maximum) = max_replacements {
            if replacement_count > maximum {
                return Ok(ToolOutput::error(format!(
                    "Edit would replace {replacement_count} occurrence(s) in {display_path}, which exceeds max_replacements={maximum}."
                ))
                .with_metadata(serde_json::json!({
                    "file_path": file_path,
                    "dry_run": dry_run,
                    "max_replacements": maximum,
                    "replacement_count": replacement_count,
                })));
            }
        }

        let new_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };

        let change_metadata = || {
            serde_json::json!({
                "file_path": file_path,
                "before": content,
                "after": new_content,
                "dry_run": dry_run,
                "replacement_count": replacement_count,
                "expected_replacements": expected_replacements,
                "max_replacements": max_replacements,
            })
        };
        if dry_run {
            return Ok(ToolOutput::success(format!(
                "Dry run: would replace {replacement_count} occurrence(s) in {display_path}; nothing was written"
            ))
            .with_metadata(change_metadata()));
        }

        match ctx
            .workspace_services
            .write_for_edit(&workspace_path, &new_content, version.as_deref())
            .await
        {
            Ok(_) => Ok(ToolOutput::success(format!(
                "Replaced {} occurrence(s) in {}",
                replacement_count, display_path
            ))
            .with_metadata(change_metadata())),
            Err(e) => {
                // Surface the typed kind via ToolOutput.error_kind so SDK
                // callers can react programmatically; the human-readable
                // `content` message stays the same so the model sees the
                // retry hint.
                let typed = crate::tools::ToolErrorKind::from_workspace_error(&e);
                let out = if matches!(e, WorkspaceError::VersionConflict(_)) {
                    ToolOutput::error(format!(
                        "Concurrent modification detected on {}: the file changed between read and write. Re-read the file and retry the edit.",
                        display_path
                    ))
                } else {
                    ToolOutput::error(format!("Failed to write file {}: {}", display_path, e))
                };
                Ok(match typed {
                    Some(kind) => out.with_error_kind(kind),
                    None => out,
                })
            }
        }
    }
}

fn positive_usize_arg(
    args: &serde_json::Value,
    name: &str,
) -> std::result::Result<Option<usize>, String> {
    match args.get(name) {
        Some(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value > 0)
            .map(Some)
            .ok_or_else(|| format!("{name} must be a positive integer")),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_edit_single_replace() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("test.txt"), "hello world").unwrap();

        let tool = EditTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "old_string": "hello",
                    "new_string": "goodbye"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        let content = std::fs::read_to_string(temp.path().join("test.txt")).unwrap();
        assert_eq!(content, "goodbye world");
    }

    #[tokio::test]
    async fn test_edit_replace_all() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("test.txt"), "aaa bbb aaa").unwrap();

        let tool = EditTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "old_string": "aaa",
                    "new_string": "ccc",
                    "replace_all": true
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        let content = std::fs::read_to_string(temp.path().join("test.txt")).unwrap();
        assert_eq!(content, "ccc bbb ccc");
    }

    #[tokio::test]
    async fn test_edit_dry_run_returns_diff_without_writing() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("test.txt");
        std::fs::write(&path, "alpha alpha").unwrap();
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = EditTool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "old_string": "alpha",
                    "new_string": "beta",
                    "replace_all": true,
                    "dry_run": true
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success, "{}", result.content);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "alpha alpha");
        let metadata = result.metadata.unwrap();
        assert_eq!(metadata["dry_run"], true);
        assert_eq!(metadata["replacement_count"], 2);
        assert_eq!(metadata["before"], "alpha alpha");
        assert_eq!(metadata["after"], "beta beta");
        assert!(
            EditTool
                .capabilities(&serde_json::json!({"dry_run": true}))
                .read_only
        );
    }

    #[tokio::test]
    async fn test_edit_expected_replacements_mismatch_does_not_write() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("test.txt");
        std::fs::write(&path, "alpha alpha").unwrap();
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = EditTool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "old_string": "alpha",
                    "new_string": "beta",
                    "replace_all": true,
                    "expected_replacements": 3
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result
            .content
            .contains("expected 3 replacement(s), found 2"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "alpha alpha");
    }

    #[tokio::test]
    async fn test_edit_max_replacements_blocks_oversized_change() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("test.txt");
        std::fs::write(&path, "alpha alpha alpha").unwrap();
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = EditTool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "old_string": "alpha",
                    "new_string": "beta",
                    "replace_all": true,
                    "max_replacements": 2
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.content.contains("exceeds max_replacements=2"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "alpha alpha alpha");
    }

    #[tokio::test]
    async fn test_edit_not_unique() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("test.txt"), "aaa bbb aaa").unwrap();

        let tool = EditTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "old_string": "aaa",
                    "new_string": "ccc"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.content.contains("2 times"));
    }

    #[tokio::test]
    async fn test_edit_not_found() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("test.txt"), "hello world").unwrap();

        let tool = EditTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "old_string": "xyz",
                    "new_string": "abc"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.content.contains("not found"));
    }

    #[test]
    fn test_edit_schema_is_canonical() {
        let tool = EditTool;
        let params = tool.parameters();
        assert_eq!(params["additionalProperties"], false);
        assert_eq!(
            params["required"],
            serde_json::json!(["file_path", "old_string", "new_string"])
        );
        let examples = params["examples"].as_array().unwrap();
        assert_eq!(examples[0]["file_path"], "src/lib.rs");
        assert!(examples[0].get("path").is_none());
        assert_eq!(params["properties"]["dry_run"]["type"], "boolean");
        assert_eq!(
            params["properties"]["max_replacements"]["minimum"],
            serde_json::json!(1)
        );
        assert_eq!(
            params["properties"]["expected_replacements"]["minimum"],
            serde_json::json!(1)
        );
    }

    #[tokio::test]
    async fn test_edit_surfaces_concurrent_modification_as_typed_error() {
        // Mock backend whose write step always reports a version conflict —
        // simulating an S3 If-Match 412 between the read and the write.
        // Verifies that:
        //  (1) edit matches on WorkspaceError::VersionConflict directly,
        //  (2) the user-facing message includes "Concurrent modification"
        //      (so the model can retry) rather than the generic write error.
        use crate::workspace::{
            WorkspaceDirEntry, WorkspaceFileSystem, WorkspaceFileSystemExt, WorkspacePath,
            WorkspaceRef, WorkspaceResult, WorkspaceServices, WorkspaceVersionConflict,
            WorkspaceWriteOutcome,
        };
        use async_trait::async_trait;
        use std::sync::Arc;

        struct AlwaysConflictFs;

        #[async_trait]
        impl WorkspaceFileSystem for AlwaysConflictFs {
            async fn read_text(&self, _path: &WorkspacePath) -> WorkspaceResult<String> {
                Ok("hello world".to_string())
            }
            async fn write_text(
                &self,
                _path: &WorkspacePath,
                content: &str,
            ) -> WorkspaceResult<WorkspaceWriteOutcome> {
                Ok(WorkspaceWriteOutcome {
                    bytes: content.len(),
                    lines: content.lines().count(),
                })
            }
            async fn list_dir(
                &self,
                _path: &WorkspacePath,
            ) -> WorkspaceResult<Vec<WorkspaceDirEntry>> {
                Ok(Vec::new())
            }
        }

        #[async_trait]
        impl WorkspaceFileSystemExt for AlwaysConflictFs {
            async fn read_text_with_version(
                &self,
                _path: &WorkspacePath,
            ) -> WorkspaceResult<(String, String)> {
                Ok(("hello world".to_string(), "v0".to_string()))
            }
            async fn write_text_if_version(
                &self,
                path: &WorkspacePath,
                _content: &str,
                _expected_version: &str,
            ) -> WorkspaceResult<WorkspaceWriteOutcome> {
                Err(WorkspaceError::VersionConflict(WorkspaceVersionConflict {
                    path: path.as_str().to_string(),
                    expected: "v0".to_string(),
                    actual: Some("v-other".to_string()),
                }))
            }
        }

        let backend = Arc::new(AlwaysConflictFs);
        let fs: Arc<dyn WorkspaceFileSystem> = backend.clone();
        let fs_ext: Arc<dyn WorkspaceFileSystemExt> = backend;
        let services = WorkspaceServices::builder(WorkspaceRef::new("mem", "mem://ws"), fs)
            .file_system_ext(fs_ext)
            .build();

        let tool = EditTool;
        let ctx = ToolContext::new(std::env::temp_dir()).with_workspace_services(services);

        let result = tool
            .execute(
                &serde_json::json!({
                    "file_path": "anything.txt",
                    "old_string": "hello",
                    "new_string": "goodbye",
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(
            !result.success,
            "edit on conflicting backend must report failure"
        );
        assert!(
            result.content.contains("Concurrent modification"),
            "expected retry-friendly conflict message, got: {}",
            result.content
        );

        // Phase 8: the typed error_kind must also survive end-to-end so SDK
        // callers can branch on it without parsing the string.
        let kind = result
            .error_kind
            .as_ref()
            .expect("edit must surface a typed error_kind for VersionConflict");
        match kind {
            crate::tools::ToolErrorKind::VersionConflict {
                path,
                expected,
                actual,
            } => {
                assert_eq!(path, "anything.txt");
                assert_eq!(expected, "v0");
                assert_eq!(actual.as_deref(), Some("v-other"));
            }
            other => panic!("expected VersionConflict kind, got {other:?}"),
        }

        // The serialised wire shape is the contract SDKs will see. Pin it
        // so any accidental rename / restructure breaks the build.
        let json = serde_json::to_value(kind).unwrap();
        assert_eq!(json["type"], "version_conflict");
        assert_eq!(json["path"], "anything.txt");
        assert_eq!(json["expected"], "v0");
        assert_eq!(json["actual"], "v-other");
    }
}
