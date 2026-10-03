//! Ignored live kernel oracles for the deep end-to-end case list.
//!
//! Each `DEEP_E2E_ROW` is printed only after that case's kernel oracle passes.
//! The receipt file is written by the runner after this process exits 0.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

const ADMISSION_COMMIT: &str = "9fb1cb7985226c7971ef59a20710b46391af2f2b";
const PINNED_MODEL: &str = "boyue/bailian/deepseek-v4.1-flash";

struct ClearResolutionScript;

impl Drop for ClearResolutionScript {
    fn drop(&mut self) {
        crate::tools::set_resolution_script(None);
    }
}

fn row(id: &str, model: &str, oracle: &str) {
    assert!(
        !oracle.contains(' ') && !oracle.contains("commit="),
        "oracle token must be a single token"
    );
    assert!(
        !model.contains(' ') && !model.contains("commit="),
        "model token must be a single token"
    );
    println!("DEEP_E2E_ROW id={id} commit={ADMISSION_COMMIT} model={model} oracle={oracle}");
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

fn redact(text: &str) -> String {
    let mut redacted = String::new();
    let mut rest = text;
    loop {
        let http = rest.find("http://");
        let https = rest.find("https://");
        let start = match (http, https) {
            (Some(left), Some(right)) => left.min(right),
            (Some(left), None) => left,
            (None, Some(right)) => right,
            (None, None) => break,
        };
        redacted.push_str(&rest[..start]);
        redacted.push_str("[redacted]");
        let after = &rest[start..];
        let skip = if after.starts_with("https://") { 8 } else { 7 };
        let tail = &after[skip..];
        let end = tail
            .find(|character: char| character.is_whitespace())
            .unwrap_or(tail.len());
        rest = &tail[end..];
    }
    redacted.push_str(rest);
    let lower = redacted.to_ascii_lowercase();
    if lower.contains("bearer ")
        || lower.contains("sk-")
        || lower.contains("api_key")
        || lower.contains("authorization")
    {
        return "model call failed".to_string();
    }
    redacted
}

fn accept_model(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.contains("http")
        || trimmed.contains('@')
        || trimmed.chars().any(char::is_whitespace)
    {
        PINNED_MODEL.to_string()
    } else {
        trimmed.to_string()
    }
}

fn assert_admission_head() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&repo)
        .output()
        .expect("git rev-parse");
    assert!(output.status.success(), "git rev-parse failed");
    let head = String::from_utf8_lossy(&output.stdout);
    let head = head.trim();
    assert!(
        head.starts_with(ADMISSION_COMMIT),
        "live receipt requires admission commit {ADMISSION_COMMIT}, HEAD is {head}"
    );
}

async fn served_model() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../.a3s/config.acl");
    let config = match crate::CodeConfig::from_file(&path) {
        Ok(config) => config,
        Err(_) => panic!("acl unreadable"),
    };
    let llm = match config.llm_config("boyue", "bailian/deepseek-v4.1-flash") {
        Some(llm) => llm,
        None => panic!("route missing"),
    };
    let client = crate::llm::create_client_with_config(llm);
    let response = match client
        .complete(
            &[crate::llm::Message::user("Reply with the single word ok")],
            None,
            &[],
        )
        .await
    {
        Ok(response) => response,
        Err(error) => panic!("model call failed: {}", redact(&format!("{error:#}"))),
    };
    let served = response
        .meta
        .as_ref()
        .and_then(|meta| meta.response_model.clone())
        .unwrap_or_default();
    accept_model(&served)
}

fn workspace_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("temp workspace")
}

fn admitted_options(id: &str) -> crate::SessionOptions {
    crate::SessionOptions::new()
        .with_session_id(id)
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
}

async fn open_session(id: &str) -> (crate::Agent, crate::AgentSession, tempfile::TempDir) {
    let dir = workspace_dir();
    let agent = crate::Agent::from_config(crate::agent_api::test_config())
        .await
        .expect("test agent");
    let session = crate::agent_api::create_session(
        &agent,
        dir.path().to_string_lossy().as_ref(),
        Some(admitted_options(id)),
    )
    .expect("session");
    (agent, session, dir)
}

fn json_has_key(value: &Value, key: &str) -> bool {
    match value {
        Value::Object(map) => {
            map.keys().any(|name| name == key) || map.values().any(|child| json_has_key(child, key))
        }
        Value::Array(items) => items.iter().any(|child| json_has_key(child, key)),
        _ => false,
    }
}

#[tokio::test]
#[ignore = "live deep e2e against the admission commit"]
async fn deep_e2e_live_receipts() {
    let _clear_resolution = ClearResolutionScript;
    assert_admission_head();
    let model = served_model().await;

    case_agent_runtime(&model).await;
    case_conversation(&model).await;
    case_run_control(&model).await;
    case_governed_tools(&model).await;
    case_workspace_tools(&model).await;
    case_workspace_retrieval(&model).await;
    case_model_adapters(&model).await;
    case_structured_output(&model).await;
    case_mcp_and_skills(&model).await;
    case_planning_delegation(&model).await;
    case_priority_scheduling(&model).await;
    case_persistence(&model).await;
    case_governance(&model).await;
    case_run_observability(&model).await;
    case_context_memory(&model).await;
    case_web_search(&model).await;
    case_web_fetch(&model).await;
    case_code_intelligence(&model).await;
    case_cognitive_packages(&model).await;
    case_use_runtime_tasks(&model).await;
    case_program(&model).await;
    case_programmable_workflows(&model).await;
    case_state_graph(&model).await;
    case_agent_release_contract(&model).await;
    case_agent_protocol(&model).await;
    case_evaluation_substrate(&model).await;
    case_typed_decisions(&model).await;
    case_moli_runtime(&model).await;
    case_s3_workspace(&model).await;
    case_opentelemetry(&model).await;
    case_effect_isolation(&model).await;
    case_native_sandbox(&model).await;
    case_safe_http(&model).await;
    case_mutation_verify_gate(&model).await;
    case_batch_schema(&model).await;
    case_image_read(&model).await;
    case_event_envelope(&model).await;
    case_research_wire(&model).await;
}

async fn case_agent_runtime(model: &str) {
    let (agent, _session, _dir) = open_session("session-1").await;
    agent.close().await;
    let before = agent.list_sessions().await;
    let err = crate::agent_api::create_session(
        &agent,
        _dir.path().to_string_lossy().as_ref(),
        Some(crate::SessionOptions::new().with_session_id("session-closed-again")),
    );
    match err {
        Err(crate::CodeError::SessionClosed { session_id }) if session_id == "<agent-closed>" => {}
        other => panic!("agent_runtime: expected SessionClosed, got {other:?}"),
    }
    let after = agent.list_sessions().await;
    assert_eq!(before, after);
    assert!(!after.iter().any(|id| id == "session-closed-again"));
    row("agent_runtime", model, "SessionClosed");
}

async fn case_conversation(model: &str) {
    let (_agent, session, _dir) = open_session("session-1").await;
    let state = crate::agent_api::RunControlState::from_session(&session);
    let handle = state.start_run("replay-prompt").await;
    let run_id = handle.id().to_string();
    let replayed = crate::agent_api::exact_run_replay(&session, &run_id, "replay-prompt")
        .await
        .expect("matching replay");
    assert!(matches!(
        replayed,
        Some(crate::AgentRunSpawn::Replayed { .. })
    ));
    let conflict = crate::agent_api::exact_run_replay(&session, &run_id, "other-prompt").await;
    match conflict {
        Err(crate::CodeError::RunIdentityConflict { .. }) => {}
        Err(other) => panic!("conversation: expected RunIdentityConflict, got {other}"),
        Ok(_) => panic!("conversation: expected RunIdentityConflict"),
    }
    row("conversation", model, "RunIdentityConflict");
}

async fn case_run_control(model: &str) {
    let inbox =
        crate::run_control::RunControlInbox::new("session-1", "run-1", CancellationToken::new());
    let snapshot = inbox.update_turn(1).await;
    let request = crate::RunControlRequest::steer("run-1", "later")
        .with_session_id("session-1")
        .with_expected_turn("turn-0", snapshot.turn_revision);
    let err = inbox.submit_locked(request, 1).await;
    match err {
        Err(crate::RunControlError::StaleTurn { .. }) => {}
        other => panic!("run_control: expected StaleTurn, got {other:?}"),
    }
    assert_eq!(inbox.snapshot().await.queued_controls, 0);
    row("run_control", model, "StaleTurn");
}

async fn case_governed_tools(model: &str) {
    let (_agent, session, _dir) = open_session("governed-1").await;
    let runtime = crate::agent_api::DirectToolRuntime::from_session(&session);
    session.close().await;
    let err = runtime
        .call_governed("read", json!({"file_path": "missing.txt"}))
        .await;
    match err {
        Err(crate::CodeError::SessionClosed { session_id }) if session_id == "governed-1" => {}
        other => panic!("governed_tools: expected SessionClosed, got {other:?}"),
    }
    row("governed_tools", model, "SessionClosed");
}

async fn case_workspace_tools(model: &str) {
    use crate::tools::Tool;
    use crate::workspace::{
        WorkspaceDirEntry, WorkspaceError, WorkspaceFileSystem, WorkspaceFileSystemExt,
        WorkspacePath, WorkspaceRef, WorkspaceResult, WorkspaceServices, WorkspaceVersionConflict,
        WorkspaceWriteOutcome,
    };

    struct AlwaysConflictFs;

    #[async_trait::async_trait]
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
        async fn list_dir(&self, _path: &WorkspacePath) -> WorkspaceResult<Vec<WorkspaceDirEntry>> {
            Ok(Vec::new())
        }
    }

    #[async_trait::async_trait]
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

    let dir = workspace_dir();
    let backend = Arc::new(AlwaysConflictFs);
    let fs: Arc<dyn WorkspaceFileSystem> = backend.clone();
    let fs_ext: Arc<dyn WorkspaceFileSystemExt> = backend;
    let services = WorkspaceServices::builder(WorkspaceRef::new("mem", "mem://deep-e2e"), fs)
        .file_system_ext(fs_ext)
        .build();
    let ctx = crate::tools::ToolContext::new(dir.path().to_path_buf())
        .with_session_id("deep-e2e-write")
        .with_workspace_services(services);
    let result = crate::tools::WriteTool
        .execute(
            &json!({
                "file_path": "note.txt",
                "content": "!",
                "mode": "append",
                "expected_offset": 11
            }),
            &ctx,
        )
        .await
        .expect("write returns a tool output");
    assert!(!result.success);
    assert!(matches!(
        result.error_kind,
        Some(crate::ToolErrorKind::VersionConflict { .. })
    ));
    row("workspace_tools", model, "VersionConflict");
}

async fn case_workspace_retrieval(model: &str) {
    use crate::workspace::conformance::InMemoryFileSystem;
    use crate::{
        ChunkCatalogLimits, ChunkingConfig, EmbeddingBatchRequest, EmbeddingBatchResponse,
        EmbeddingNormalization, EmbeddingProvider, EmbeddingProviderDescriptor,
        EmbeddingProviderError, WorkspaceChunkCatalog, WorkspaceRef, WorkspaceRetrievalError,
        WorkspaceRetrievalOptions, WorkspaceRetrievalRuntime, WorkspaceSemanticSearchRequest,
        WorkspaceServices,
    };

    struct DescriptorOnlyProvider;

    #[async_trait::async_trait]
    impl EmbeddingProvider for DescriptorOnlyProvider {
        fn descriptor(&self) -> EmbeddingProviderDescriptor {
            EmbeddingProviderDescriptor::new("fixture", "semantic-binding", 2)
                .with_revision("fixture-r1")
                .with_normalization(EmbeddingNormalization::Unit)
        }

        async fn embed(
            &self,
            _request: EmbeddingBatchRequest,
            _cancellation: CancellationToken,
        ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
            Err(EmbeddingProviderError::InvalidRequest)
        }
    }

    let fs: Arc<dyn crate::workspace::WorkspaceFileSystem> = Arc::new(InMemoryFileSystem::new());
    let services =
        WorkspaceServices::builder(WorkspaceRef::new("mem", "mem://retrieval"), fs).build();
    let catalog =
        WorkspaceChunkCatalog::new(ChunkingConfig::default(), ChunkCatalogLimits::default())
            .expect("empty catalog");
    let options = WorkspaceRetrievalOptions::new(Arc::new(DescriptorOnlyProvider))
        .with_semantic_readiness_timeout(Duration::from_secs(2));
    let runtime = WorkspaceRetrievalRuntime::start(catalog, options, CancellationToken::new())
        .expect("retrieval runtime");
    let services = services
        .with_workspace_retrieval(runtime)
        .expect("attach retrieval");
    let invalid = services
        .semantic_search(
            WorkspaceSemanticSearchRequest::new("deep-e2e").with_limit(0),
            CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(invalid, Err(WorkspaceRetrievalError::InvalidQuery(_))),
        "workspace_retrieval: limit 0 must be InvalidQuery"
    );
    let first = services
        .semantic_search(
            WorkspaceSemanticSearchRequest::new("deep-e2e"),
            CancellationToken::new(),
        )
        .await
        .expect("empty semantic search");
    let second = services
        .semantic_search(
            WorkspaceSemanticSearchRequest::new("deep-e2e"),
            CancellationToken::new(),
        )
        .await
        .expect("replayed semantic search");
    assert_eq!(first.hits.len(), second.hits.len());
    assert!(first.hits.is_empty(), "empty index must not invent a hit");
    row("workspace_retrieval", model, "InvalidQuery");
}

async fn case_model_adapters(model: &str) {
    use crate::llm::{
        HttpClient, HttpResponse, NonRetryableLlmError, OpenAiClient, RetryConfig,
        StreamingHttpResponse,
    };

    struct StatusHttp {
        status: u16,
    }

    #[async_trait::async_trait]
    impl HttpClient for StatusHttp {
        async fn post(
            &self,
            _url: &str,
            _headers: Vec<(&str, &str)>,
            _body: &Value,
            _cancel: CancellationToken,
        ) -> anyhow::Result<HttpResponse> {
            Ok(HttpResponse {
                status: self.status,
                body: "provider error".to_string(),
                retry_after: None,
            })
        }

        async fn post_streaming(
            &self,
            _url: &str,
            _headers: Vec<(&str, &str)>,
            _body: &Value,
            _cancel: CancellationToken,
        ) -> anyhow::Result<StreamingHttpResponse> {
            Ok(StreamingHttpResponse {
                status: self.status,
                retry_after: None,
                byte_stream: Box::pin(futures::stream::empty()),
                error_body: "provider error".to_string(),
            })
        }
    }

    let client = OpenAiClient::new("test-key".to_string(), "gpt-test".to_string())
        .with_retry_config(RetryConfig::disabled())
        .with_http_client(Arc::new(StatusHttp { status: 402 }));
    let err = client
        .send_request(json!({
            "model": "gpt-test",
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .await
        .expect_err("402 must be terminal");
    let typed = err
        .downcast_ref::<NonRetryableLlmError>()
        .expect("terminal provider error");
    assert_eq!(typed.provider(), Some("openai"));
    assert_eq!(typed.status(), Some(402));
    row("model_adapters", model, "openai-402");
}

async fn case_structured_output(model: &str) {
    let provider = crate::security::NoOpSecurityProvider;
    let schema = json!({
        "type": "object",
        "properties": {"n": {"type": "number"}},
        "required": ["n"]
    });
    let accepted = crate::tools::task::apply_structured_output_sanitization(
        &provider,
        json!({"n": 1}),
        Some(&schema),
    )
    .expect("number matches schema");
    assert_eq!(accepted["n"], 1);
    let rejected = crate::tools::task::apply_structured_output_sanitization(
        &provider,
        json!({"n": "x"}),
        Some(&schema),
    );
    assert!(rejected.is_err());
    row("structured_output", model, "schema-rejected");
}

async fn case_mcp_and_skills(model: &str) {
    use crate::mcp::{McpServerConfig, McpTransportConfig};

    let (_agent, session, _dir) = open_session("mcp-1").await;
    let extensions = crate::agent_api::SessionExtensionRuntime::from_session(&session);
    let config = McpServerConfig {
        name: "deep-e2e-mcp".to_string(),
        transport: McpTransportConfig::Stdio {
            command: "/nonexistent/a3s-deep-e2e-mcp".to_string(),
            args: Vec::new(),
        },
        enabled: true,
        env: std::collections::HashMap::new(),
        oauth: None,
        tool_timeout_secs: 60,
    };
    let failed = extensions.add_mcp_server(config.clone()).await;
    assert!(
        failed.is_err(),
        "missing mcp command must not install tools"
    );
    assert!(!session
        .tool_names()
        .iter()
        .any(|name| name.starts_with("mcp__deep-e2e-mcp")));
    let _ = extensions.remove_mcp_server("deep-e2e-mcp").await;
    let names = session.tool_names();
    let traces = session.trace_events().len();
    session.close().await;
    let closed = extensions.add_mcp_server(config).await;
    match closed {
        Err(crate::CodeError::SessionClosed { session_id }) if session_id == "mcp-1" => {}
        other => panic!("mcp_and_skills: expected SessionClosed, got {other:?}"),
    }
    assert_eq!(session.tool_names(), names);
    assert_eq!(session.trace_events().len(), traces);
    row("mcp_and_skills", model, "SessionClosed");
}

async fn case_planning_delegation(model: &str) {
    use crate::llm::{LlmClient, Message, ToolDefinition};
    use crate::tools::Tool;

    struct PanicClient;

    #[async_trait::async_trait]
    impl LlmClient for PanicClient {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
        ) -> anyhow::Result<crate::llm::LlmResponse> {
            panic!("planning_delegation: child model call");
        }

        async fn complete_streaming(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
            _cancel: CancellationToken,
        ) -> anyhow::Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
            panic!("planning_delegation: child model stream");
        }
    }

    let dir = workspace_dir();
    let executor = crate::tools::TaskExecutor::new(
        Arc::new(crate::AgentRegistry::new()),
        Arc::new(PanicClient),
        dir.path().to_string_lossy().into_owned(),
    );
    let tool = crate::tools::TaskTool::new(Arc::new(executor));
    let ctx = crate::tools::ToolContext::new(dir.path().to_path_buf());
    let result = tool
        .execute(
            &json!({
                "tasks": [
                    {"agent": "explore", "description": "one", "prompt": "alpha", "background": true},
                    {"agent": "explore", "description": "two", "prompt": "beta"}
                ]
            }),
            &ctx,
        )
        .await
        .expect("fan-out reject is a tool output");
    assert!(!result.success);
    assert!(matches!(
        result.error_kind,
        Some(crate::ToolErrorKind::InvalidArgument { .. })
    ));
    row("planning_delegation", model, "InvalidArgument");
}

async fn case_priority_scheduling(model: &str) {
    let scheduler = Arc::new(
        crate::TaskScheduler::new(crate::TaskSchedulerConfig {
            max_active: 1,
            aging_interval_ms: 60_000,
        })
        .expect("scheduler"),
    );
    let lease = scheduler
        .acquire(
            crate::TaskPriority::Interactive,
            "blocker",
            &CancellationToken::new(),
        )
        .await
        .expect("lease");
    let shutdown = {
        let scheduler = Arc::clone(&scheduler);
        tokio::spawn(async move { scheduler.shutdown().await })
    };
    let mut saw_closed = false;
    for _ in 0..50 {
        if matches!(
            scheduler.stats().await,
            Err(crate::TaskSchedulerError::Closed)
        ) {
            saw_closed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        saw_closed,
        "priority_scheduling: stats stayed open during shutdown"
    );
    assert!(!shutdown.is_finished(), "shutdown must wait for the lease");
    drop(lease);
    shutdown.await.expect("shutdown");
    assert!(matches!(
        scheduler.stats().await,
        Err(crate::TaskSchedulerError::Closed)
    ));
    row("priority_scheduling", model, "Closed");
}

async fn case_persistence(model: &str) {
    use crate::store::SessionStore;

    struct BoomStore;

    #[async_trait::async_trait]
    impl SessionStore for BoomStore {
        async fn save(&self, _session: &crate::store::SessionData) -> anyhow::Result<()> {
            Ok(())
        }
        async fn load(&self, _id: &str) -> anyhow::Result<Option<crate::store::SessionData>> {
            Ok(None)
        }
        async fn delete(&self, _id: &str) -> anyhow::Result<()> {
            Ok(())
        }
        async fn list(&self) -> anyhow::Result<Vec<String>> {
            Ok(Vec::new())
        }
        async fn exists(&self, _id: &str) -> anyhow::Result<bool> {
            Ok(false)
        }
    }

    let dir = workspace_dir();
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = crate::Agent::from_config(crate::agent_api::test_config())
        .await
        .expect("test agent");
    let session = crate::agent_api::create_session(
        &agent,
        dir.path().to_string_lossy().as_ref(),
        Some(admitted_options("persist-1").with_session_store(store.clone())),
    )
    .expect("session");
    crate::agent_api::SessionPersistenceContext::from_session(&session)
        .save()
        .await
        .expect("memory snapshot");
    let first = store
        .load_snapshot("persist-1")
        .await
        .expect("load")
        .expect("saved snapshot");
    let second = store
        .load_snapshot("persist-1")
        .await
        .expect("reload")
        .expect("saved snapshot");
    let left = crate::store::snapshot_content_digest(&first).expect("digest");
    let right = crate::store::snapshot_content_digest(&second).expect("digest");
    assert_eq!(left, right);

    let boom_dir = workspace_dir();
    let boom = crate::agent_api::create_session(
        &agent,
        boom_dir.path().to_string_lossy().as_ref(),
        Some(admitted_options("persist-boom").with_session_store(Arc::new(BoomStore))),
    )
    .expect("boom session");
    let err = crate::agent_api::SessionPersistenceContext::from_session(&boom)
        .save()
        .await;
    match err {
        Err(crate::CodeError::Session(message)) if message.contains("Failed to save session") => {}
        other => panic!("persistence: expected Session, got {other:?}"),
    }
    row("persistence", model, "snapshot-digest");
}

async fn case_governance(model: &str) {
    use crate::hitl::{ConfirmationPolicy, ConfirmationProvider, ConfirmationResponse};

    struct StubConfirm;

    #[async_trait::async_trait]
    impl ConfirmationProvider for StubConfirm {
        async fn requires_confirmation(&self, _tool_name: &str) -> bool {
            false
        }
        async fn request_confirmation(
            &self,
            _tool_id: &str,
            _tool_name: &str,
            _args: &Value,
        ) -> tokio::sync::oneshot::Receiver<ConfirmationResponse> {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            drop(sender);
            receiver
        }
        async fn confirm(
            &self,
            tool_id: &str,
            _approved: bool,
            _reason: Option<String>,
        ) -> Result<bool, String> {
            if tool_id == "done" {
                Ok(true)
            } else {
                Err("manager down".to_string())
            }
        }
        async fn policy(&self) -> ConfirmationPolicy {
            ConfirmationPolicy::default()
        }
        async fn set_policy(&self, _policy: ConfirmationPolicy) {}
        async fn check_timeouts(&self) -> usize {
            0
        }
        async fn cancel_all(&self) -> usize {
            0
        }
    }

    let dir = workspace_dir();
    let agent = crate::Agent::from_config(crate::agent_api::test_config())
        .await
        .expect("test agent");
    let session = crate::agent_api::create_session(
        &agent,
        dir.path().to_string_lossy().as_ref(),
        Some(admitted_options("gov-1").with_confirmation_manager(Arc::new(StubConfirm))),
    )
    .expect("session");
    assert_eq!(
        session
            .confirm_tool_use("done", true, None)
            .await
            .expect("approve"),
        true
    );
    let err = session.confirm_tool_use("boom", false, None).await;
    match err {
        Err(crate::CodeError::Session(message)) if message.contains("manager down") => {}
        other => panic!("governance: expected Session, got {other:?}"),
    }
    row("governance", model, "Session");
}

async fn case_run_observability(model: &str) {
    let log = crate::CoreEventLog::new();
    let operation = crate::OperationId::new("deep-e2e-op").expect("operation");
    let revision = crate::SourceRevision::new(1);
    let record = crate::RunEventRecord {
        sequence: 0,
        timestamp_ms: 10,
        event: crate::AgentEvent::TextDelta {
            text: "ok".to_string(),
        },
    };
    let appended = log
        .append_run_event(operation.clone(), revision, None, &record)
        .expect("append");
    assert!(appended.appended && !appended.replayed);
    let replayed = log
        .append_run_event(operation.clone(), revision, None, &record)
        .expect("replay");
    assert!(!replayed.appended && replayed.replayed);
    let conflict = crate::RunEventRecord {
        event: crate::AgentEvent::TextDelta {
            text: "no".to_string(),
        },
        ..record
    };
    let err = log
        .append_run_event(operation, revision, None, &conflict)
        .expect_err("cursor conflict");
    assert!(matches!(err, crate::CoreEventLogError::CursorConflict));
    row("run_observability", model, "CursorConflict");
}

async fn case_context_memory(model: &str) {
    use a3s_memory::repository::{
        DurableMemoryKind, EvidenceKind, EvidenceRef, InMemoryRepository, MemoryChangeSet,
        MemoryNamespace, MemoryNodeDraft, MemoryOperation, MemoryRelation, MemoryRelationKind,
        MemoryRepository, MemoryStatus,
    };
    use chrono::{DateTime, Utc};

    fn time(offset_seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_777_000_000 + offset_seconds, 0).expect("time")
    }
    fn evidence(name: &str, kind: EvidenceKind, offset_seconds: i64) -> EvidenceRef {
        EvidenceRef::try_new(
            format!("a3s://evidence/{name}"),
            format!("sha256:{name:0>64}"),
            kind,
            time(offset_seconds),
        )
        .expect("evidence")
    }

    let repository = Arc::new(InMemoryRepository::new());
    let namespace = MemoryNamespace::try_new("tenant", "principal", "scope").expect("namespace");
    repository
        .apply(MemoryChangeSet::new(
            "seed-related-recall",
            namespace.clone(),
            time(1),
            vec![
                MemoryOperation::Create {
                    node: MemoryNodeDraft::new(
                        "rollback-index",
                        namespace.clone(),
                        DurableMemoryKind::Semantic,
                        MemoryStatus::Active,
                        "Deployment rollback playbook index",
                        vec![evidence(
                            "index-verification",
                            EvidenceKind::Verification,
                            1,
                        )],
                        time(1),
                    )
                    .with_relation(MemoryRelation::new(
                        MemoryRelationKind::RelatedTo,
                        "canary-procedure",
                    ))
                    .with_relation(MemoryRelation::new(
                        MemoryRelationKind::RelatedTo,
                        "zebra-procedure",
                    ))
                    .with_relation(MemoryRelation::new(
                        MemoryRelationKind::ConflictsWith,
                        "unsafe-procedure",
                    )),
                },
                MemoryOperation::Create {
                    node: MemoryNodeDraft::new(
                        "zebra-procedure",
                        namespace.clone(),
                        DurableMemoryKind::Procedural,
                        MemoryStatus::Active,
                        "Restart every production shard at the same time",
                        vec![evidence(
                            "zebra-verification",
                            EvidenceKind::Verification,
                            1,
                        )],
                        time(1),
                    )
                    .with_relation(MemoryRelation::new(
                        MemoryRelationKind::RelatedTo,
                        "rollback-index",
                    )),
                },
                MemoryOperation::Create {
                    node: MemoryNodeDraft::new(
                        "canary-procedure",
                        namespace.clone(),
                        DurableMemoryKind::Procedural,
                        MemoryStatus::Active,
                        "Drain the first ring before shifting production traffic",
                        vec![evidence(
                            "canary-verification",
                            EvidenceKind::Verification,
                            1,
                        )],
                        time(1),
                    )
                    .with_relation(MemoryRelation::new(
                        MemoryRelationKind::RelatedTo,
                        "rollback-index",
                    )),
                },
                MemoryOperation::Create {
                    node: MemoryNodeDraft::new(
                        "unsafe-procedure",
                        namespace.clone(),
                        DurableMemoryKind::Procedural,
                        MemoryStatus::Active,
                        "Shift all traffic without observing the first ring",
                        vec![evidence(
                            "unsafe-verification",
                            EvidenceKind::Verification,
                            1,
                        )],
                        time(1),
                    )
                    .with_relation(MemoryRelation::new(
                        MemoryRelationKind::ConflictsWith,
                        "rollback-index",
                    )),
                },
                MemoryOperation::Create {
                    node: MemoryNodeDraft::new(
                        "candidate-related",
                        namespace.clone(),
                        DurableMemoryKind::Procedural,
                        MemoryStatus::Candidate,
                        "Unverified recovery shortcut",
                        vec![evidence("candidate-proposal", EvidenceKind::SessionTurn, 1)],
                        time(1),
                    ),
                },
            ],
        ))
        .await
        .expect("seed");
    repository
        .apply(MemoryChangeSet::new(
            "attach-candidate-relation",
            namespace.clone(),
            time(2),
            vec![
                MemoryOperation::AddRelation {
                    node_id: "rollback-index".into(),
                    expected_revision: 1,
                    relation: MemoryRelation::new(
                        MemoryRelationKind::RelatedTo,
                        "candidate-related",
                    ),
                },
                MemoryOperation::AddRelation {
                    node_id: "candidate-related".into(),
                    expected_revision: 1,
                    relation: MemoryRelation::new(MemoryRelationKind::RelatedTo, "rollback-index"),
                },
            ],
        ))
        .await
        .expect("relate");

    let candidates: Vec<crate::durable_memory::RecallCandidate> =
        crate::DurableMemorySession::active_recall(
            repository,
            namespace,
            crate::DurableMemoryRecallPolicy::try_new(4, 0.2)
                .expect("policy")
                .try_with_related_lookups(2)
                .expect("related"),
        )
        .query_recall_candidates("deployment rollback", CancellationToken::new())
        .await
        .expect("recall");
    assert!(candidates
        .iter()
        .any(|candidate| candidate.node.id == "rollback-index"));
    assert!(candidates
        .iter()
        .all(|candidate| candidate.node.id != "candidate-related"));
    row("context_memory", model, "candidate-related-absent");
}

fn search_config(endpoint: &str) -> crate::config::SearchConfig {
    let mut engines = std::collections::HashMap::new();
    engines.insert(
        "tavily".to_string(),
        crate::config::SearchEngineConfig {
            enabled: true,
            weight: 1.0,
            timeout: None,
            api_key: None,
            project: None,
            endpoint: Some(endpoint.to_string()),
        },
    );
    crate::config::SearchConfig {
        timeout: 5,
        health: None,
        cascade_order: None,
        engines,
        headless: None,
    }
}

async fn case_web_search(model: &str) {
    let mut rejected = a3s_search::Search::new();
    let failure = crate::tools::add_http_engine(
        &mut rejected,
        "tavily",
        None,
        Some(&search_config("http://example.com/search")),
    )
    .expect_err("non-loopback http");
    assert_eq!(failure.engine, "tavily");
    assert_eq!(failure.provider.as_deref(), Some("tavily"));
    let mut accepted = a3s_search::Search::new();
    let added = crate::tools::add_http_engine(
        &mut accepted,
        "tavily",
        None,
        Some(&search_config("https://example.com/search")),
    )
    .expect("https endpoint");
    assert!(added);
    row("web_search", model, "EngineFailure-tavily");
}

async fn case_web_fetch(model: &str) {
    use crate::tools::Tool;

    let dir = workspace_dir();
    crate::tools::set_resolution_script(Some(crate::tools::ResolutionScript::PrivateOnly));
    let result = crate::tools::WebFetchTool
        .execute(
            &json!({"url": "https://example.com/deep-e2e"}),
            &crate::tools::ToolContext::new(dir.path().to_path_buf()),
        )
        .await
        .expect("fetch output");
    crate::tools::set_resolution_script(None);
    assert!(!result.success);
    assert!(result.images.is_empty());
    assert!(!result.content.contains("Example Domain"));
    row("web_fetch", model, "SafeHttpError");
}

async fn case_code_intelligence(model: &str) {
    use crate::WorkspaceCodeIntelligence;

    let dir = workspace_dir();
    let backend = crate::workspace::ManifestWorkspaceBackend::new(dir.path());
    let manifest = backend.manifest();
    let file_system: Arc<dyn crate::workspace::WorkspaceFileSystem> = backend;
    let provider = crate::LocalCodeIntelligence::start("deep-e2e", manifest, file_system)
        .await
        .expect("code intelligence");
    let (sender, mut receiver) =
        tokio::sync::watch::channel(crate::CodeIntelligenceStatus::default());
    provider.spawn_status_forwarder(u64::MAX, &mut receiver);
    let mut leaked = crate::CodeIntelligenceStatus::default();
    leaked.message = Some("deep-e2e-must-not-publish".to_string());
    sender.send_replace(leaked);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let observed = provider.subscribe_status();
    assert_ne!(
        observed.borrow().message.as_deref(),
        Some("deep-e2e-must-not-publish")
    );
    provider.shutdown().await;
    row("code_intelligence", model, "status-not-published");
}

fn cognitive_binding(content_digest: &str) -> crate::CognitivePackageBindingV1 {
    crate::cognitive_context::package_binding_for_generation(
        "0.1.0",
        7,
        "sha256:aa0beeb62f1b7b21bf70f21e6f0e858a1e4b720d313f0907209b5b9dad2eeb20",
        content_digest,
    )
}

async fn case_cognitive_packages(model: &str) {
    let store = crate::InMemoryRunStore::new();
    let run = store.create_run("session-1", "bind").await;
    let binding = cognitive_binding(
        "sha256:1def786da6d190b7b3ce0176e71d99ff1cac3f8c8cc7c0f8b76a893c544e7a90",
    );
    let admitted = store
        .bind_cognitive_package(&run.id, binding.clone())
        .await
        .expect("admit");
    assert_eq!(admitted.event_count, 0);
    assert!(matches!(admitted.status, crate::RunStatus::Created));
    let replayed = store
        .bind_cognitive_package(&run.id, binding.clone())
        .await
        .expect("replay");
    assert_eq!(
        replayed.cognitive_package_binding,
        admitted.cognitive_package_binding
    );
    let conflict = store
        .bind_cognitive_package(
            &run.id,
            cognitive_binding(
                "sha256:1def786da6d190b7b3ce0176e71d99ff1cac3f8c8cc7c0f8b76a893c544e7a91",
            ),
        )
        .await;
    assert!(matches!(
        conflict,
        Err(crate::run::RunCognitiveBindingError::Conflict)
    ));
    let stored = store.snapshot(&run.id).await.expect("snapshot");
    assert_eq!(
        stored.cognitive_package_binding,
        admitted.cognitive_package_binding
    );
    assert_eq!(stored.event_count, 0);
    row("cognitive_packages", model, "Conflict");
}

fn use_projection() -> crate::UseRuntimeTaskProjectionV1 {
    crate::UseRuntimeTaskProjectionV1 {
        tool_name: "use_tool_research_convert_0123456789abcdef".to_owned(),
        surface_id: "convert".to_owned(),
        command: "acme-convert".to_owned(),
        json_output: true,
        timeout_ms: 30_000,
        scope: crate::UsePlanScope {
            kind: crate::UsePlanScopeKind::Workspace,
            id: "workspace:fixture".to_owned(),
        },
        lifecycle_identity: crate::UseProjectedLifecycleIdentity {
            package_id: "acme/research".to_owned(),
            package_digest: format!("sha256:{}", "a".repeat(64)),
            manifest_digest: format!("sha256:{}", "b".repeat(64)),
            generation: 7,
        },
        provider_id: "test-runtime".to_owned(),
    }
}

fn use_execution(generation: u64) -> crate::UseRuntimeTaskExecutionV1 {
    crate::UseRuntimeTaskExecutionV1 {
        schema: crate::USE_RUNTIME_TASK_RESULT_SCHEMA.to_owned(),
        package_id: "acme/research".to_owned(),
        surface_id: "convert".to_owned(),
        lifecycle_generation: generation,
        provider_id: "test-runtime".to_owned(),
        exit_code: 0,
        stdout: "converted".to_owned(),
        stderr: "fixture warning".to_owned(),
        truncated: false,
    }
}

async fn case_use_runtime_tasks(model: &str) {
    let projection = use_projection();
    projection.validate().expect("projection");
    use_execution(7)
        .validate_for(&projection)
        .expect("matching execution");
    let drifted = use_execution(8).validate_for(&projection);
    assert!(matches!(
        drifted,
        Err(crate::UseRuntimeTaskError::ResponseDrift(_))
    ));
    row("use_runtime_tasks", model, "ResponseDrift");
}

async fn case_program(model: &str) {
    use crate::tools::{ToolInvocation, ToolInvoker, ToolResult};

    struct NoInvoker;

    #[async_trait::async_trait]
    impl ToolInvoker for NoInvoker {
        async fn invoke(
            &self,
            _invocation: ToolInvocation,
            _ctx: &crate::tools::ToolContext,
        ) -> ToolResult {
            ToolResult::error("none", "unused".to_string())
        }
        fn available_tools(&self) -> Vec<String> {
            Vec::new()
        }
    }

    let dir = workspace_dir();
    let ctx = crate::tools::ToolContext::new(dir.path().to_path_buf());
    let invoker: Arc<dyn ToolInvoker> = Arc::new(NoInvoker);
    let finished_source = json!({
        "source": "async function run() { return 1 + 2; }",
        "limits": {"timeoutMs": 1000}
    });
    let finished = crate::tools::execute_script_program(
        &finished_source,
        json!({}),
        Arc::clone(&invoker),
        &ctx,
    )
    .await
    .expect("script");
    assert!(finished.success, "program: finished script failed");
    assert!(finished.content.contains('3'));
    let timed_out = crate::tools::execute_script_program(
        &json!({
            "source": "async function run() { while (true) {} }",
            "limits": {"timeoutMs": 50}
        }),
        json!({}),
        Arc::clone(&invoker),
        &ctx,
    )
    .await
    .expect("timeout output");
    assert!(timed_out.content.contains("timed out"));
    let replayed = crate::tools::execute_script_program(&finished_source, json!({}), invoker, &ctx)
        .await
        .expect("replay");
    assert!(replayed.success, "program: replay failed");
    assert!(replayed.content.contains('3'));
    row("program", model, "script-timeout");
}

async fn case_programmable_workflows(model: &str) {
    use crate::store::SessionStore;
    use std::collections::HashMap;

    struct EchoExecutor {
        ran: Arc<tokio::sync::Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl crate::AgentExecutor for EchoExecutor {
        async fn execute_step(
            &self,
            spec: crate::AgentStepSpec,
            _event_tx: Option<tokio::sync::broadcast::Sender<crate::AgentEvent>>,
        ) -> crate::StepOutcome {
            self.ran.lock().await.push(spec.task_id.clone());
            crate::StepOutcome {
                task_id: spec.task_id.clone(),
                session_id: format!("task-run-{}", spec.task_id),
                agent: spec.agent.clone(),
                output: spec.prompt.clone(),
                success: true,
                structured: None,
                source_anchors: Vec::new(),
            }
        }
        fn concurrency_hint(&self) -> usize {
            4
        }
    }

    let exec = Arc::new(EchoExecutor {
        ran: Arc::new(tokio::sync::Mutex::new(Vec::new())),
    });
    let ran = Arc::clone(&exec.ran);
    let store: Arc<dyn SessionStore> = Arc::new(crate::store::MemorySessionStore::new());
    let workflow = crate::Workflow::builder(exec)
        .with_store(Arc::clone(&store))
        .with_root_id("root")
        .build();
    let mut done = HashMap::new();
    done.insert(
        "a".to_string(),
        crate::StepOutcome {
            task_id: "a".into(),
            session_id: "task-run-a".into(),
            agent: "explore".into(),
            output: "cached-a".into(),
            success: true,
            structured: None,
            source_anchors: Vec::new(),
        },
    );
    store
        .save_workflow_checkpoint(
            "root/0:implement",
            &crate::WorkflowCheckpoint::from_completed("root/0:implement", &done, 1),
        )
        .await
        .expect("checkpoint");
    let out = workflow
        .phase(
            "implement",
            vec![
                crate::AgentStepSpec::new("a", "explore", "d", "pa"),
                crate::AgentStepSpec::new("b", "review", "d", "pb"),
            ],
        )
        .await;
    assert_eq!(out[0].output, "cached-a");
    assert_eq!(*ran.lock().await, vec!["b".to_string()]);
    row("programmable_workflows", model, "cached-a");
}

#[cfg(feature = "state-graph")]
async fn case_state_graph(model: &str) {
    use crate::{GraphEvent, GraphPatch, GraphRuntime, PatchOperation};

    let mut runtime = GraphRuntime::new();
    assert!(runtime
        .propose_patch(
            GraphPatch::new(
                0,
                vec![PatchOperation::AddObject {
                    id: "a".into(),
                    object_type: "task".into(),
                    data: json!({}),
                }],
            ),
            None,
        )
        .expect("apply"));
    let version = runtime.graph().version();
    let hash = runtime.graph().state_hash().expect("hash");
    let rejected = runtime
        .propose_patch(
            GraphPatch::new(
                0,
                vec![
                    PatchOperation::AddObject {
                        id: "b".into(),
                        object_type: "task".into(),
                        data: json!({}),
                    },
                    PatchOperation::AddObject {
                        id: "c".into(),
                        object_type: "task".into(),
                        data: json!({}),
                    },
                ],
            ),
            None,
        )
        .expect("reject");
    assert!(!rejected);
    assert!(runtime.graph().object("b").is_none());
    assert!(runtime.graph().object("c").is_none());
    assert_eq!(runtime.graph().version(), version);
    assert_eq!(runtime.graph().state_hash().expect("hash"), hash);
    assert!(matches!(
        runtime.events().last().expect("event").event,
        GraphEvent::PatchRejected { .. }
    ));
    row("state_graph", model, "PatchRejected");
}

#[cfg(not(feature = "state-graph"))]
async fn case_state_graph(_model: &str) {
    panic!("feature required: state-graph");
}

#[cfg(feature = "evaluation")]
fn evaluation_record(decision: &str) -> crate::EvaluationRecordV1 {
    let result = crate::EvaluationResultV1::new(
        "fixture-evaluator",
        crate::ExecutionTargetV1::new("session-1", "run-1"),
        "aux-1",
        decision,
        json!({"issues": []}),
        format!("sha256:{}", "c".repeat(64)),
    )
    .expect("result");
    crate::EvaluationRecordV1::new(result, 1).expect("record")
}

#[cfg(feature = "evaluation")]
async fn case_evaluation_substrate(model: &str) {
    use crate::EvaluationResultSink;

    let store = crate::InMemoryEvaluationResultStore::new();
    let original = evaluation_record("observed");
    let digest = original.record_digest.clone();
    let first = store.write(original.clone()).await.expect("write");
    assert!(first.written && !first.replayed);
    let replay = store.write(original.clone()).await.expect("replay");
    assert!(!replay.written && replay.replayed);
    let conflict = store.write(evaluation_record("inconclusive")).await;
    assert!(matches!(
        conflict,
        Err(crate::EvaluationStoreError::Conflict)
    ));
    let kept = store.get(&digest).await.expect("first record stays");
    assert_eq!(kept.record_digest, digest);
    let again = store.write(original).await.expect("replay after conflict");
    assert!(!again.written && again.replayed);
    row("evaluation_substrate", model, "Conflict");
}

#[cfg(not(feature = "evaluation"))]
async fn case_evaluation_substrate(_model: &str) {
    panic!("feature required: evaluation");
}

#[cfg(feature = "apofasi")]
async fn case_typed_decisions(model: &str) {
    let planning = crate::admit_planning_pre_analysis();
    crate::enforce_ineligible(planning).expect("ineligible stays");
    let err = crate::enforce_ineligible(crate::GenerationAdmission {
        eligible: true,
        reason: "single typed answer",
    })
    .expect_err("eligible cannot replace");
    assert!(matches!(err, crate::TypedDecisionError::CannotReplace(_)));
    row("typed_decisions", model, "CannotReplace");
}

#[cfg(not(feature = "apofasi"))]
async fn case_typed_decisions(_model: &str) {
    panic!("feature required: apofasi");
}

#[cfg(feature = "headless-search")]
async fn case_moli_runtime(model: &str) {
    use fs2::FileExt;

    let dir = workspace_dir();
    let lock_path = dir.path().join(".install.lock");
    let held = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("lock file");
    held.try_lock_exclusive()
        .expect("test holds the install lock");
    let contended = tokio::time::timeout(
        Duration::from_secs(2),
        crate::moli_runtime::acquire_install_lock(
            dir.path(),
            Instant::now() + Duration::from_millis(30),
        ),
    )
    .await;
    let denied = match contended {
        Ok(Err(_)) => true,
        Ok(Ok(file)) => {
            drop(file);
            crate::moli_runtime::acquire_install_lock(
                dir.path(),
                Instant::now() - Duration::from_secs(1),
            )
            .await
            .is_err()
        }
        Err(_) => false,
    };
    drop(held);
    assert!(denied, "moli_runtime: install lock was not denied");
    row("moli_runtime", model, "install-lock-denied");
}

#[cfg(not(feature = "headless-search"))]
async fn case_moli_runtime(_model: &str) {
    panic!("feature required: headless-search");
}

#[cfg(feature = "s3")]
async fn case_s3_workspace(model: &str) {
    use crate::workspace::{
        WorkspaceError, WorkspaceFileSystem, WorkspaceFileSystemExt, WorkspacePath,
    };
    use wiremock::matchers::{header_exists, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"etag-1\"")
                .insert_header("content-type", "text/plain")
                .insert_header("content-length", "10")
                .insert_header("x-amz-request-id", "deep-e2e")
                .set_body_string("alpha-body"),
        )
        .mount(&server)
        .await;
    Mock::given(method("HEAD"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"etag-1\"")
                .insert_header("content-type", "text/plain")
                .insert_header("content-length", "10")
                .insert_header("x-amz-request-id", "deep-e2e"),
        )
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(header_exists("if-match"))
        .respond_with(
            ResponseTemplate::new(412)
                .insert_header("content-type", "application/xml")
                .insert_header("x-amz-request-id", "deep-e2e")
                .set_body_string(
                    r#"<?xml version="1.0"?><Error><Code>PreconditionFailed</Code></Error>"#,
                ),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"etag-1\"")
                .insert_header("x-amz-request-id", "deep-e2e"),
        )
        .with_priority(2)
        .mount(&server)
        .await;

    let backend = crate::S3WorkspaceBackend::new(
        crate::S3BackendConfig::new("deep-e2e", "prefix", "test-key", "test-secret")
            .endpoint(server.uri())
            .region("us-east-1")
            .force_path_style(true),
    );
    let path = WorkspacePath::from_normalized("notes/alpha.txt");
    backend.write_text(&path, "alpha-body").await.expect("put");
    let (text, _version) = backend.read_text_with_version(&path).await.expect("get");
    assert_eq!(text, "alpha-body");
    let err = backend
        .write_text_if_version(&path, "beta", "\"not-the-etag\"")
        .await
        .expect_err("stale etag");
    match err {
        WorkspaceError::VersionConflict(conflict) => assert!(conflict.actual.is_none()),
        other => panic!("s3_workspace: expected VersionConflict, got {other:?}"),
    }
    let (again, _) = backend
        .read_text_with_version(&path)
        .await
        .expect("get again");
    assert_eq!(again, "alpha-body");
    row("s3_workspace", model, "VersionConflict");
}

#[cfg(not(feature = "s3"))]
async fn case_s3_workspace(_model: &str) {
    panic!("feature required: s3");
}

#[cfg(feature = "telemetry")]
async fn case_opentelemetry(model: &str) {
    let provider = opentelemetry_sdk::trace::TracerProvider::builder().build();
    crate::telemetry_otel::shutdown_provider(provider);
    let mut guard = crate::telemetry_otel::TelemetryGuard::from_provider(
        opentelemetry_sdk::trace::TracerProvider::builder().build(),
    );
    assert!(guard.holds_provider());
    guard.shutdown_held();
    assert!(!guard.holds_provider());
    row("opentelemetry", model, "shutdown-empty-slot");
}

#[cfg(not(feature = "telemetry"))]
async fn case_opentelemetry(_model: &str) {
    panic!("feature required: telemetry");
}

fn git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("-c")
        .arg("commit.gpgsign=false")
        .args(args)
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?} failed");
}

fn init_repo(root: &Path) {
    std::fs::create_dir_all(root).expect("repo");
    git(root, &["init"]);
    git(root, &["config", "user.email", "a3s@example.com"]);
    git(root, &["config", "user.name", "a3s"]);
    std::fs::write(root.join("README.md"), "source\n").expect("readme");
    git(root, &["add", "README.md"]);
    git(root, &["commit", "-m", "init"]);
}

async fn case_effect_isolation(model: &str) {
    let parent = workspace_dir();
    let root = parent.path().join("repo");
    init_repo(&root);
    let session_id = format!(
        "de2e{}{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    );
    let binding = crate::effect_isolation::bind(&session_id, &root, true)
        .await
        .expect("bind");
    let worktree = binding.worktree_path.clone();
    std::fs::write(worktree.join("from-isolate.txt"), "isolated\n").expect("isolate file");
    std::fs::write(root.join("README.md"), "moved\n").expect("source move");
    git(&root, &["add", "README.md"]);
    git(&root, &["commit", "-m", "move"]);
    let outcome = crate::effect_isolation::promote_current(&session_id).expect("promote");
    assert!(matches!(
        outcome,
        crate::effect_isolation::PromoteOutcome::Conflict { .. }
    ));
    assert!(!root.join("from-isolate.txt").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("README.md")).expect("readme"),
        "moved\n"
    );
    crate::effect_isolation::discard(&session_id)
        .await
        .expect("discard");
    assert!(!worktree.exists());
    let orphans = std::fs::read_dir(parent.path())
        .expect("parent")
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".a3s-isolate-")
        });
    assert!(!orphans, "isolation directory remained");
    row(
        "Effect isolation / orphan `.a3s-isolate-*`",
        model,
        "PromoteOutcome-Conflict",
    );
}

async fn case_native_sandbox(model: &str) {
    use crate::sandbox::BashSandbox;

    let dir = workspace_dir();
    let marker = dir.path().join("deep-e2e-sandbox-marker");
    let sandbox = crate::agent_api::select_default_local_sandbox(
        dir.path(),
        false,
        Err(anyhow::anyhow!("native unavailable")),
    );
    let failed = BashSandbox::exec_command(
        sandbox.as_ref(),
        "touch deep-e2e-sandbox-marker",
        &dir.path().to_string_lossy(),
    )
    .await;
    assert!(failed.is_err());
    assert!(!marker.exists());
    row(
        "Native sandbox and process-host opt-in",
        model,
        "sandbox-denied",
    );
}

async fn case_safe_http(model: &str) {
    use crate::tools::{get_with_redirects_observed, RedirectQueryPolicy, ResolutionScript};

    let url = reqwest::Url::parse("https://example.com/deep-e2e").expect("url");
    for script in [
        ResolutionScript::FakeIpDohFail,
        ResolutionScript::FakeIpPrivateDoh,
    ] {
        crate::tools::set_resolution_script(Some(script));
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            get_with_redirects_observed(
                url.clone(),
                Some("http://127.0.0.1:9"),
                reqwest::header::HeaderMap::new(),
                2,
                RedirectQueryPolicy::Preserve,
            ),
        )
        .await;
        crate::tools::set_resolution_script(None);
        match result {
            Ok(Err(error)) => {
                let _typed: &crate::tools::SafeHttpError = &error;
            }
            Ok(Ok(_)) => panic!("safe_http: script {script:?} opened a response"),
            Err(_) => panic!("safe_http: script {script:?} hung"),
        }
    }
    row("Safe HTTP / Fake-IP / SSRF", model, "SafeHttpError");
}

async fn case_agent_release_contract(model: &str) {
    use crate::release::{
        AgentReleaseCapability, AgentReleaseCompatibility, AgentReleaseError, AgentReleaseManifest,
        AGENT_PROTOCOL_V1,
    };

    const RAW_FIXTURE: &str = include_str!("../../fixtures/agent-release-contract/.a3s/asset.acl");
    let fixture = RAW_FIXTURE.replace("\r\n", "\n").replace('\r', "\n");
    let manifest = AgentReleaseManifest::parse(&fixture).expect("manifest");
    let digest = manifest.artifact().digest().to_string();
    let mismatch = AgentReleaseCompatibility::new(
        "a3s.code.agent.v2",
        [AgentReleaseCapability::new("runtime.service", 1).expect("capability")],
    )
    .expect("mismatch compatibility");
    let err = manifest
        .verify_compatibility(&mismatch)
        .expect_err("protocol");
    assert!(matches!(err, AgentReleaseError::IncompatibleProtocol));
    assert_eq!(manifest.artifact().digest(), digest);
    let matching = AgentReleaseCompatibility::new(
        AGENT_PROTOCOL_V1,
        [
            AgentReleaseCapability::new("runtime.service", 1).expect("capability"),
            AgentReleaseCapability::new("secrets.external", 1).expect("capability"),
            AgentReleaseCapability::new("workspace.local", 2).expect("capability"),
        ],
    )
    .expect("matching compatibility");
    manifest
        .verify_compatibility(&matching)
        .expect("compatible");
    assert_eq!(manifest.artifact().digest(), digest);
    row("agent_release_contract", model, "IncompatibleProtocol");
}

fn protocol_identity(run_id: &str) -> crate::AgentProtocolRunIdentityV1 {
    crate::AgentProtocolRunIdentityV1 {
        schema: crate::AgentProtocolRunIdentityV1::SCHEMA.into(),
        protocol: crate::AGENT_PROTOCOL_V1.into(),
        agent_release_identity: format!("sha256:{}", "a".repeat(64)),
        session_id: "conversation-bound-meta".into(),
        run_id: run_id.into(),
    }
}

fn protocol_record(
    identity: &crate::AgentProtocolRunIdentityV1,
) -> crate::AgentProtocolEventRecordV1 {
    crate::AgentProtocolEventRecordV1 {
        sequence: 0,
        occurred_at_ms: 1,
        event: crate::EventEnvelopeV1::new("text_delta", json!({"text": "ok"})).with_metadata(
            json!({
                "session_id": identity.session_id,
                "run_id": identity.run_id,
                "sequence": 0u64,
                "timestamp_ms": 1u64
            }),
        ),
    }
}

async fn case_agent_protocol(model: &str) {
    let identity = protocol_identity("run-bound-meta");
    identity.validate().expect("identity");
    let record = protocol_record(&identity);
    record.validate_for(&identity).expect("match");
    record.validate_for(&identity).expect("replay");
    let swapped = protocol_identity("other-run");
    let err = record.validate_for(&swapped).expect_err("mismatch");
    assert!(matches!(err, crate::AgentProtocolError::IdentityMismatch));
    record.validate_for(&identity).expect("record unchanged");
    row("agent_protocol", model, "IdentityMismatch");
}

async fn case_mutation_verify_gate(model: &str) {
    use crate::harness_loop::{
        decide_completion, CompletionGate, CompletionTerminal, MutationLedger,
    };
    use crate::verification::{VerificationCheck, VerificationReport, VerificationStatus};

    let mut ledger = MutationLedger::default();
    ledger.observe_tool(
        "write",
        0,
        Some(&json!({"file_path": "src/lib.rs", "success": true})),
    );
    assert!(matches!(
        decide_completion(&ledger, &[], &[], true),
        CompletionGate::Incomplete { .. }
    ));
    let report = VerificationReport::new(
        "edit",
        vec![VerificationCheck::required("build", "command", "compiles")
            .with_status(VerificationStatus::Passed)],
    )
    .with_effect_digest(ledger.digest());
    match decide_completion(&ledger, &[report], &[], false) {
        CompletionGate::Allow(CompletionTerminal::Verified { effect_digest }) => {
            assert_eq!(effect_digest, ledger.digest());
        }
        other => panic!("mutation gate: expected verified, got {other:?}"),
    }
    let unbound = VerificationReport::new("edit", vec![]).with_effect_digest(ledger.digest());
    assert!(matches!(
        decide_completion(&ledger, &[unbound], &[], false),
        CompletionGate::Incomplete { .. }
    ));
    row("Mutation verify gate", model, "Incomplete-then-Verified");
}

async fn case_batch_schema(model: &str) {
    use crate::tools::Tool;

    let dir = workspace_dir();
    let tool = crate::tools::BatchTool::new(Arc::new(crate::tools::ToolRegistry::new(
        dir.path().to_path_buf(),
    )));
    let parameters = Tool::parameters(&tool);
    assert!(!json_has_key(&parameters, "examples"));
    assert!(parameters.to_string().contains("maxItems"));
    let invocations = (0..=crate::tools::MAX_BATCH_INVOCATIONS)
        .map(|_| json!({"tool": "noop", "args": {}}))
        .collect::<Vec<_>>();
    let result = tool
        .execute(
            &json!({"invocations": invocations}),
            &crate::tools::ToolContext::new(dir.path().to_path_buf()),
        )
        .await
        .expect("batch output");
    assert!(!result.success);
    assert!(result.content.contains("at most"));
    row(
        "`batch` schema pin (no application `$ref` in `examples`)",
        model,
        "maxItems-no-examples",
    );
}

const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
    0x00, 0x00, 0x03, 0x00, 0x01, 0x00, 0x05, 0xFE, 0x02, 0xFE, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

async fn case_image_read(model: &str) {
    let dir = workspace_dir();
    std::fs::write(dir.path().join("shot.png"), PNG).expect("png");
    std::fs::write(dir.path().join("note.txt"), "hello").expect("text");
    let ctx = crate::tools::ToolContext::new(dir.path().to_path_buf());
    let ranged = crate::tools::read_image_file(
        "shot.png",
        &json!({"file_path": "shot.png", "offset": 1}),
        &ctx,
    );
    assert!(!ranged.success);
    assert!(ranged.images.is_empty());
    let text = crate::tools::read_image_file("note.txt", &json!({"file_path": "note.txt"}), &ctx);
    assert!(text.images.is_empty());
    let image = crate::tools::read_image_file("shot.png", &json!({"file_path": "shot.png"}), &ctx);
    assert_eq!(image.images.len(), 1);
    assert_eq!(image.images[0].media_type, "image/png");
    assert_eq!(image.images[0].data, PNG);
    row("Image `read` attachments", model, "image-png");
}

async fn case_event_envelope(model: &str) {
    let marker = "DEEP-E2E-PAYLOAD-MARKER".repeat(4_000);
    let identity = protocol_identity("run-bound-meta");
    let mut oversized = protocol_record(&identity);
    oversized.event = crate::EventEnvelopeV1::new("text_delta", json!({"text": marker}))
        .with_metadata(json!({
            "session_id": identity.session_id,
            "run_id": identity.run_id,
            "sequence": 0u64,
            "timestamp_ms": 1u64
        }));
    crate::agent_protocol::bound_projected_event_record(&mut oversized).expect("stub payload");
    let encoded = serde_json::to_string(&oversized).expect("encode");
    assert!(!encoded.contains(&marker));
    let mut huge_identity = protocol_record(&identity);
    huge_identity.event = crate::EventEnvelopeV1::new("text_delta", json!({"text": "ok"}))
        .with_metadata(json!({
            "session_id": "s".repeat(2_000_000),
            "run_id": identity.run_id,
            "sequence": 0u64,
            "timestamp_ms": 1u64
        }));
    let err = crate::agent_protocol::bound_projected_event_record(&mut huge_identity)
        .expect_err("identity metadata stays");
    assert!(matches!(
        err,
        crate::AgentProtocolError::InvalidField("event")
    ));
    row(
        "Event envelope and oversized projection",
        model,
        "InvalidField-event",
    );
}

#[cfg(feature = "research")]
async fn case_research_wire(model: &str) {
    let core = crate::CoreEventIdentity::from_agent_event(
        crate::CoreIdentity::new(
            crate::OperationId::new("session-1/run-1").expect("operation"),
            crate::SourceRevision::new(4),
            Some(
                crate::CapabilityStamp::new(2, format!("sha256:{}", "b".repeat(64)))
                    .expect("stamp"),
            ),
            crate::EvidenceCursor::new(6),
        ),
        42,
        &crate::AgentEvent::TextDelta {
            text: "finding".to_string(),
        },
    )
    .expect("core event");
    let projected =
        crate::ResearchEventV1::from_core_event("project-1", 3, &core).expect("project");
    let mut tampered = core.clone();
    tampered.event_digest = format!("sha256:{}", "0".repeat(64));
    let err =
        crate::ResearchEventV1::from_core_event("project-1", 3, &tampered).expect_err("tamper");
    assert!(matches!(
        err,
        crate::ResearchContractError::InvalidField("coreEvent")
    ));
    let replayed = crate::ResearchEventV1::from_core_event("project-1", 3, &core).expect("replay");
    assert_eq!(replayed.payload_digest, projected.payload_digest);
    assert_eq!(replayed.event_digest, projected.event_digest);
    row("research wire", model, "payload-digest");
}

#[cfg(not(feature = "research"))]
async fn case_research_wire(_model: &str) {
    panic!("feature required: research");
}
