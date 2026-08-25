use super::*;
use crate::workspace::{
    CommandOutput, CommandRequest, WorkspaceCommandRunner, WorkspaceDirEntry, WorkspaceError,
    WorkspaceFileSystem, WorkspaceFileType, WorkspacePath, WorkspaceRef, WorkspaceResult,
    WorkspaceServices, WorkspaceWriteOutcome,
};
use async_trait::async_trait;
use std::sync::{Arc, Mutex, RwLock};

fn immutable_content_binding(marker: char) -> ImmutableContentAdapterBindingV1 {
    ImmutableContentAdapterBindingV1::new(
        format!("sha256:{}", marker.to_string().repeat(64)),
        (MAX_OUTPUT_SIZE as u64) * 4,
    )
    .expect("test immutable-content binding")
}

#[derive(Default)]
struct RecordingImmutableContentAdapter {
    writes: Mutex<Vec<(ImmutableContentDescriptorV1, Vec<u8>)>>,
    request_debug: Mutex<Vec<String>>,
}

#[async_trait]
impl ImmutableContentAdapter for RecordingImmutableContentAdapter {
    fn name(&self) -> &str {
        "recording-immutable-content"
    }

    async fn put(
        &self,
        request: &ImmutableContentWriteRequestV1<'_>,
    ) -> ImmutableContentResult<ImmutableContentReferenceV1> {
        self.request_debug
            .lock()
            .unwrap()
            .push(format!("{request:?}"));
        self.writes
            .lock()
            .unwrap()
            .push((request.descriptor().clone(), request.content().to_vec()));
        let digest = request
            .descriptor()
            .content_digest
            .strip_prefix("sha256:")
            .expect("canonical test digest");
        ImmutableContentReferenceV1::new(
            request.binding(),
            request.descriptor(),
            format!("a3s+test://authorized-content/{digest}"),
        )
    }
}

struct DriftedImmutableContentAdapter;

#[async_trait]
impl ImmutableContentAdapter for DriftedImmutableContentAdapter {
    fn name(&self) -> &str {
        "drifted-immutable-content"
    }

    async fn put(
        &self,
        request: &ImmutableContentWriteRequestV1<'_>,
    ) -> ImmutableContentResult<ImmutableContentReferenceV1> {
        let digest = request
            .descriptor()
            .content_digest
            .strip_prefix("sha256:")
            .unwrap();
        let mut reference = ImmutableContentReferenceV1::new(
            request.binding(),
            request.descriptor(),
            format!("a3s+test://authorized-content/{digest}"),
        )?;
        reference.size_bytes = reference.size_bytes.saturating_add(1);
        Ok(reference)
    }
}

struct LeakyFailedImmutableContentAdapter;

#[async_trait]
impl ImmutableContentAdapter for LeakyFailedImmutableContentAdapter {
    fn name(&self) -> &str {
        "failed-immutable-content"
    }

    async fn put(
        &self,
        _request: &ImmutableContentWriteRequestV1<'_>,
    ) -> ImmutableContentResult<ImmutableContentReferenceV1> {
        Err(ImmutableContentError::Provider(
            "private-provider-error-sentinel".to_string(),
        ))
    }
}

struct BlockingImmutableContentAdapter {
    started: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl ImmutableContentAdapter for BlockingImmutableContentAdapter {
    fn name(&self) -> &str {
        "blocking-immutable-content"
    }

    async fn put(
        &self,
        _request: &ImmutableContentWriteRequestV1<'_>,
    ) -> ImmutableContentResult<ImmutableContentReferenceV1> {
        self.started.notify_one();
        std::future::pending::<ImmutableContentResult<ImmutableContentReferenceV1>>().await
    }
}

fn immutable_content_session(
    marker: char,
    adapter: Arc<dyn ImmutableContentAdapter>,
) -> ImmutableContentAdapterSession {
    ImmutableContentAdapterSession::new(immutable_content_binding(marker), adapter)
        .expect("test immutable-content session")
}

#[test]
fn immutable_content_public_contract_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<ImmutableContentAdapterBindingV1>();
    assert_send_sync::<ImmutableContentDescriptorV1>();
    assert_send_sync::<ImmutableContentReferenceV1>();
    assert_send_sync::<ImmutableContentWriteRequestV1<'static>>();
    assert_send_sync::<ImmutableContentAdapterSession>();
}

#[test]
fn immutable_content_binding_and_reference_reject_tampering() {
    let binding = immutable_content_binding('a');
    binding.validate().unwrap();
    let descriptor = ImmutableContentDescriptorV1::new(
        ImmutableContentKindV1::ToolResultOriginal,
        TOOL_RESULT_CONTENT_MEDIA_TYPE,
        b"full tool result",
    )
    .unwrap();
    let reference = ImmutableContentReferenceV1::new(
        &binding,
        &descriptor,
        format!(
            "a3s+test://authorized-content/{}",
            descriptor.content_digest.strip_prefix("sha256:").unwrap()
        ),
    )
    .unwrap();
    reference.validate_for(&binding, &descriptor).unwrap();

    let mut drifted_binding = binding.clone();
    drifted_binding.maximum_bytes += 1;
    assert!(drifted_binding.validate().is_err());

    let mut drifted_reference = reference;
    drifted_reference.uri.push_str("-replacement");
    assert!(drifted_reference
        .validate_for(&binding, &descriptor)
        .is_err());

    let digest = descriptor.content_digest.strip_prefix("sha256:").unwrap();
    for unsafe_uri in [
        format!("a3s+test://user:password@authorized-content/{digest}"),
        format!("a3s+test://authorized-content/{digest}?token=secret"),
        format!("a3s+test://authorized-content/{digest}#secret"),
    ] {
        assert!(ImmutableContentReferenceV1::new(&binding, &descriptor, unsafe_uri).is_err());
    }
}

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
fn semantic_query_values_are_not_part_of_invocation_summaries() {
    let sentinel = "private-semantic-query-sentinel";
    let args = serde_json::json!({
        "mode": "semantic",
        "query": sentinel,
        "path": "private/module"
    });
    let summary = redacted_tool_log_summary("search", &args);

    assert!(!summary.contains(sentinel));
    assert!(!summary.contains("private/module"));
    assert!(summary.contains("mode"));
    assert!(summary.contains("query"));
    assert!(summary.contains("path"));
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

struct LargeChangeArtifactTool;

#[async_trait]
impl Tool for LargeChangeArtifactTool {
    fn name(&self) -> &str {
        "large_change_artifact"
    }

    fn description(&self) -> &str {
        "Produces bounded change metadata for immutable-content tests"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }

    async fn execute(&self, _args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        Ok(
            ToolOutput::success("changed").with_metadata(serde_json::json!({
                "before": format!("before\n{}", "a".repeat(40 * 1024)),
                "after": format!("after\n{}", "b".repeat(40 * 1024)),
            })),
        )
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
    #[cfg(windows)]
    let _permit = crate::test_support::resource_intensive_test_permit().await;
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
    let policy = ToolResultTransformPolicyV1::context_efficient();
    executor
        .registry()
        .set_tool_result_transform_policy(policy.clone())
        .unwrap();
    executor.register_dynamic_tool(Arc::new(LargeArtifactTool));

    let result = executor
        .execute("large_artifact", &serde_json::json!({}))
        .await
        .unwrap();
    let metadata = result.metadata.as_ref().unwrap();
    let evidence: ToolResultEvidenceV1 =
        serde_json::from_value(metadata["a3s_tool_result_evidence"].clone()).unwrap();
    let transform_binding: ToolResultTransformBindingV1 =
        serde_json::from_value(metadata[TOOL_RESULT_TRANSFORM_BINDING_METADATA_KEY].clone())
            .unwrap();

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
    transform_binding.validate_for_policy(&policy).unwrap();
    assert!(result.output.contains("omitted"));
}

#[test]
fn retained_transform_binding_drift_fails_closed() {
    let retained_policy = ToolResultTransformPolicyV1::context_efficient();
    let retained_binding = ToolResultTransformBindingV1::from_policy(&retained_policy).unwrap();
    let metadata = attach_tool_result_evidence_with_transform_binding(
        None,
        "original",
        "projected",
        ToolResultLossModeV1::HeadTail,
        &retained_binding,
    )
    .unwrap();
    let drifted_binding =
        ToolResultTransformBindingV1::from_policy(&ToolResultTransformPolicyV1::conservative())
            .unwrap();

    let error = ensure_tool_result_evidence_with_transform_binding(
        Some(metadata),
        "projected",
        &drifted_binding,
    )
    .unwrap_err();
    assert!(error.to_string().contains("binding drifted"));
}

#[tokio::test]
async fn configured_immutable_content_adapter_retains_original_without_local_copy() {
    let adapter = Arc::new(RecordingImmutableContentAdapter::default());
    let binding = immutable_content_binding('b');
    let adapter_port: Arc<dyn ImmutableContentAdapter> = adapter.clone();
    let session = ImmutableContentAdapterSession::new(binding.clone(), adapter_port).unwrap();
    let executor = ToolExecutor::new_with_immutable_content_adapter("/tmp".to_string(), session);
    executor
        .registry()
        .set_tool_result_transform_policy(ToolResultTransformPolicyV1::context_efficient())
        .unwrap();
    executor.register_dynamic_tool(Arc::new(LargeArtifactTool));
    let sentinel = "private-immutable-content-sentinel";

    let result = executor
        .execute("large_artifact", &serde_json::json!({"suffix": sentinel}))
        .await
        .unwrap();
    let metadata = result.metadata.as_ref().unwrap();
    let evidence: ToolResultEvidenceV1 =
        serde_json::from_value(metadata["a3s_tool_result_evidence"].clone()).unwrap();
    let reference: ImmutableContentReferenceV1 =
        serde_json::from_value(metadata["artifact"]["content_reference"].clone()).unwrap();
    let writes = adapter.writes.lock().unwrap();

    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].0.kind, ImmutableContentKindV1::ToolResultOriginal);
    assert_eq!(writes[0].0.media_type, TOOL_RESULT_CONTENT_MEDIA_TYPE);
    assert_eq!(writes[0].0.size_bytes as usize, writes[0].1.len());
    assert!(writes[0].1.ends_with(sentinel.as_bytes()));
    reference.validate_for(&binding, &writes[0].0).unwrap();
    assert_eq!(evidence.content_ref, reference.uri);
    assert_eq!(metadata["artifact"]["artifact_uri"], reference.uri);
    assert!(executor.artifact_store().is_empty());
    assert!(executor.get_artifact(&reference.uri).is_none());
    assert!(adapter
        .request_debug
        .lock()
        .unwrap()
        .iter()
        .all(|debug| !debug.contains(sentinel)));
}

#[tokio::test]
async fn configured_immutable_content_adapter_retains_lossless_original() {
    let adapter = Arc::new(RecordingImmutableContentAdapter::default());
    let binding = immutable_content_binding('9');
    let adapter_port: Arc<dyn ImmutableContentAdapter> = adapter.clone();
    let session = ImmutableContentAdapterSession::new(binding.clone(), adapter_port).unwrap();
    let executor = ToolExecutor::new_with_immutable_content_adapter("/tmp".to_string(), session);
    executor.register_dynamic_tool(Arc::new(EchoTool));
    let content = "bounded-original-content";

    let result = executor
        .execute("echo", &serde_json::json!({"message": content}))
        .await
        .unwrap();
    let metadata = result.metadata.as_ref().unwrap();
    let evidence: ToolResultEvidenceV1 =
        serde_json::from_value(metadata["a3s_tool_result_evidence"].clone()).unwrap();
    let reference: ImmutableContentReferenceV1 =
        serde_json::from_value(metadata["artifact"]["content_reference"].clone()).unwrap();
    let writes = adapter.writes.lock().unwrap();

    assert_eq!(result.output, content);
    assert_eq!(evidence.loss_mode, ToolResultLossModeV1::None);
    assert_eq!(evidence.content_ref, reference.uri);
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].0.kind, ImmutableContentKindV1::ToolResultOriginal);
    assert_eq!(writes[0].1, content.as_bytes());
    reference.validate_for(&binding, &writes[0].0).unwrap();
    assert!(executor.artifact_store().is_empty());
}

#[tokio::test]
async fn immutable_content_byte_ceiling_fails_before_provider_without_local_fallback() {
    let adapter = Arc::new(RecordingImmutableContentAdapter::default());
    let binding =
        ImmutableContentAdapterBindingV1::new(format!("sha256:{}", "8".repeat(64)), 4).unwrap();
    let adapter_port: Arc<dyn ImmutableContentAdapter> = adapter.clone();
    let session = ImmutableContentAdapterSession::new(binding, adapter_port).unwrap();
    let executor = ToolExecutor::new_with_immutable_content_adapter("/tmp".to_string(), session);
    executor.register_dynamic_tool(Arc::new(EchoTool));

    let error = executor
        .execute("echo", &serde_json::json!({"message": "too-large"}))
        .await
        .expect_err("the host-pinned content ceiling must fail closed");

    assert!(error
        .to_string()
        .contains("invalid immutable content descriptor"));
    assert!(adapter.writes.lock().unwrap().is_empty());
    assert!(executor.artifact_store().is_empty());
}

#[tokio::test]
async fn configured_immutable_content_adapter_covers_raw_registry_execution() {
    let adapter = Arc::new(RecordingImmutableContentAdapter::default());
    let adapter_port: Arc<dyn ImmutableContentAdapter> = adapter.clone();
    let session =
        ImmutableContentAdapterSession::new(immutable_content_binding('7'), adapter_port).unwrap();
    let executor = ToolExecutor::new_with_immutable_content_adapter("/tmp".to_string(), session);
    executor.register_dynamic_tool(Arc::new(EchoTool));

    let output = executor
        .registry()
        .execute_raw("echo", &serde_json::json!({"message": "raw-result"}))
        .await
        .unwrap()
        .unwrap();
    let reference: ImmutableContentReferenceV1 =
        serde_json::from_value(output.metadata.unwrap()["artifact"]["content_reference"].clone())
            .unwrap();

    assert_eq!(output.content, "raw-result");
    assert_eq!(adapter.writes.lock().unwrap().len(), 1);
    assert!(reference
        .uri
        .contains(reference.content_digest.strip_prefix("sha256:").unwrap()));
    assert!(executor.artifact_store().is_empty());
}

#[tokio::test]
async fn configured_immutable_content_adapter_retains_compacted_change_sides() {
    let adapter = Arc::new(RecordingImmutableContentAdapter::default());
    let adapter_port: Arc<dyn ImmutableContentAdapter> = adapter.clone();
    let session =
        ImmutableContentAdapterSession::new(immutable_content_binding('f'), adapter_port).unwrap();
    let executor = ToolExecutor::new_with_immutable_content_adapter("/tmp".to_string(), session);
    executor.register_dynamic_tool(Arc::new(LargeChangeArtifactTool));

    let result = executor
        .execute("large_change_artifact", &serde_json::json!({}))
        .await
        .unwrap();
    let metadata = result.metadata.unwrap();
    let writes = adapter.writes.lock().unwrap();

    assert_eq!(writes.len(), 3);
    assert_eq!(writes[0].0.kind, ImmutableContentKindV1::ToolChangeBefore);
    assert_eq!(writes[1].0.kind, ImmutableContentKindV1::ToolChangeAfter);
    assert_eq!(writes[2].0.kind, ImmutableContentKindV1::ToolResultOriginal);
    assert_eq!(writes[2].1, b"changed");
    for side in ["before", "after"] {
        let reference: ImmutableContentReferenceV1 = serde_json::from_value(
            metadata["change"][side]["artifact"]["content_reference"].clone(),
        )
        .unwrap();
        assert_eq!(
            metadata["change"][side]["artifact"]["artifact_uri"],
            reference.uri
        );
    }
    assert!(executor.artifact_store().is_empty());
}

#[tokio::test]
async fn immutable_content_reference_drift_fails_closed_without_local_fallback() {
    let session = immutable_content_session('c', Arc::new(DriftedImmutableContentAdapter));
    let executor = ToolExecutor::new_with_immutable_content_adapter("/tmp".to_string(), session);
    executor.register_dynamic_tool(Arc::new(LargeArtifactTool));

    let error = executor
        .execute("large_artifact", &serde_json::json!({}))
        .await
        .expect_err("a drifted reference must not release transformed content");

    assert!(error.to_string().contains("immutable content reference"));
    assert!(executor.artifact_store().is_empty());
}

#[tokio::test]
async fn immutable_content_provider_error_detail_is_not_released() {
    let session = immutable_content_session('e', Arc::new(LeakyFailedImmutableContentAdapter));
    let executor = ToolExecutor::new_with_immutable_content_adapter("/tmp".to_string(), session);
    executor.register_dynamic_tool(Arc::new(LargeArtifactTool));

    let error = executor
        .execute("large_artifact", &serde_json::json!({}))
        .await
        .expect_err("provider failure must fail closed");
    let message = error.to_string();

    assert!(message.contains("immutable content provider failure"));
    assert!(!message.contains("private-provider-error-sentinel"));
    assert!(executor.artifact_store().is_empty());
}

#[tokio::test]
async fn cancellation_interrupts_immutable_content_retention() {
    let started = Arc::new(tokio::sync::Notify::new());
    let session = immutable_content_session(
        'd',
        Arc::new(BlockingImmutableContentAdapter {
            started: Arc::clone(&started),
        }),
    );
    let executor = Arc::new(ToolExecutor::new_with_immutable_content_adapter(
        "/tmp".to_string(),
        session,
    ));
    executor.register_dynamic_tool(Arc::new(LargeArtifactTool));
    let cancellation = tokio_util::sync::CancellationToken::new();
    let context = executor
        .registry()
        .context()
        .with_cancellation(cancellation.clone());
    let task_executor = Arc::clone(&executor);
    let task = tokio::spawn(async move {
        task_executor
            .execute_with_context("large_artifact", &serde_json::json!({}), &context)
            .await
    });

    tokio::time::timeout(std::time::Duration::from_secs(1), started.notified())
        .await
        .expect("adapter must begin retention");
    cancellation.cancel();
    let error = tokio::time::timeout(std::time::Duration::from_secs(1), task)
        .await
        .expect("cancellation must interrupt adapter backpressure")
        .unwrap()
        .expect_err("cancelled retention must fail closed");

    assert!(error.to_string().contains("cancelled"));
    assert!(executor.artifact_store().is_empty());
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
