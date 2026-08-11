use super::*;
use crate::workspace::{
    CommandOutput, CommandRequest, WorkspaceCommandRunner, WorkspaceDirEntry, WorkspaceError,
    WorkspaceFileSystem, WorkspaceFileType, WorkspacePath, WorkspaceRef, WorkspaceResult,
    WorkspaceServices, WorkspaceWriteOutcome,
};
use async_trait::async_trait;
use std::sync::RwLock;

#[test]
fn test_redacted_tool_log_summary_omits_values() {
    let args = serde_json::json!({
        "command": "export AWS_SECRET_ACCESS_KEY=AKIAIOSFODNN7EXAMPLE && deploy",
        "timeout": 30
    });
    let summary = redacted_tool_log_summary("bash", &args);
    // Field names and size are logged...
    assert!(summary.contains("bash"));
    assert!(summary.contains("command"));
    assert!(summary.contains("timeout"));
    assert!(summary.contains("bytes"));
    // ...but never the values (the secret must not appear).
    assert!(!summary.contains("AKIAIOSFODNN7EXAMPLE"));
    assert!(!summary.contains("deploy"));
}

#[test]
fn test_redacted_tool_log_summary_handles_non_object_args() {
    let summary = redacted_tool_log_summary("noop", &serde_json::json!("raw string"));
    assert!(summary.contains("noop"));
    assert!(summary.contains("arg_keys=[]"));
    assert!(!summary.contains("raw string"));
}

struct LargeArtifactTool;

#[async_trait]
impl Tool for LargeArtifactTool {
    fn name(&self) -> &str {
        "large_artifact"
    }

    fn description(&self) -> &str {
        "Produces large output for artifact API tests"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let suffix = args
            .get("suffix")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        Ok(ToolOutput::success(format!(
            "{}{}",
            "z".repeat(MAX_OUTPUT_SIZE + 1),
            suffix
        )))
    }
}

struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echoes the message argument"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "message": { "type": "string" }
            },
            "required": ["message"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        Ok(ToolOutput::success(
            args["message"].as_str().unwrap_or_default(),
        ))
    }
}

#[derive(Default)]
struct MemoryWorkspaceFs {
    files: RwLock<HashMap<String, String>>,
}

impl MemoryWorkspaceFs {
    fn insert(&self, path: &str, content: &str) {
        self.files
            .write()
            .unwrap()
            .insert(path.to_string(), content.to_string());
    }

    fn get(&self, path: &str) -> Option<String> {
        self.files.read().unwrap().get(path).cloned()
    }
}

#[async_trait]
impl WorkspaceFileSystem for MemoryWorkspaceFs {
    async fn read_text(&self, path: &WorkspacePath) -> WorkspaceResult<String> {
        self.files
            .read()
            .unwrap()
            .get(path.as_str())
            .cloned()
            .ok_or_else(|| WorkspaceError::NotFound {
                path: path.as_str().to_string(),
            })
    }

    async fn write_text(
        &self,
        path: &WorkspacePath,
        content: &str,
    ) -> WorkspaceResult<WorkspaceWriteOutcome> {
        self.insert(path.as_str(), content);
        Ok(WorkspaceWriteOutcome {
            bytes: content.len(),
            lines: content.lines().count(),
        })
    }

    async fn list_dir(&self, path: &WorkspacePath) -> WorkspaceResult<Vec<WorkspaceDirEntry>> {
        let prefix = if path.is_root() {
            String::new()
        } else {
            format!("{}/", path.as_str())
        };
        let files = self.files.read().unwrap();
        let mut entries = Vec::new();
        for name in files.keys() {
            if !name.starts_with(&prefix) {
                continue;
            }
            let remaining = &name[prefix.len()..];
            if remaining.is_empty() || remaining.contains('/') {
                continue;
            }
            entries.push(WorkspaceDirEntry {
                name: remaining.to_string(),
                kind: WorkspaceFileType::File,
                size: files
                    .get(name)
                    .map(|content| content.len() as u64)
                    .unwrap_or(0),
            });
        }
        Ok(entries)
    }
}

struct MockCommandRunner;

#[async_trait]
impl WorkspaceCommandRunner for MockCommandRunner {
    async fn exec(&self, request: CommandRequest) -> Result<CommandOutput> {
        Ok(CommandOutput {
            output: format!("remote: {}\n", request.command),
            exit_code: 0,
            timed_out: false,
        })
    }
}

#[tokio::test]
async fn test_tool_executor_creation() {
    let executor = ToolExecutor::new("/tmp".to_string());
    // Baseline tools on a raw local ToolExecutor: 13. Workspace search is one
    // model-facing tool with three internal modes.
    assert_eq!(executor.registry.len(), 13);
}

#[tokio::test]
async fn test_unknown_tool() {
    let executor = ToolExecutor::new("/tmp".to_string());
    let result = executor
        .execute("unknown", &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(result.exit_code, 1);
    assert!(result.output.contains("Unknown tool"));
}

#[tokio::test]
async fn test_builtin_tools_registered() {
    let executor = ToolExecutor::new("/tmp".to_string());
    let definitions = executor.definitions();

    assert!(definitions.iter().any(|t| t.name == "bash"));
    assert!(definitions.iter().any(|t| t.name == "read"));
    assert!(definitions.iter().any(|t| t.name == "write"));
    assert!(definitions.iter().any(|t| t.name == "edit"));
    assert!(definitions.iter().any(|t| t.name == "search"));
    assert!(!definitions.iter().any(|t| t.name == "grep"));
    assert!(!definitions.iter().any(|t| t.name == "bm25"));
    assert!(!definitions.iter().any(|t| t.name == "glob"));
    assert!(definitions.iter().any(|t| t.name == "ls"));
    assert!(definitions.iter().any(|t| t.name == "patch"));
    assert!(definitions.iter().any(|t| t.name == "web_fetch"));
    assert!(definitions.iter().any(|t| t.name == "web_search"));
    assert!(definitions.iter().any(|t| t.name == "download"));
    assert!(definitions.iter().any(|t| t.name == "batch"));
}

#[tokio::test]
async fn test_builtin_file_tools_use_workspace_services() {
    let fs = Arc::new(MemoryWorkspaceFs::default());
    fs.insert("remote.txt", "first\nsecond\n");
    let services = WorkspaceServices::builder(
        WorkspaceRef::new("browser-workspace", "browser://workspace"),
        fs.clone(),
    )
    .build();
    let executor = ToolExecutor::new_with_workspace_services_and_artifact_limits(
        "/server/local-placeholder".to_string(),
        services,
        ArtifactStoreLimits::default(),
    );
    let definitions = executor.definitions();
    assert!(definitions.iter().any(|tool| tool.name == "read"));
    assert!(definitions.iter().any(|tool| tool.name == "write"));
    assert!(definitions.iter().any(|tool| tool.name == "ls"));
    assert!(!definitions.iter().any(|tool| tool.name == "bash"));
    assert!(!definitions.iter().any(|tool| tool.name == "search"));
    assert!(definitions.iter().any(|tool| tool.name == "edit"));
    assert!(definitions.iter().any(|tool| tool.name == "patch"));
    assert!(!definitions.iter().any(|tool| tool.name == "download"));

    let read = executor
        .execute("read", &serde_json::json!({"file_path": "remote.txt"}))
        .await
        .unwrap();
    assert_eq!(read.exit_code, 0);
    assert!(read.output.contains("first"));

    let write = executor
        .execute(
            "write",
            &serde_json::json!({"file_path": "created.txt", "content": "remote write\n"}),
        )
        .await
        .unwrap();
    assert_eq!(write.exit_code, 0);
    assert_eq!(fs.get("created.txt").unwrap(), "remote write\n");

    let ls = executor
        .execute("ls", &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(ls.exit_code, 0);
    assert!(ls.output.contains("created.txt"));
    assert!(ls.output.contains("remote.txt"));
}

#[tokio::test]
async fn test_bash_uses_workspace_command_runner() {
    let fs = Arc::new(MemoryWorkspaceFs::default());
    let fs_backend: Arc<dyn WorkspaceFileSystem> = fs;
    let services = WorkspaceServices::builder(
        WorkspaceRef::new("remote-workspace", "remote://workspace"),
        fs_backend,
    )
    .command_runner(Arc::new(MockCommandRunner))
    .build();
    let executor = ToolExecutor::new_with_workspace_services_and_artifact_limits(
        "/server/local-placeholder".to_string(),
        services,
        ArtifactStoreLimits::default(),
    );
    assert!(executor
        .definitions()
        .iter()
        .any(|tool| tool.name == "bash"));

    let result = executor
        .execute("bash", &serde_json::json!({"command": "pwd"}))
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.output, "remote: pwd\n");
}

#[tokio::test]
async fn test_command_env_is_available_on_default_context() {
    let temp = tempfile::tempdir().unwrap();
    let mut env = HashMap::new();
    env.insert(
        "A3S_COMMAND_ENV_TEST".to_string(),
        "registry-env".to_string(),
    );

    let executor = ToolExecutor::new(temp.path().to_string_lossy().to_string());
    executor.registry().set_command_env(Arc::new(env));
    let context = executor.registry().context();
    assert_eq!(
        context
            .command_env
            .as_ref()
            .and_then(|env| env.get("A3S_COMMAND_ENV_TEST"))
            .map(String::as_str),
        Some("registry-env")
    );

    #[cfg(windows)]
    let command = "Write-Output $env:A3S_COMMAND_ENV_TEST";
    #[cfg(not(windows))]
    let command = "printf '%s' \"$A3S_COMMAND_ENV_TEST\"";

    let result = executor
        .execute("bash", &serde_json::json!({ "command": command }))
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0, "{}", result.output);
    assert!(result.output.contains("registry-env"));
}

#[tokio::test]
async fn test_execute_applies_workspace_boundary_for_default_context() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();

    let executor = ToolExecutor::new(workspace.path().to_string_lossy().to_string());
    let result = executor
        .execute(
            "search",
            &serde_json::json!({
                "mode": "grep",
                "query": "secret",
                "path": outside.path().to_string_lossy()
            }),
        )
        .await
        .unwrap();

    assert_eq!(result.exit_code, 1);
    assert!(result.output.contains("Workspace boundary"));
    assert!(result.output.contains("escapes workspace"));
}

#[test]
fn test_tool_result_success() {
    let result = ToolResult::success("test_tool", "output text".to_string());
    assert_eq!(result.name, "test_tool");
    assert_eq!(result.output, "output text");
    assert_eq!(result.exit_code, 0);
    assert!(result.metadata.is_none());
}

#[test]
fn test_tool_result_error() {
    let result = ToolResult::error("test_tool", "error message".to_string());
    assert_eq!(result.name, "test_tool");
    assert_eq!(result.output, "error message");
    assert_eq!(result.exit_code, 1);
    assert!(result.metadata.is_none());
}

#[test]
fn test_tool_result_from_tool_output_success() {
    let output = ToolOutput {
        content: "success content".to_string(),
        success: true,
        metadata: None,
        images: Vec::new(),
        error_kind: None,
    };
    let result: ToolResult = output.into();
    assert_eq!(result.output, "success content");
    assert_eq!(result.exit_code, 0);
    assert!(result.metadata.is_none());
}

#[test]
fn test_tool_result_from_tool_output_failure() {
    let output = ToolOutput {
        content: "failure content".to_string(),
        success: false,
        metadata: Some(serde_json::json!({"error": "test"})),
        images: Vec::new(),
        error_kind: None,
    };
    let result: ToolResult = output.into();
    assert_eq!(result.output, "failure content");
    assert_eq!(result.exit_code, 1);
    assert_eq!(result.metadata, Some(serde_json::json!({"error": "test"})));
}

#[test]
fn test_tool_result_metadata_propagation() {
    let output = ToolOutput::success("content")
        .with_metadata(serde_json::json!({"_load_skill": true, "skill_name": "test"}));
    let result: ToolResult = output.into();
    assert_eq!(result.exit_code, 0);
    let meta = result.metadata.unwrap();
    assert_eq!(meta["_load_skill"], true);
    assert_eq!(meta["skill_name"], "test");
}

#[test]
fn test_tool_executor_workspace() {
    let executor = ToolExecutor::new("/test/workspace".to_string());
    assert_eq!(executor.workspace().to_str().unwrap(), "/test/workspace");
}

#[test]
fn test_tool_executor_registry() {
    let executor = ToolExecutor::new("/tmp".to_string());
    let registry = executor.registry();
    // Baseline tools on a raw local ToolExecutor: 13.
    assert_eq!(registry.len(), 13);
}

#[tokio::test]
async fn test_tool_executor_get_artifact() {
    let executor = ToolExecutor::new("/tmp".to_string());
    executor.register_dynamic_tool(Arc::new(LargeArtifactTool));

    let result = executor
        .execute("large_artifact", &serde_json::json!({}))
        .await
        .unwrap();

    let artifact_uri = result.metadata.as_ref().unwrap()["artifact"]["artifact_uri"]
        .as_str()
        .unwrap();
    let artifact = executor.get_artifact(artifact_uri).expect("artifact");
    assert_eq!(artifact.tool_name, "large_artifact");
    assert_eq!(artifact.content.len(), MAX_OUTPUT_SIZE + 1);
    assert!(executor.artifact_store().get(artifact_uri).is_some());
}

#[tokio::test]
async fn context_efficient_policy_is_applied_with_replayable_evidence() {
    let executor = ToolExecutor::new("/tmp".to_string());
    executor
        .registry()
        .set_tool_result_transform_policy(ToolResultTransformPolicyV1::context_efficient())
        .unwrap();
    executor.register_dynamic_tool(Arc::new(LargeArtifactTool));

    let result = executor
        .execute("large_artifact", &serde_json::json!({}))
        .await
        .unwrap();
    let metadata = result.metadata.as_ref().unwrap();
    let evidence: ToolResultEvidenceV1 =
        serde_json::from_value(metadata["a3s_tool_result_evidence"].clone()).unwrap();

    assert_eq!(evidence.loss_mode, ToolResultLossModeV1::HeadTail);
    assert_eq!(
        evidence.transform_algorithm,
        Some(TOOL_RESULT_TRANSFORM_ALGORITHM_V1.to_string())
    );
    assert_ne!(
        Some(evidence.content_digest.clone()),
        evidence.projected_digest
    );
    assert!(evidence.byte_delta.unwrap() < 0);
    assert!(evidence.content_ref.starts_with("a3s://tool-output/"));
    assert!(result.output.contains("omitted"));
}

#[tokio::test]
async fn test_tool_executor_respects_artifact_limits() {
    let executor = ToolExecutor::new_with_artifact_limits(
        "/tmp".to_string(),
        ArtifactStoreLimits {
            max_artifacts: 1,
            max_bytes: usize::MAX,
        },
    );
    executor.register_dynamic_tool(Arc::new(LargeArtifactTool));

    let first = executor
        .execute("large_artifact", &serde_json::json!({}))
        .await
        .unwrap();
    let first_uri = first.metadata.as_ref().unwrap()["artifact"]["artifact_uri"]
        .as_str()
        .unwrap()
        .to_string();

    executor
        .execute("large_artifact", &serde_json::json!({ "suffix": "again" }))
        .await
        .unwrap();

    assert_eq!(executor.artifact_store().limits().max_artifacts, 1);
    assert_eq!(executor.artifact_store().len(), 1);
    assert!(executor.get_artifact(&first_uri).is_none());
}

#[tokio::test]
async fn test_tool_executor_register_program_catalog_keeps_script_only_program_tool() {
    let executor = ToolExecutor::new("/tmp".to_string());
    let trace_sink = crate::trace::InMemoryTraceSink::default();
    executor.set_trace_sink(Arc::new(trace_sink.clone()));
    executor.register_dynamic_tool(Arc::new(EchoTool));
    let mut catalog = crate::program::ProgramCatalog::new();
    catalog.register(
        crate::program::ProgramTemplate::new("custom_echo", "Run a custom echo program")
            .with_parameter(crate::program::ProgramParameter::required(
                "message",
                "Message to echo",
            ))
            .with_step(
                crate::program::ProgramStepTemplate::new(
                    "echo",
                    serde_json::json!({ "message": "{{message}}" }),
                )
                .with_label("echo_message"),
            ),
    );
    executor.register_program_catalog(catalog);

    let result = executor
        .execute(
            "program",
            &serde_json::json!({
                "name": "custom_echo",
                "inputs": {
                    "message": "hello from catalog"
                }
            }),
        )
        .await
        .unwrap();

    assert_eq!(result.exit_code, 1);
    assert!(result.output.contains("type parameter is required"));

    let events = trace_sink.events();
    assert!(events.iter().any(|event| {
        event.kind == crate::trace::TraceEventKind::ToolExecution && event.name == "program"
    }));
    assert!(!events.iter().any(|event| {
        event.kind == crate::trace::TraceEventKind::ToolExecution && event.name == "echo"
    }));
}

#[test]
fn test_max_output_size_constant() {
    assert_eq!(MAX_OUTPUT_SIZE, 100 * 1024);
}

#[test]
fn test_max_read_lines_constant() {
    assert_eq!(MAX_READ_LINES, 2000);
}

#[test]
fn test_max_line_length_constant() {
    assert_eq!(MAX_LINE_LENGTH, 2000);
}

#[test]
fn test_truncate_tool_output_with_artifact_reference() {
    let output = "x".repeat(MAX_OUTPUT_SIZE + 1);
    let truncated = truncate_tool_output_with_artifact("test/tool", &output);

    let artifact = truncated.artifact.expect("artifact");
    assert!(truncated.content.contains("Full output artifact:"));
    assert_eq!(artifact.original_bytes, MAX_OUTPUT_SIZE + 1);
    assert_eq!(artifact.shown_bytes, MAX_OUTPUT_SIZE);
    assert!(artifact.artifact_id.starts_with("tool-output:test_tool:"));
    assert!(artifact
        .artifact_uri
        .starts_with("a3s://tool-output/test_tool/"));
    assert!(artifact
        .artifact_uri
        .ends_with(&format!("{:x}", sha2::Sha256::digest(output.as_bytes()))));
}

#[test]
fn tool_result_evidence_is_versioned_deterministic_and_byte_exact() {
    let metadata =
        attach_tool_result_evidence(None, "éé", "é", ToolResultLossModeV1::BoundedPreview);
    let value = &metadata["a3s_tool_result_evidence"];
    let evidence: ToolResultEvidenceV1 = serde_json::from_value(value.clone()).unwrap();

    assert_eq!(evidence.schema, TOOL_RESULT_EVIDENCE_SCHEMA_V1);
    assert_eq!(evidence.original_bytes, 4);
    assert_eq!(evidence.projected_bytes, 2);
    assert_eq!(evidence.original_estimated_tokens, 1);
    assert_eq!(evidence.projected_estimated_tokens, 1);
    assert_eq!(evidence.token_estimator, TOOL_RESULT_TOKEN_ESTIMATOR_V1);
    assert_eq!(
        evidence.transform_algorithm,
        Some(TOOL_RESULT_TRANSFORM_ALGORITHM_V1.to_string())
    );
    assert_eq!(evidence.content_digest, evidence.repeat_key);
    assert_ne!(
        Some(evidence.content_digest.clone()),
        evidence.projected_digest
    );
    assert!(evidence.content_digest.starts_with("sha256:"));
    assert_eq!(
        evidence.content_ref,
        format!("inline:{}", evidence.content_digest)
    );
    assert_eq!(evidence.loss_mode, ToolResultLossModeV1::BoundedPreview);
    assert_eq!(evidence.byte_delta, Some(-2));
    assert_eq!(evidence.estimated_token_delta, Some(0));
    assert_eq!(
        metadata,
        attach_tool_result_evidence(None, "éé", "é", ToolResultLossModeV1::BoundedPreview,)
    );
}

#[test]
fn tool_result_evidence_reads_pre_transform_v1_without_rewriting_it() {
    let legacy = serde_json::json!({
        "schema": TOOL_RESULT_EVIDENCE_SCHEMA_V1,
        "original_bytes": 4,
        "projected_bytes": 2,
        "original_estimated_tokens": 1,
        "projected_estimated_tokens": 1,
        "token_estimator": TOOL_RESULT_TOKEN_ESTIMATOR_V1,
        "content_digest": "sha256:source",
        "repeat_key": "sha256:source",
        "content_ref": "a3s://tool-output/test/source",
        "loss_mode": "bounded_preview"
    });
    let evidence: ToolResultEvidenceV1 = serde_json::from_value(legacy.clone()).unwrap();

    assert_eq!(evidence.transform_algorithm, None);
    assert_eq!(evidence.projected_digest, None);
    assert_eq!(evidence.byte_delta, None);
    assert_eq!(evidence.estimated_token_delta, None);
    assert_eq!(
        serde_json::to_value(evidence).unwrap(),
        legacy,
        "reading retained evidence must not manufacture CAR-03 fields"
    );
}

#[test]
fn tool_result_evidence_uses_the_immutable_artifact_reference() {
    let metadata = serde_json::json!({
        "artifact": {"artifact_uri": "a3s://tool-output/bash/abc"},
        "a3s_tool_result_evidence": {"untrusted": true},
    });
    let metadata = attach_tool_result_evidence(
        Some(metadata),
        "full",
        "preview",
        ToolResultLossModeV1::BoundedPreview,
    );
    let evidence: ToolResultEvidenceV1 =
        serde_json::from_value(metadata["a3s_tool_result_evidence"].clone()).unwrap();
    assert_eq!(evidence.content_ref, "a3s://tool-output/bash/abc");
    assert_eq!(evidence.original_bytes, 4);
    assert_eq!(evidence.projected_bytes, 7);
    assert_eq!(evidence.loss_mode, ToolResultLossModeV1::BoundedPreview);
}

#[test]
fn test_tool_result_clone() {
    let result = ToolResult::success("test", "output".to_string());
    let cloned = result.clone();
    assert_eq!(result.name, cloned.name);
    assert_eq!(result.output, cloned.output);
    assert_eq!(result.exit_code, cloned.exit_code);
    assert_eq!(result.metadata, cloned.metadata);
}

#[test]
fn test_tool_result_debug() {
    let result = ToolResult::success("test", "output".to_string());
    let debug_str = format!("{:?}", result);
    assert!(debug_str.contains("test"));
    assert!(debug_str.contains("output"));
}

#[tokio::test]
async fn test_execute_attaches_diff_metadata() {
    use tempfile::TempDir;
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("hello.txt");
    std::fs::write(&file, "before content\n").unwrap();

    let executor = ToolExecutor::new(dir.path().to_str().unwrap().to_string());
    let args = serde_json::json!({
        "file_path": "hello.txt",
        "content": "after content\n"
    });
    let result = executor.execute("write", &args).await.unwrap();

    let meta = result.metadata.expect("metadata should be present");
    assert_eq!(meta["before"], "before content\n");
    assert_eq!(meta["after"], "after content\n");
    assert_eq!(meta["file_path"], "hello.txt");
}

#[tokio::test]
async fn test_execute_with_context_attaches_diff_metadata() {
    use tempfile::TempDir;
    let dir = TempDir::new().unwrap();
    let canonical_dir = dir.path().canonicalize().unwrap();
    let file = canonical_dir.join("ctx.txt");
    std::fs::write(&file, "original\n").unwrap();

    let executor = ToolExecutor::new(canonical_dir.to_str().unwrap().to_string());
    let ctx = ToolContext::new(canonical_dir.clone());
    let args = serde_json::json!({
        "file_path": "ctx.txt",
        "content": "updated\n"
    });
    let result = executor
        .execute_with_context("write", &args, &ctx)
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0, "write tool failed: {}", result.output);

    let meta = result.metadata.expect("metadata should be present");
    assert_eq!(meta["before"], "original\n");
    assert_eq!(meta["after"], "updated\n");
    assert_eq!(meta["file_path"], "ctx.txt");
}
