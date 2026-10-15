//! Real-model scenarios for the public deep cases.
//!
//! Each test admits one pinned Flash completion, then asserts the shipped
//! fail-closed result. Assistant text is not the pass condition.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use a3s_memory::repository::MemoryRepository;
use async_trait::async_trait;
use serde_json::json;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::deep38_support::{complete, pass};
use crate::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingProvider, EmbeddingProviderDescriptor,
    EmbeddingProviderError, EmbeddingVector,
};
use crate::hitl::{ConfirmationPolicy, ConfirmationProvider, ConfirmationResponse};
use crate::store::SessionStore;
use crate::tools::{TaskExecutor, TaskTool, Tool, ToolExecutor};
use crate::{
    Agent, AgentEvent, AgentExecutor, AgentRegistry, AgentRunSpawn, AgentStepSpec, CodeError,
    InMemoryRunStore, LlmClient, Message, OpenAiClient, RunControlError, RunControlRequest,
    SessionOptions, StepOutcome, ToolErrorKind, WorkflowBuilder, WorkflowCheckpoint,
};

const ISOLATION_ID: &str = "Effect isolation / orphan `.a3s-isolate-*`";
const BATCH_ID: &str = "`batch` schema pin (no application `$ref` in `examples`)";
const IMAGE_ID: &str = "Image `read` attachments";

#[tokio::test]
async fn deep38_agent_runtime() {
    let config = crate::deep38_support::load_pinned_config();
    let model = crate::deep38_support::pinned_model(&config);
    let agent = Agent::from_config(config)
        .await
        .expect("build real-provider agent");
    let workspace = tempfile::tempdir().expect("workspace");
    let options = SessionOptions::new()
        .with_session_id("deep38-agent-runtime")
        .with_model(model.clone())
        .with_max_tool_rounds(1)
        .with_temperature(0.0);
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().as_ref(),
            Some(options.clone()),
        )
        .await
        .expect("admit the session");
    tokio::time::timeout(
        Duration::from_secs(180),
        session.send("Reply with the single word ready.", None),
    )
    .await
    .expect("model completion timed out")
    .expect("model completion");
    agent.close().await;
    let again = agent
        .session_async(workspace.path().to_string_lossy().as_ref(), Some(options))
        .await;
    match again {
        Err(CodeError::SessionClosed { .. }) => {}
        other => panic!("expected SessionClosed from create_session, got {other:?}"),
    }
    pass("agent_runtime", &model, "SessionClosed");
}

#[tokio::test]
async fn deep38_conversation() {
    let model = complete().await;
    let config = crate::deep38_support::load_pinned_config();
    let agent = Agent::from_config(config).await.expect("agent");
    let workspace = tempfile::tempdir().expect("workspace");
    let options = SessionOptions::new()
        .with_session_id("deep38-conversation")
        .with_model(model.clone())
        .with_max_tool_rounds(1)
        .with_temperature(0.0);
    let session = agent
        .session_async(workspace.path().to_string_lossy().as_ref(), Some(options))
        .await
        .expect("session");
    let started = session
        .spawn_run_with_id("deep38-run", "alpha")
        .await
        .expect("first spawn stores the run identity");
    let replay = session
        .spawn_run_with_id("deep38-run", "alpha")
        .await
        .expect("matching prompt returns the stored spawn");
    assert!(
        replay.replayed(),
        "matching prompt must replay the stored spawn"
    );
    let conflict = match session.spawn_run_with_id("deep38-run", "beta").await {
        Ok(_) => panic!("a different prompt must fail closed"),
        Err(error) => error,
    };
    assert!(matches!(conflict, CodeError::RunIdentityConflict { .. }));
    if let AgentRunSpawn::Started { worker, .. } = started {
        worker.abort();
        let _ = worker.await;
    }
    agent.close().await;
    pass("conversation", &model, "RunIdentityConflict");
}

#[tokio::test]
async fn deep38_run_control() {
    let model = complete().await;
    let inbox =
        crate::run_control::RunControlInbox::new("deep38", "run-1", CancellationToken::new());
    inbox.update_turn(1).await;
    inbox
        .submit(
            RunControlRequest::steer("run-1", "continue").with_expected_turn("turn-1", 1),
            1,
        )
        .await
        .expect("current turn is admitted");
    inbox.update_turn(2).await;
    let stale = inbox
        .submit(
            RunControlRequest::steer("run-1", "later").with_expected_turn("turn-1", 1),
            2,
        )
        .await
        .expect_err("stale turn must be rejected");
    assert!(matches!(stale, RunControlError::StaleTurn { .. }));
    assert_eq!(inbox.snapshot().await.queued_controls, 1);
    pass("run_control", &model, "StaleTurn");
}

#[tokio::test]
async fn deep38_governed_tools() {
    let model = complete().await;
    let config = crate::deep38_support::load_pinned_config();
    let agent = Agent::from_config(config).await.expect("agent");
    let workspace = tempfile::tempdir().expect("workspace");
    let options = SessionOptions::new()
        .with_session_id("deep38-governed")
        .with_model(model.clone())
        .with_max_tool_rounds(1)
        .with_temperature(0.0);
    let session = agent
        .session_async(workspace.path().to_string_lossy().as_ref(), Some(options))
        .await
        .expect("session");
    session.close().await;
    let error = session
        .governed_tool(
            "write",
            json!({"file_path": "ghost.txt", "content": "must-not-land"}),
        )
        .await
        .expect_err("closed session must reject the tool");
    assert!(matches!(error, CodeError::SessionClosed { .. }));
    assert!(!workspace.path().join("ghost.txt").exists());
    agent.close().await;
    pass("governed_tools", &model, "SessionClosed");
}

#[tokio::test]
async fn deep38_workspace_retrieval() {
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let model = complete().await;
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(
        workspace.path().join("notes.txt"),
        "semantic fixture token\n",
    )
    .expect("seed");
    let provider: Arc<dyn EmbeddingProvider> = Arc::new(ImmediateVectors {
        inputs: Arc::new(AtomicUsize::new(0)),
    });
    let config = crate::deep38_support::load_pinned_config();
    let agent = Agent::from_config(config).await.expect("agent");
    let options = SessionOptions::new()
        .with_session_id("deep38-retrieval")
        .with_model(model.clone())
        .with_workspace_retrieval(crate::WorkspaceRetrievalOptions::new(provider));
    let session = agent
        .session_async(workspace.path().to_string_lossy().as_ref(), Some(options))
        .await
        .expect("retrieval session");
    tokio::time::timeout(Duration::from_secs(30), async {
        while session.workspace_retrieval_status().phase != crate::WorkspaceRetrievalPhase::Ready {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("semantic index did not become ready");

    let zero = session
        .semantic_search(
            crate::WorkspaceSemanticSearchRequest::new("semantic fixture").with_limit(0),
        )
        .await
        .expect_err("a zero limit must be InvalidQuery");
    assert!(
        matches!(zero, crate::WorkspaceRetrievalError::InvalidQuery(ref message) if message.contains("limit")),
        "{zero:?}"
    );
    let first = session
        .semantic_search(
            crate::WorkspaceSemanticSearchRequest::new("semantic fixture").with_limit(1),
        )
        .await
        .expect("non-zero search");
    let again = session
        .semantic_search(
            crate::WorkspaceSemanticSearchRequest::new("semantic fixture").with_limit(1),
        )
        .await
        .expect("replay");
    assert!(
        !first.hits.is_empty(),
        "non-zero limit must return the indexed hit"
    );
    assert_eq!(first.hits.len(), again.hits.len());
    assert_eq!(
        first.hits[0].chunk.path.as_ref(),
        again.hits[0].chunk.path.as_ref()
    );
    session.close().await;
    agent.close().await;
    pass("workspace_retrieval", &model, "InvalidQuery");
}

#[tokio::test]
async fn deep38_model_adapters() {
    let model = complete().await;
    let server = crate::deep38_support::StatusHttp::spawn(
        402,
        "Payment Required",
        "application/json",
        "{\"error\":{\"message\":\"payment required\"}}",
    )
    .await;
    let client = OpenAiClient::new("test-key".to_string(), "deepseek-v4-flash".to_string())
        .with_base_url(format!("http://{}", server.addr))
        .with_provider_name("openai")
        .with_retry_config(crate::retry::RetryConfig::disabled());
    let error = tokio::time::timeout(
        Duration::from_secs(20),
        LlmClient::complete(&client, &[Message::user("go")], None, &[]),
    )
    .await
    .expect("402 response timed out")
    .expect_err("HTTP 402 must fail closed");
    let typed = error
        .downcast_ref::<crate::llm::NonRetryableLlmError>()
        .expect("402 must be NonRetryableLlmError");
    assert_eq!(typed.provider(), Some("openai"));
    assert_eq!(typed.status(), Some(402));
    pass("model_adapters", &model, "NonRetryableLlmError");
}

#[tokio::test]
async fn deep38_mcp_and_skills() {
    let config = crate::deep38_support::load_pinned_config();
    let model = crate::deep38_support::pinned_model(&config);
    let agent = Agent::from_config(config).await.expect("agent");
    let workspace = tempfile::tempdir().expect("workspace");
    let server_dir = tempfile::tempdir().expect("mcp server dir");
    let server_path = server_dir.path().join("deep38_mcp.py");
    std::fs::write(
        &server_path,
        r#"
import json, sys
for raw in sys.stdin:
    raw = raw.strip()
    if not raw:
        continue
    msg = json.loads(raw)
    method = msg.get("method")
    ident = msg.get("id")
    if method == "notifications/initialized":
        continue
    if method == "initialize":
        out = {"jsonrpc":"2.0","id":ident,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"deep38","version":"1"}}}
    elif method == "tools/list":
        out = {"jsonrpc":"2.0","id":ident,"result":{"tools":[{"name":"ping","description":"deep38 ping","inputSchema":{"type":"object"}}]}}
    else:
        continue
    print(json.dumps(out), flush=True)
"#,
    )
    .expect("write mcp server");
    let server_name = "deep38";
    let tool_name = "mcp__deep38__ping";
    let options = SessionOptions::new()
        .with_session_id("deep38-mcp")
        .with_model(model.clone())
        .with_max_tool_rounds(1)
        .with_temperature(0.0);
    let session = agent
        .session_async(workspace.path().to_string_lossy().as_ref(), Some(options))
        .await
        .expect("session");
    let before_tools = session.tool_names();
    assert!(
        !before_tools.iter().any(|name| name == tool_name),
        "the server tool must be absent before admission"
    );
    let admitted = session
        .add_mcp_server(mcp_server(server_name, &server_path))
        .await
        .expect("open session admits the isolated MCP server");
    assert_eq!(admitted, 1, "the server lists one tool");
    let admitted_tools: Vec<_> = session
        .tool_names()
        .into_iter()
        .filter(|name| !before_tools.iter().any(|existing| existing == name))
        .collect();
    assert_eq!(
        admitted_tools,
        vec![tool_name.to_string()],
        "add_mcp_server lists only that server's tool"
    );
    let completed = tokio::time::timeout(
        Duration::from_secs(180),
        session.send("Reply with the single word ready.", None),
    )
    .await
    .expect("model completion timed out")
    .expect("model completion");
    assert!(
        !completed.messages.is_empty(),
        "the completed turn must be retained before removal"
    );
    let history_after_turn = serde_json::to_value(session.history()).expect("history");
    assert!(
        history_after_turn
            .to_string()
            .contains("Reply with the single word ready."),
        "history must name the completed turn"
    );
    session
        .remove_mcp_server(server_name)
        .await
        .expect("remove admitted server");
    assert!(
        !session.tool_names().iter().any(|name| name == tool_name),
        "the tool name is absent after removal"
    );
    assert_eq!(
        serde_json::to_value(session.history()).expect("history"),
        history_after_turn,
        "removing the server must not rewrite history"
    );
    session.close().await;
    let history_after_close = serde_json::to_value(session.history()).expect("history");
    let error = session
        .add_mcp_server(mcp_server(server_name, &server_path))
        .await
        .expect_err("closed session must reject MCP admission");
    assert!(matches!(error, CodeError::SessionClosed { .. }));
    assert_eq!(
        serde_json::to_value(session.history()).expect("history"),
        history_after_close,
        "SessionClosed must not rewrite history"
    );
    assert!(
        !session.tool_names().iter().any(|name| name == tool_name),
        "the closed admission must not restore the tool"
    );
    agent.close().await;
    pass("mcp_and_skills", &model, "SessionClosed");
}

#[tokio::test]
async fn deep38_planning_delegation() {
    let model = complete().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let llm: Arc<dyn LlmClient> = Arc::new(OpenAiClient::new(
        "unused".to_string(),
        "unused".to_string(),
    ));
    let tracker = Arc::new(crate::subagent_task_tracker::InMemorySubagentTaskTracker::new());
    let tool = TaskTool::new(Arc::new(
        TaskExecutor::new(
            Arc::new(AgentRegistry::new()),
            llm,
            workspace.path().display().to_string(),
        )
        .with_subagent_tracker(Arc::clone(&tracker)),
    ));
    let ctx = crate::tools::ToolContext::new(workspace.path().to_path_buf());
    let task = json!({"agent": "general", "description": "d", "prompt": "p"});
    let mut over_cap = Vec::new();
    for index in 0..33 {
        over_cap.push(json!({
            "agent": "general",
            "description": "d",
            "prompt": format!("p{index}")
        }));
    }
    let cases = [
        (
            "timeout_ms must be at least 1",
            json!({"tasks": [task.clone()], "timeout_ms": 0}),
        ),
        ("accepts at most 32 tasks", json!({"tasks": over_cap})),
        (
            "cannot set background=true",
            json!({"tasks": [
                {"agent": "general", "description": "d", "prompt": "p", "background": true},
                task.clone()
            ]}),
        ),
        (
            "min_success_count must be between 1 and the task count",
            json!({"tasks": [task.clone()], "allow_partial_failure": true, "min_success_count": 0}),
        ),
        (
            "min_success_count must be between 1 and the task count",
            json!({"tasks": [task.clone(), task.clone()], "allow_partial_failure": true, "min_success_count": 3}),
        ),
        (
            "min_success_count requires allow_partial_failure=true",
            json!({"tasks": [task.clone()], "min_success_count": 1}),
        ),
        ("Invalid task parameters", json!({"tasks": "not-an-array"})),
        ("task requires at least 1 task", json!({"tasks": []})),
    ];
    for (needle, args) in cases {
        let output = tool
            .execute(&args, &ctx)
            .await
            .expect("delegation returns a tool output");
        assert!(
            matches!(
                output.error_kind,
                Some(ToolErrorKind::InvalidArgument { .. })
            ),
            "{needle}: {output:?}"
        );
        assert!(
            output.content.contains(needle),
            "{needle} missing from {}",
            output.content
        );
        assert!(tracker.list().await.is_empty(), "{needle} started a child");
    }
    pass("planning_delegation", &model, "InvalidArgument");
}

#[tokio::test]
async fn deep38_persistence() {
    let config = crate::deep38_support::load_pinned_config();
    let model = crate::deep38_support::pinned_model(&config);
    let agent = Agent::from_config(config).await.expect("agent");
    let workspace = tempfile::tempdir().expect("workspace");
    let session_id = format!("deep38-persistence-{}", uuid::Uuid::new_v4());
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let stored: Arc<dyn SessionStore> = store.clone();
    let options = SessionOptions::new()
        .with_session_id(session_id.clone())
        .with_model(model.clone())
        .with_max_tool_rounds(1)
        .with_temperature(0.0)
        .with_session_store(Arc::clone(&stored));
    let session = agent
        .session_async(workspace.path().to_string_lossy().as_ref(), Some(options))
        .await
        .expect("session");
    tokio::time::timeout(
        Duration::from_secs(180),
        session.send("Reply with the single word ready.", None),
    )
    .await
    .expect("model completion timed out")
    .expect("completed turn");
    session
        .save()
        .await
        .expect("save snapshot after the completed turn");
    let saved = store
        .load_snapshot(&session_id)
        .await
        .expect("load saved snapshot")
        .expect("snapshot was written");
    let saved_digest = crate::store::snapshot_content_digest(&saved).expect("saved digest");
    let saved_artifacts = artifact_digests(&saved);
    assert_eq!(saved.session.id, session_id);
    assert!(
        !saved.run_records.is_empty(),
        "the checkpoint must name the completed run"
    );
    let run = saved.run_records[0].snapshot.clone();
    assert_eq!(run.session_id, session_id);
    assert_eq!(run.prompt, "Reply with the single word ready.");
    assert!(!run.id.is_empty());
    let saved_messages = serde_json::to_value(&saved.session.messages).expect("messages");
    session.close().await;
    drop(session);
    let resumed = agent
        .resume_session_async(
            &session_id,
            SessionOptions::new().with_session_store(Arc::clone(&stored)),
        )
        .await
        .expect("resume from the saved snapshot");
    let loaded = store
        .load_snapshot(&session_id)
        .await
        .expect("load resumed snapshot")
        .expect("resumed snapshot");
    let resumed_digest = crate::store::snapshot_content_digest(&loaded).expect("resumed digest");
    assert_eq!(resumed_digest, saved_digest, "resumed snapshot digest");
    assert_eq!(artifact_digests(&loaded), saved_artifacts);
    assert_eq!(
        serde_json::to_value(resumed.history()).expect("resumed history"),
        saved_messages,
        "resume must restore the saved conversation"
    );
    let resumed_run = resumed.run_snapshot(&run.id).await.expect("resumed run");
    assert_eq!(resumed_run.id, run.id);
    assert_eq!(resumed_run.session_id, run.session_id);
    assert_eq!(resumed_run.prompt, run.prompt);
    assert_eq!(resumed_run.status, run.status);
    resumed.close().await;

    let rejecting: Arc<dyn SessionStore> = Arc::new(RejectingSnapshotStore);
    let rejecting_session = agent
        .session_async(
            workspace.path().to_string_lossy().as_ref(),
            Some(
                SessionOptions::new()
                    .with_session_id(format!(
                        "deep38-persistence-reject-{}",
                        uuid::Uuid::new_v4()
                    ))
                    .with_model(model.clone())
                    .with_session_store(rejecting),
            ),
        )
        .await
        .expect("rejecting session");
    let error = rejecting_session
        .save()
        .await
        .expect_err("snapshot-incapable store fails");
    match error {
        CodeError::Session(message) => {
            assert!(
                message.contains("does not support aggregate session snapshots"),
                "{message}"
            );
        }
        other => panic!("expected CodeError::Session, got {other:?}"),
    }
    rejecting_session.close().await;
    agent.close().await;
    pass("persistence", &model, "CodeError::Session");
}

#[tokio::test]
async fn deep38_governance() {
    let model = complete().await;
    let config = crate::deep38_support::load_pinned_config();
    let agent = Agent::from_config(config).await.expect("agent");
    let workspace = tempfile::tempdir().expect("workspace");
    let pending_id = "deep38-pending";
    let completing = Arc::new(CompletingConfirmation::new(pending_id));
    let admitted = agent
        .session_async(
            workspace.path().to_string_lossy().as_ref(),
            Some(
                SessionOptions::new()
                    .with_session_id("deep38-governance-ok")
                    .with_model(model.clone())
                    .with_confirmation_manager(
                        Arc::clone(&completing) as Arc<dyn ConfirmationProvider>
                    ),
            ),
        )
        .await
        .expect("session");
    let completed = admitted
        .confirm_tool_use(pending_id, true, None)
        .await
        .expect("completed pending id");
    assert!(completed, "a completed pending id returns true");
    assert_eq!(completing.seen(), vec![pending_id.to_string()]);
    admitted.close().await;

    let failing = agent
        .session_async(
            workspace.path().to_string_lossy().as_ref(),
            Some(
                SessionOptions::new()
                    .with_session_id("deep38-governance-err")
                    .with_model(model.clone())
                    .with_confirmation_manager(Arc::new(FailingConfirmation)),
            ),
        )
        .await
        .expect("session");
    let error = failing
        .confirm_tool_use("tool-1", true, None)
        .await
        .expect_err("confirmation failure is a session error");
    match error {
        CodeError::Session(message) => assert!(message.contains("deep38"), "{message}"),
        other => panic!("expected CodeError::Session, got {other:?}"),
    }
    failing.close().await;
    agent.close().await;
    pass("governance", &model, "CodeError::Session");
}

#[tokio::test]
async fn deep38_run_observability() {
    let model = complete().await;
    let log = crate::CoreEventLog::new();
    let operation = crate::OperationId::new("session-1/run-1").unwrap();
    let revision = crate::SourceRevision::new(7);
    let stamp = Some(
        crate::CapabilityStamp::new(
            3,
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap(),
    );
    let first = crate::RunEventRecord {
        sequence: 0,
        timestamp_ms: 1_000,
        event: AgentEvent::TurnStart { turn: 4 },
    };
    let admitted = log
        .append_run_event(operation.clone(), revision, stamp.clone(), &first)
        .expect("first cursor");
    assert!(admitted.appended && !admitted.replayed);
    let replay = log
        .append_run_event(operation.clone(), revision, stamp.clone(), &first)
        .expect("identical record replays");
    assert!(!replay.appended && replay.replayed);
    let page = log
        .page(&operation, None, 8)
        .expect("page")
        .expect("retained journal");
    assert_eq!(
        page.entries.len(),
        1,
        "a replay must not write a second copy"
    );
    let stored = &page.entries[0].event.identity;
    assert_eq!(stored.operation_id, operation);
    assert_eq!(stored.source_revision, revision);
    assert_eq!(stored.capability_stamp, stamp);
    assert_eq!(stored.evidence_cursor.sequence(), 0);
    let rendered = serde_json::to_string(&page).expect("page json");
    assert!(
        !rendered.contains("prompt"),
        "the retained journal stores identities, not prompt plaintext: {rendered}"
    );
    let retained_digest = page.entries[0].entry_digest.clone();
    let second = crate::RunEventRecord {
        sequence: 0,
        timestamp_ms: 1_000,
        event: AgentEvent::TurnStart { turn: 5 },
    };
    let error = log
        .append_run_event(operation.clone(), revision, stamp, &second)
        .expect_err("same cursor with a different payload conflicts");
    assert!(matches!(error, crate::CoreEventLogError::CursorConflict));
    let after = log
        .page(&operation, None, 8)
        .expect("page")
        .expect("journal remains");
    assert_eq!(after.entries.len(), 1);
    assert_eq!(after.entries[0].entry_digest, retained_digest);
    pass("run_observability", &model, "CursorConflict");
}

#[tokio::test]
async fn deep38_context_memory() {
    let model = complete().await;
    let repository = Arc::new(a3s_memory::repository::InMemoryRepository::new());
    let namespace =
        a3s_memory::repository::MemoryNamespace::try_new("tenant", "principal", "scope").unwrap();
    repository
        .apply(a3s_memory::repository::MemoryChangeSet::new(
            "seed-deep38",
            namespace.clone(),
            memory_time(1),
            vec![
                a3s_memory::repository::MemoryOperation::Create {
                    node: a3s_memory::repository::MemoryNodeDraft::new(
                        "rollback-index",
                        namespace.clone(),
                        a3s_memory::repository::DurableMemoryKind::Semantic,
                        a3s_memory::repository::MemoryStatus::Active,
                        "Deployment rollback playbook index",
                        vec![memory_evidence(
                            "index-verification",
                            a3s_memory::repository::EvidenceKind::Verification,
                            1,
                        )],
                        memory_time(1),
                    )
                    .with_relation(
                        a3s_memory::repository::MemoryRelation::new(
                            a3s_memory::repository::MemoryRelationKind::RelatedTo,
                            "canary-procedure",
                        ),
                    ),
                },
                a3s_memory::repository::MemoryOperation::Create {
                    node: a3s_memory::repository::MemoryNodeDraft::new(
                        "canary-procedure",
                        namespace.clone(),
                        a3s_memory::repository::DurableMemoryKind::Procedural,
                        a3s_memory::repository::MemoryStatus::Active,
                        "Drain the first ring before shifting production traffic",
                        vec![memory_evidence(
                            "canary-verification",
                            a3s_memory::repository::EvidenceKind::Verification,
                            1,
                        )],
                        memory_time(1),
                    )
                    .with_relation(
                        a3s_memory::repository::MemoryRelation::new(
                            a3s_memory::repository::MemoryRelationKind::RelatedTo,
                            "rollback-index",
                        ),
                    ),
                },
                a3s_memory::repository::MemoryOperation::Create {
                    node: a3s_memory::repository::MemoryNodeDraft::new(
                        "candidate-related",
                        namespace.clone(),
                        a3s_memory::repository::DurableMemoryKind::Procedural,
                        a3s_memory::repository::MemoryStatus::Candidate,
                        "Unverified recovery shortcut",
                        vec![memory_evidence(
                            "candidate-proposal",
                            a3s_memory::repository::EvidenceKind::SessionTurn,
                            1,
                        )],
                        memory_time(1),
                    ),
                },
            ],
        ))
        .await
        .unwrap();
    repository
        .apply(a3s_memory::repository::MemoryChangeSet::new(
            "attach-candidate",
            namespace.clone(),
            memory_time(2),
            vec![
                a3s_memory::repository::MemoryOperation::AddRelation {
                    node_id: "rollback-index".into(),
                    expected_revision: 1,
                    relation: a3s_memory::repository::MemoryRelation::new(
                        a3s_memory::repository::MemoryRelationKind::RelatedTo,
                        "candidate-related",
                    ),
                },
                a3s_memory::repository::MemoryOperation::AddRelation {
                    node_id: "candidate-related".into(),
                    expected_revision: 1,
                    relation: a3s_memory::repository::MemoryRelation::new(
                        a3s_memory::repository::MemoryRelationKind::RelatedTo,
                        "rollback-index",
                    ),
                },
            ],
        ))
        .await
        .unwrap();
    let related = crate::DurableMemorySession::active_recall(
        repository,
        namespace,
        crate::DurableMemoryRecallPolicy::try_new(4, 0.2)
            .unwrap()
            .try_with_related_lookups(2)
            .unwrap(),
    )
    .preview_recall("deployment rollback")
    .await
    .unwrap();
    assert!(related
        .hits
        .iter()
        .any(|hit| hit.node_id == "canary-procedure"));
    assert!(related
        .hits
        .iter()
        .all(|hit| hit.node_id != "candidate-related"));
    pass("context_memory", &model, "MemoryStatus");
}

#[tokio::test]
async fn deep38_web_fetch() {
    const SECRET: &str = "BLOCKED-BODY-SECRET";
    let secret_hits = Arc::new(AtomicUsize::new(0));
    let secret_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("secret listener");
    let secret_addr = secret_listener.local_addr().expect("secret addr");
    let hits = Arc::clone(&secret_hits);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = secret_listener.accept().await else {
                break;
            };
            hits.fetch_add(1, Ordering::SeqCst);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{SECRET}",
                SECRET.len()
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, response.as_bytes()).await;
        }
    });

    let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("proxy listener");
    let proxy_addr = proxy_listener.local_addr().expect("proxy addr");
    let redirect = format!("http://{secret_addr}/secret");
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = proxy_listener.accept().await else {
                break;
            };
            let redirect = redirect.clone();
            tokio::spawn(async move {
                let Some((header, _)) = read_http_message(&mut socket).await else {
                    return;
                };
                let target = header.lines().next().unwrap_or("");
                // reqwest tunnels HTTPS, so this hop is the public HTTP URL.
                // The 302 is what get_with_redirects revalidates before connect.
                let response = if target.contains("example.com") && target.contains("/public") {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: {redirect}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                } else {
                    "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_string()
                };
                let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, response.as_bytes()).await;
            });
        }
    });

    let headless = crate::config::HeadlessConfig {
        proxy_url: Some(format!("http://{proxy_addr}")),
        ..Default::default()
    };
    let search = crate::config::SearchConfig {
        timeout: 20,
        health: None,
        cascade_order: None,
        engines: std::collections::HashMap::new(),
        headless: Some(headless),
    };
    let workspace = tempfile::tempdir().expect("workspace");
    let executor = ToolExecutor::new(workspace.path().display().to_string());
    let context =
        crate::tools::ToolContext::new(workspace.path().to_path_buf()).with_search_config(search);
    let result = executor
        .execute_with_context(
            "web_fetch",
            &json!({"url": "http://example.com/public", "timeout": 15}),
            &context,
        )
        .await
        .expect("web_fetch maps FetchFailure into a tool result");
    assert_ne!(result.exit_code, 0, "{}", result.output);
    assert!(
        !result.output.contains(SECRET),
        "blocked response body leaked: {}",
        result.output
    );
    assert!(
        result.output.contains("non-public") && result.output.contains("blocked"),
        "private redirect must fail closed: {}",
        result.output
    );
    assert_eq!(
        secret_hits.load(Ordering::SeqCst),
        0,
        "fetch_url must not open the private redirect"
    );
    let model = complete().await;
    pass("web_fetch", &model, "ToolOutput");
}

#[tokio::test]
async fn deep38_cognitive_packages() {
    let model = complete().await;
    let store = InMemoryRunStore::new();
    let run = store.create_run("session-1", "query knowledge").await;
    let binding = cognitive_binding();
    store
        .bind_cognitive_package(&run.id, binding.clone())
        .await
        .expect("first binding");
    store
        .bind_cognitive_package(&run.id, binding.clone())
        .await
        .expect("exact replay");
    let mut conflict = binding.clone();
    conflict.limits.max_results -= 1;
    conflict.validate().unwrap();
    let error = store
        .bind_cognitive_package(&run.id, conflict)
        .await
        .expect_err("a different binding conflicts");
    assert!(matches!(
        error,
        crate::run::RunCognitiveBindingError::Conflict
    ));
    let snapshot = store.snapshot(&run.id).await.expect("snapshot");
    assert_eq!(snapshot.cognitive_package_binding.as_ref(), Some(&binding));
    pass("cognitive_packages", &model, "RunCognitiveBindingError");
}

#[tokio::test]
async fn deep38_use_runtime_tasks() {
    let model = complete().await;
    let projection = use_projection();
    projection.validate().unwrap();
    let matching = use_execution(7);
    matching
        .validate_for(&projection)
        .expect("matching generation");
    let drifted = use_execution(8);
    let error = drifted
        .validate_for(&projection)
        .expect_err("generation drift");
    assert!(matches!(
        error,
        crate::UseRuntimeTaskError::ResponseDrift(_)
    ));
    pass("use_runtime_tasks", &model, "ResponseDrift");
}

#[tokio::test]
async fn deep38_program() {
    let model = complete().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let executor = ToolExecutor::new(workspace.path().display().to_string());
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        executor.execute(
            "program",
            &json!({
                "type": "script",
                "source": "async function run(){while(true){}}",
                "limits": {"timeoutMs": 200}
            }),
        ),
    )
    .await
    .expect("program timeout hung")
    .expect("program returns a tool result");
    assert_ne!(result.exit_code, 0);
    assert!(
        result
            .output
            .contains("program script timed out after 200 ms"),
        "{}",
        result.output
    );
    let finished = tokio::time::timeout(
        Duration::from_secs(10),
        executor.execute(
            "program",
            &json!({
                "type": "script",
                "source": "async function run(){ return { summary: \"deep38-original\" }; }",
                "limits": {"timeoutMs": 2000}
            }),
        ),
    )
    .await
    .expect("finished program hung")
    .expect("finished program returns a tool result");
    assert_eq!(finished.exit_code, 0, "{}", finished.output);
    assert!(
        finished.output.contains("deep38-original"),
        "original value missing from {}",
        finished.output
    );
    let metadata = finished.metadata.expect("script metadata");
    assert_eq!(metadata["script_result"]["summary"], "deep38-original");
    pass("program", &model, "TimedOut");
}

#[tokio::test]
async fn deep38_programmable_workflows() {
    let model = complete().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let executor: Arc<dyn AgentExecutor> = Arc::new(CountingExecutor {
        calls: Arc::clone(&calls),
    });
    let store: Arc<dyn SessionStore> = Arc::new(crate::store::MemorySessionStore::new());
    let mut completed = HashMap::new();
    completed.insert(
        "implement".to_string(),
        StepOutcome {
            task_id: "implement".into(),
            session_id: "task-run-implement".into(),
            agent: "general".into(),
            output: "cached-kernel".into(),
            success: true,
            structured: None,
            source_anchors: Vec::new(),
        },
    );
    let checkpoint = WorkflowCheckpoint::from_completed("root/0:implement", &completed, 1);
    store
        .save_workflow_checkpoint("root/0:implement", &checkpoint)
        .await
        .expect("seed checkpoint");
    let workflow = WorkflowBuilder::new(executor)
        .with_store(store)
        .with_root_id("root")
        .build();
    let outcomes = workflow
        .phase(
            "implement",
            vec![AgentStepSpec::new(
                "implement",
                "general",
                "implement",
                "do it",
            )],
        )
        .await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "cached step must skip the executor"
    );
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].output, "cached-kernel");
    assert!(outcomes[0].success);
    pass("programmable_workflows", &model, "StepOutcome");
}

#[cfg(feature = "state-graph")]
#[tokio::test]
async fn deep38_state_graph() {
    let model = complete().await;
    let mut runtime = crate::GraphRuntime::new();
    assert!(runtime
        .propose_patch(
            crate::GraphPatch::new(
                0,
                vec![crate::PatchOperation::AddObject {
                    id: "a".into(),
                    object_type: "task".into(),
                    data: json!({}),
                }],
            ),
            None,
        )
        .unwrap());
    let applied_version = runtime.graph().version();
    let applied_hash = runtime
        .graph()
        .state_hash()
        .expect("applied head state hash");
    let accepted = runtime
        .propose_patch(
            crate::GraphPatch::new(
                0,
                vec![crate::PatchOperation::AddObject {
                    id: "b".into(),
                    object_type: "task".into(),
                    data: json!({}),
                }],
            ),
            None,
        )
        .unwrap();
    assert!(!accepted);
    assert_eq!(
        runtime.graph().version(),
        applied_version,
        "PatchRejected must leave the applied graph version"
    );
    assert_eq!(
        runtime.graph().state_hash().expect("head state hash"),
        applied_hash,
        "PatchRejected must leave the applied state hash"
    );
    assert!(runtime.graph().object("b").is_none());
    assert!(matches!(
        runtime.events().last().unwrap().event,
        crate::GraphEvent::PatchRejected { .. }
    ));
    pass("state_graph", &model, "PatchRejected");
}

#[tokio::test]
async fn deep38_agent_release_contract() {
    let model = complete().await;
    let raw = include_str!("../../fixtures/agent-release-contract/.a3s/asset.acl");
    let manifest = crate::release::AgentReleaseManifest::parse(&raw.replace("\r\n", "\n")).unwrap();
    let declared = manifest.artifact().digest().to_string();
    assert_eq!(
        declared,
        "sha256:1111111111111111111111111111111111111111111111111111111111111111"
    );
    let wrong = crate::release::AgentReleaseCompatibility::new(
        "a3s.code.agent.v2",
        [
            crate::release::AgentReleaseCapability::new("runtime.service", 1).unwrap(),
            crate::release::AgentReleaseCapability::new("secrets.external", 1).unwrap(),
            crate::release::AgentReleaseCapability::new("workspace.local", 1).unwrap(),
        ],
    )
    .unwrap();
    let error = manifest
        .verify_compatibility(&wrong)
        .expect_err("protocol mismatch");
    assert!(matches!(
        error,
        crate::release::AgentReleaseError::IncompatibleProtocol
    ));
    assert_eq!(error.code(), "a3s.code.agent_release.incompatible_protocol");
    assert_eq!(
        manifest.artifact().digest(),
        declared,
        "protocol mismatch must leave the declared artifact digest"
    );
    pass("agent_release_contract", &model, "IncompatibleProtocol");
}

#[tokio::test]
async fn deep38_agent_protocol() {
    let model = complete().await;
    let identity = protocol_identity();
    let matching = protocol_record(&identity.session_id, &identity.run_id);
    matching.validate_for(&identity).expect("matching identity");
    matching
        .validate_for(&identity)
        .expect("replaying the same record stays accepted");
    let drifted = protocol_record(&identity.session_id, "other-run");
    let error = drifted.validate_for(&identity).expect_err("swapped run id");
    assert!(matches!(error, crate::AgentProtocolError::IdentityMismatch));
    pass("agent_protocol", &model, "IdentityMismatch");
}

#[cfg(feature = "evaluation")]
#[tokio::test]
async fn deep38_evaluation_substrate() {
    use crate::evaluation::EvaluationResultSink;
    let store = crate::InMemoryEvaluationResultStore::new();
    let target = crate::ExecutionTargetV1::new("session-1", "run-1");
    let first = crate::EvaluationRecordV1::new(
        evaluation_result(&target, json!({"issues": []}), b"fixture"),
        1,
    )
    .unwrap();
    let written = store.write(first.clone()).await.unwrap();
    assert!(written.written && !written.replayed);
    let replayed = store.write(first.clone()).await.unwrap();
    assert!(
        !replayed.written && replayed.replayed,
        "the identical write is a replay: {replayed:?}"
    );
    let second = crate::EvaluationRecordV1::new(
        evaluation_result(&target, json!({"issues": ["different"]}), b"other"),
        2,
    )
    .unwrap();
    assert_ne!(second.record_digest, first.record_digest);
    let error = store
        .write(second.clone())
        .await
        .expect_err("conflicting identity");
    assert!(matches!(error, crate::EvaluationStoreError::Conflict));
    let stored = store
        .get(&first.record_digest)
        .await
        .expect("Conflict must leave the first record stored");
    assert_eq!(stored, first);
    assert!(store.get(&second.record_digest).await.is_none());
    let model = complete().await;
    pass("evaluation_substrate", &model, "Conflict");
}

#[cfg(feature = "apofasi")]
#[tokio::test]
async fn deep38_typed_decisions() {
    let model = complete().await;
    assert!(crate::enforce_ineligible(crate::admit_planning_pre_analysis()).is_ok());
    let error = crate::enforce_ineligible(crate::GenerationAdmission {
        eligible: true,
        reason: "typed answer is the whole generation",
    })
    .expect_err("an eligible generation cannot be replaced here");
    assert!(matches!(error, crate::TypedDecisionError::CannotReplace(_)));
    pass("typed_decisions", &model, "CannotReplace");
}

#[cfg(feature = "s3")]
#[tokio::test]
async fn deep38_s3_workspace() {
    let addr = spawn_versioned_bucket().await;
    let mut config = crate::S3BackendConfig::new("deep38", "", "key", "secret")
        .endpoint(format!("http://{addr}"))
        .region("us-east-1");
    config.force_path_style = true;
    let backend = crate::S3WorkspaceBackend::new(config);
    let path = crate::WorkspacePath::from_normalized("notes.txt");
    let first = "alpha-kernel\n";
    let stale = "beta-should-not-stick\n";
    tokio::time::timeout(
        Duration::from_secs(15),
        crate::WorkspaceFileSystemExt::write_text_if_version(&backend, &path, first, "\"etag-1\""),
    )
    .await
    .expect("matching etag write timed out")
    .expect("matching etag stores the body");
    let error = tokio::time::timeout(
        Duration::from_secs(15),
        crate::WorkspaceFileSystemExt::write_text_if_version(&backend, &path, stale, "\"etag-1\""),
    )
    .await
    .expect("stale etag write timed out")
    .expect_err("stale etag must conflict");
    match error {
        crate::WorkspaceError::VersionConflict(conflict) => {
            assert_eq!(conflict.expected, "\"etag-1\"");
            assert!(conflict.actual.is_none());
            assert!(conflict.path.contains("notes.txt"), "{}", conflict.path);
        }
        other => panic!("expected WorkspaceVersionConflict, got {other:?}"),
    }
    let stored = tokio::time::timeout(
        Duration::from_secs(15),
        crate::WorkspaceFileSystem::read_text(&backend, &path),
    )
    .await
    .expect("read timed out")
    .expect("stored object");
    assert_eq!(
        crate::digest_bytes("a3s.code.deep38.s3-object.v1", stored.as_bytes()),
        crate::digest_bytes("a3s.code.deep38.s3-object.v1", first.as_bytes()),
        "stored body is {stored:?}"
    );
    let model = complete().await;
    pass("s3_workspace", &model, "WorkspaceVersionConflict");
}

#[tokio::test]
async fn deep38_effect_isolation() {
    let root = tempfile::tempdir().expect("root");
    let source = root.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    git(&source, &["init", "-b", "main"]);
    git(&source, &["config", "user.email", "a3s@example.com"]);
    git(&source, &["config", "user.name", "a3s"]);
    std::fs::write(source.join("README.md"), "source\n").unwrap();
    git(&source, &["add", "README.md"]);
    git(&source, &["commit", "-m", "init"]);
    let session = format!("deep38-iso-{}", uuid::Uuid::new_v4());
    let binding = crate::effect_isolation::bind(&session, &source, true)
        .await
        .expect("bind isolation worktree");
    assert!(binding
        .worktree_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".a3s-isolate-")));
    std::fs::write(binding.worktree_path.join("note.txt"), "isolated\n").unwrap();
    let first = crate::effect_isolation::promote_current(&session);
    std::fs::write(source.join("README.md"), "moved\n").unwrap();
    git(&source, &["add", "README.md"]);
    git(&source, &["commit", "-m", "move"]);
    std::fs::write(binding.worktree_path.join("extra.txt"), "drift\n").unwrap();
    let drifted = crate::effect_isolation::promote_current(&session);
    let discarded = crate::effect_isolation::discard(&session).await;
    match first.expect("first promote") {
        crate::effect_isolation::PromoteOutcome::Applied { digest } => {
            assert!(!digest.is_empty());
        }
        other => panic!("bound revision must apply once: {other:?}"),
    }
    match drifted.expect("drifted promote") {
        crate::effect_isolation::PromoteOutcome::Conflict {
            bound_revision,
            current_revision,
        } => assert_ne!(bound_revision, current_revision),
        other => panic!("expected PromoteOutcome::Conflict, got {other:?}"),
    }
    discarded.expect("discard isolate");
    assert!(
        !binding.worktree_path.exists(),
        "discard left {}",
        binding.worktree_path.display()
    );
    let parent = binding.worktree_path.parent().expect("isolate parent");
    let orphan = std::fs::read_dir(parent)
        .expect("list isolate parent")
        .any(|entry| {
            entry
                .ok()
                .and_then(|entry| entry.file_name().into_string().ok())
                .is_some_and(|name| name.starts_with(".a3s-isolate-"))
        });
    assert!(
        !orphan,
        "discarded isolate left an orphan .a3s-isolate-* directory"
    );
    let model = complete().await;
    pass(ISOLATION_ID, &model, "PromoteOutcome");
}

#[tokio::test]
async fn deep38_mutation_verify_gate() {
    let model = complete().await;
    let mut ledger = crate::harness_loop::MutationLedger::default();
    assert!(matches!(
        crate::harness_loop::decide_completion(&ledger, &[], &[], false),
        crate::harness_loop::CompletionGate::Allow(
            crate::harness_loop::CompletionTerminal::Narrative
        )
    ));
    ledger.observe_tool(
        "write",
        0,
        Some(&json!({"file_path": "src/lib.rs", "after": "fn main() {}"})),
    );
    assert!(matches!(
        crate::harness_loop::decide_completion(&ledger, &[], &[], false),
        crate::harness_loop::CompletionGate::Incomplete { .. }
    ));
    let digest = ledger.digest().to_string();
    assert!(!digest.is_empty());
    let passed = || {
        crate::verification::VerificationReport::new(
            "edit",
            vec![
                crate::verification::VerificationCheck::required("build", "command", "compiles")
                    .with_status(crate::verification::VerificationStatus::Passed),
            ],
        )
    };
    let unbound = passed();
    assert!(unbound.effect_digest.is_none());
    assert!(matches!(
        crate::harness_loop::decide_completion(&ledger, &[unbound], &[], false),
        crate::harness_loop::CompletionGate::Incomplete { .. }
    ));
    let mismatched = passed().with_effect_digest("sha256:not-the-ledger");
    assert!(matches!(
        crate::harness_loop::decide_completion(&ledger, &[mismatched], &[], false),
        crate::harness_loop::CompletionGate::Incomplete { .. }
    ));
    let report = passed().with_effect_digest(&digest);
    assert!(matches!(
        crate::harness_loop::decide_completion(&ledger, &[report], &[], false),
        crate::harness_loop::CompletionGate::Allow(
            crate::harness_loop::CompletionTerminal::Verified { .. }
        )
    ));
    pass("Mutation verify gate", &model, "Incomplete");
}

#[tokio::test]
async fn deep38_batch_schema_pin() {
    let model = complete().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let executor = ToolExecutor::new(workspace.path().display().to_string());
    let parameters = executor
        .registry()
        .get("batch")
        .expect("batch")
        .parameters();
    assert!(parameters.get("examples").is_none(), "{parameters}");
    let invocations = (0..33)
        .map(|index| json!({"tool": "read", "args": {"file_path": format!("f{index}.txt")}}))
        .collect::<Vec<_>>();
    let result = executor
        .execute("batch", &json!({"invocations": invocations}))
        .await
        .expect("over-cap batch returns a tool result");
    assert_ne!(result.exit_code, 0);
    assert!(result.output.contains("at most 32"), "{}", result.output);
    pass(BATCH_ID, &model, "MAX_BATCH_INVOCATIONS");
}

#[tokio::test]
async fn deep38_image_read() {
    let model = complete().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let png = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=",
    )
    .unwrap();
    std::fs::write(workspace.path().join("pixel.png"), &png).unwrap();
    std::fs::write(workspace.path().join("notes.txt"), "line\n").unwrap();
    let executor = ToolExecutor::new(workspace.path().display().to_string());
    let ranged = executor
        .execute("read", &json!({"file_path": "pixel.png", "offset": 0}))
        .await
        .expect("ranged image read");
    assert_ne!(ranged.exit_code, 0);
    assert!(ranged
        .output
        .contains("offset and limit apply to text files only"));
    assert!(ranged.images.is_empty());
    let text = executor
        .execute(
            "read",
            &json!({"file_path": "notes.txt", "offset": 0, "limit": 1}),
        )
        .await
        .expect("text read");
    assert!(
        text.images.is_empty(),
        "a text read must not become an image attachment"
    );
    let image = executor
        .execute("read", &json!({"file_path": "pixel.png"}))
        .await
        .expect("image read");
    assert_eq!(image.exit_code, 0, "{}", image.output);
    assert!(image
        .images
        .iter()
        .any(|image| image.media_type.starts_with("image/")));
    pass(IMAGE_ID, &model, "ToolOutput");
}

#[cfg(feature = "research")]
#[tokio::test]
async fn deep38_research_wire() {
    let model = complete().await;
    let core = crate::CoreEventIdentity::from_agent_event(
        crate::CoreIdentity::new(
            crate::OperationId::new("session-1/run-1").unwrap(),
            crate::SourceRevision::new(4),
            Some(
                crate::CapabilityStamp::new(
                    2,
                    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                )
                .unwrap(),
            ),
            crate::EvidenceCursor::new(6),
        ),
        42,
        &AgentEvent::TextDelta {
            text: "finding".into(),
        },
    )
    .unwrap();
    let projected = crate::ResearchEventV1::from_core_event("project-1", 3, &core).unwrap();
    assert_eq!(projected.payload_digest, core.payload_digest);
    assert_ne!(projected.event_digest, core.event_digest);
    assert!(projected.validate().is_ok());
    let mut broken = core.clone();
    broken.event_digest =
        "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".into();
    let error = crate::ResearchEventV1::from_core_event("project-1", 3, &broken)
        .expect_err("invalid core event");
    assert!(matches!(
        error,
        crate::ResearchContractError::InvalidField("coreEvent")
    ));
    pass("research wire", &model, "InvalidField");
}

fn protocol_identity() -> crate::AgentProtocolRunIdentityV1 {
    crate::AgentProtocolRunIdentityV1 {
        schema: crate::AgentProtocolRunIdentityV1::SCHEMA.into(),
        protocol: crate::AGENT_PROTOCOL_V1.into(),
        agent_release_identity: format!("sha256:{}", "a".repeat(64)),
        session_id: "conversation-bound-meta".into(),
        run_id: "run-bound-meta".into(),
    }
}

fn protocol_record(session_id: &str, run_id: &str) -> crate::AgentProtocolEventRecordV1 {
    crate::AgentProtocolEventRecordV1 {
        sequence: 0,
        occurred_at_ms: 1,
        event: crate::EventEnvelopeV1::new("text_delta", json!({"text": "ok"})).with_metadata(
            json!({
                "session_id": session_id,
                "run_id": run_id,
                "sequence": 0u64,
                "timestamp_ms": 1u64,
            }),
        ),
    }
}

fn mcp_server(name: &str, command: &std::path::Path) -> crate::mcp::McpServerConfig {
    crate::mcp::McpServerConfig {
        name: name.to_string(),
        transport: crate::mcp::McpTransportConfig::Stdio {
            command: "python3".to_string(),
            args: vec![command.display().to_string()],
        },
        enabled: true,
        env: Default::default(),
        oauth: None,
        tool_timeout_secs: 5,
    }
}

fn artifact_digests(snapshot: &crate::store::SessionSnapshotV1) -> Vec<String> {
    snapshot
        .artifacts
        .iter()
        .map(|artifact| {
            crate::digest_bytes("a3s.code.deep38.artifact.v1", artifact.content.as_bytes())
        })
        .collect()
}

fn use_projection() -> crate::UseRuntimeTaskProjectionV1 {
    crate::UseRuntimeTaskProjectionV1 {
        tool_name: "use_tool_research_convert_0123456789abcdef".into(),
        surface_id: "convert".into(),
        command: "acme-convert".into(),
        json_output: true,
        timeout_ms: 30_000,
        scope: crate::UsePlanScope {
            kind: crate::UsePlanScopeKind::Workspace,
            id: "workspace:fixture".into(),
        },
        lifecycle_identity: crate::UseProjectedLifecycleIdentity {
            package_id: "acme/research".into(),
            package_digest: format!("sha256:{}", "a".repeat(64)),
            manifest_digest: format!("sha256:{}", "b".repeat(64)),
            generation: 7,
        },
        provider_id: "test-runtime".into(),
    }
}

fn use_execution(generation: u64) -> crate::UseRuntimeTaskExecutionV1 {
    crate::UseRuntimeTaskExecutionV1 {
        schema: crate::USE_RUNTIME_TASK_RESULT_SCHEMA.into(),
        package_id: "acme/research".into(),
        surface_id: "convert".into(),
        lifecycle_generation: generation,
        provider_id: "test-runtime".into(),
        exit_code: 0,
        stdout: "{\"answer\":42}".into(),
        stderr: "fixture warning".into(),
        truncated: false,
    }
}

fn cognitive_binding() -> crate::CognitivePackageBindingV1 {
    let generation_digest =
        "sha256:aa0beeb62f1b7b21bf70f21e6f0e858a1e4b720d313f0907209b5b9dad2eeb20";
    let knowledge = crate::CognitiveKnowledgeBindingV1::new(
        "domain-knowledge",
        "0.2",
        "sha256:1def786da6d190b7b3ce0176e71d99ff1cac3f8c8cc7c0f8b76a893c544e7a90",
        7,
        generation_digest,
    )
    .unwrap();
    crate::CognitivePackageBindingV1::new(
        "contra-sense/handbook",
        "0.1.0",
        7,
        generation_digest,
        "sha256:1e0f0a0162f5b290887ade8886af69fbba4548c863df026178e3550c77813455",
        knowledge,
        crate::CognitiveContextLimits::default(),
    )
    .unwrap()
}

#[cfg(feature = "evaluation")]
fn evaluation_result(
    target: &crate::ExecutionTargetV1,
    payload: serde_json::Value,
    evidence: &[u8],
) -> crate::EvaluationResultV1 {
    crate::EvaluationResultV1::new(
        "fixture-evaluator",
        target.clone(),
        "aux-1",
        "inconclusive",
        payload,
        crate::evaluation::digest_bytes("evidence", evidence),
    )
    .unwrap()
}

fn memory_time(offset_seconds: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(1_777_000_000 + offset_seconds, 0).unwrap()
}

fn memory_evidence(
    name: &str,
    kind: a3s_memory::repository::EvidenceKind,
    offset_seconds: i64,
) -> a3s_memory::repository::EvidenceRef {
    a3s_memory::repository::EvidenceRef::try_new(
        format!("a3s://evidence/{name}"),
        format!("sha256:{name:0>64}"),
        kind,
        memory_time(offset_seconds),
    )
    .unwrap()
}

fn header_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case(name).then_some(value.trim())
    })
}

async fn read_line(socket: &mut tokio::net::TcpStream, pending: &mut Vec<u8>) -> Option<String> {
    use tokio::io::AsyncReadExt;
    loop {
        if let Some(pos) = pending.windows(2).position(|window| window == b"\r\n") {
            let line = String::from_utf8_lossy(&pending[..pos]).into_owned();
            pending.drain(..pos + 2);
            return Some(line);
        }
        let mut tmp = [0u8; 1024];
        let count = tokio::time::timeout(Duration::from_secs(8), socket.read(&mut tmp))
            .await
            .ok()?
            .ok()?;
        if count == 0 {
            return None;
        }
        pending.extend_from_slice(&tmp[..count]);
    }
}

async fn read_chunked(socket: &mut tokio::net::TcpStream, mut pending: Vec<u8>) -> Option<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut decoded = Vec::new();
    loop {
        let line = read_line(socket, &mut pending).await?;
        let size_text = line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16).ok()?;
        if size == 0 {
            loop {
                let trailer = read_line(socket, &mut pending).await?;
                if trailer.is_empty() {
                    break;
                }
            }
            break;
        }
        while pending.len() < size + 2 {
            let mut tmp = [0u8; 8192];
            let count = tokio::time::timeout(Duration::from_secs(8), socket.read(&mut tmp))
                .await
                .ok()?
                .ok()?;
            if count == 0 {
                return None;
            }
            pending.extend_from_slice(&tmp[..count]);
        }
        decoded.extend_from_slice(&pending[..size]);
        pending.drain(..size + 2);
    }
    Some(decoded)
}

async fn read_http_message(socket: &mut tokio::net::TcpStream) -> Option<(String, Vec<u8>)> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let header_end = loop {
        if let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > 1024 * 1024 {
            return None;
        }
        let count = match tokio::time::timeout(Duration::from_secs(8), socket.read(&mut tmp)).await
        {
            Ok(Ok(0)) | Err(_) | Ok(Err(_)) => return None,
            Ok(Ok(count)) => count,
        };
        buf.extend_from_slice(&tmp[..count]);
    };
    let header = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut body = buf[header_end..].to_vec();
    let expects_continue = header.lines().any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.starts_with("expect:") && lower.contains("100-continue")
    });
    if expects_continue {
        socket
            .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
            .await
            .ok()?;
    }
    if header_value(&header, "transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"))
    {
        body = read_chunked(socket, body).await?;
    } else if let Some(length) =
        header_value(&header, "content-length").and_then(|value| value.trim().parse::<usize>().ok())
    {
        while body.len() < length {
            let count =
                match tokio::time::timeout(Duration::from_secs(8), socket.read(&mut tmp)).await {
                    Ok(Ok(0)) | Err(_) | Ok(Err(_)) => break,
                    Ok(Ok(count)) => count,
                };
            body.extend_from_slice(&tmp[..count]);
        }
        body.truncate(length);
    }
    Some((header, body))
}

#[cfg(feature = "s3")]
async fn spawn_versioned_bucket() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("versioned bucket");
    let addr = listener.local_addr().expect("versioned bucket addr");
    let object = Arc::new(std::sync::Mutex::new((
        "etag-1".to_string(),
        b"seed-not-the-write".to_vec(),
    )));
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let object = Arc::clone(&object);
            tokio::spawn(async move {
                let Some((header, body)) = read_http_message(&mut socket).await else {
                    return;
                };
                let request = header.lines().next().unwrap_or("");
                let method = request.split_whitespace().next().unwrap_or("");
                let response = {
                    let mut object = object.lock().expect("object");
                    if method == "PUT" {
                        let if_match = header_value(&header, "if-match")
                            .unwrap_or("")
                            .trim()
                            .trim_matches('"');
                        if if_match == object.0 {
                            object.1 = body;
                            object.0 = "etag-2".to_string();
                            http_response(200, "OK", &format!("ETag: \"{}\"\r\n", object.0), &[])
                        } else {
                            let xml = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>PreconditionFailed</Code><Message>At least one of the preconditions you specified did not hold</Message><RequestId>deep38</RequestId></Error>";
                            http_response(
                                412,
                                "Precondition Failed",
                                "Content-Type: application/xml\r\n",
                                xml,
                            )
                        }
                    } else if method == "GET" || method == "HEAD" {
                        let payload = if method == "HEAD" {
                            Vec::new()
                        } else {
                            object.1.clone()
                        };
                        let length = object.1.len();
                        let headers = format!(
                            "ETag: \"{}\"\r\nContent-Type: text/plain; charset=utf-8\r\n",
                            object.0
                        );
                        let mut response = http_response(200, "OK", &headers, &payload);
                        if method == "HEAD" {
                            response = http_response_with_length(200, "OK", &headers, length);
                        }
                        response
                    } else {
                        http_response(405, "Method Not Allowed", "", &[])
                    }
                };
                let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, &response).await;
                let _ = tokio::io::AsyncWriteExt::shutdown(&mut socket).await;
            });
        }
    });
    addr
}

#[cfg(feature = "s3")]
fn http_response(status: u16, reason: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    http_response_with_length(status, reason, headers, body.len())
        .into_iter()
        .chain(body.iter().copied())
        .collect()
}

#[cfg(feature = "s3")]
fn http_response_with_length(status: u16, reason: &str, headers: &str, length: usize) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status} {reason}\r\n{headers}Content-Length: {length}\r\nx-amz-request-id: deep38\r\nx-amz-id-2: deep38\r\nConnection: close\r\n\r\n"
    )
    .into_bytes()
}

fn git(root: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?} failed");
}

struct ImmediateVectors {
    inputs: Arc<AtomicUsize>,
}

#[async_trait]
impl EmbeddingProvider for ImmediateVectors {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        EmbeddingProviderDescriptor::new("fixture", "immediate-v1", 2)
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        _cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        self.inputs
            .fetch_add(request.inputs().len(), Ordering::AcqRel);
        let vectors = request
            .inputs()
            .iter()
            .map(|input| EmbeddingVector::new(input.id(), vec![1.0, 0.0]))
            .collect();
        Ok(EmbeddingBatchResponse::new(self.descriptor(), vectors))
    }
}

struct RejectingSnapshotStore;

#[async_trait]
impl SessionStore for RejectingSnapshotStore {
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

struct CompletingConfirmation {
    pending: &'static str,
    seen: std::sync::Mutex<Vec<String>>,
}

impl CompletingConfirmation {
    fn new(pending: &'static str) -> Self {
        Self {
            pending,
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().expect("seen").clone()
    }
}

#[async_trait]
impl ConfirmationProvider for CompletingConfirmation {
    async fn requires_confirmation(&self, _tool_name: &str) -> bool {
        false
    }

    async fn request_confirmation(
        &self,
        _tool_id: &str,
        _tool_name: &str,
        _args: &serde_json::Value,
    ) -> tokio::sync::oneshot::Receiver<ConfirmationResponse> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = tx.send(ConfirmationResponse {
            approved: true,
            reason: None,
        });
        rx
    }

    async fn confirm(
        &self,
        tool_id: &str,
        approved: bool,
        _reason: Option<String>,
    ) -> Result<bool, String> {
        self.seen.lock().expect("seen").push(tool_id.to_string());
        Ok(approved && tool_id == self.pending)
    }

    async fn policy(&self) -> ConfirmationPolicy {
        ConfirmationPolicy {
            enabled: false,
            ..ConfirmationPolicy::default()
        }
    }

    async fn set_policy(&self, _policy: ConfirmationPolicy) {}

    async fn check_timeouts(&self) -> usize {
        0
    }

    async fn cancel_all(&self) -> usize {
        0
    }
}

struct FailingConfirmation;

#[async_trait]
impl ConfirmationProvider for FailingConfirmation {
    async fn requires_confirmation(&self, _tool_name: &str) -> bool {
        false
    }

    async fn request_confirmation(
        &self,
        _tool_id: &str,
        _tool_name: &str,
        _args: &serde_json::Value,
    ) -> tokio::sync::oneshot::Receiver<ConfirmationResponse> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = tx.send(ConfirmationResponse {
            approved: false,
            reason: None,
        });
        rx
    }

    async fn confirm(
        &self,
        _tool_id: &str,
        _approved: bool,
        _reason: Option<String>,
    ) -> Result<bool, String> {
        Err("deep38 confirmation store failed".to_string())
    }

    async fn policy(&self) -> ConfirmationPolicy {
        ConfirmationPolicy {
            enabled: false,
            ..ConfirmationPolicy::default()
        }
    }

    async fn set_policy(&self, _policy: ConfirmationPolicy) {}

    async fn check_timeouts(&self) -> usize {
        0
    }

    async fn cancel_all(&self) -> usize {
        0
    }
}

struct CountingExecutor {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl AgentExecutor for CountingExecutor {
    async fn execute_step(
        &self,
        spec: AgentStepSpec,
        _event_tx: Option<broadcast::Sender<AgentEvent>>,
    ) -> StepOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        StepOutcome::failed(spec.task_id, spec.agent, "cached step must not execute")
    }
}
