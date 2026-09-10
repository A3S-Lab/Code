use super::*;
use crate::hitl::ConfirmationPolicy;
use crate::llm::{Message, TokenUsage};
use crate::orchestration::{workflow_step_result_receipt, AgentStepSpec, ToolSourceAnchor};
use crate::permissions::PermissionPolicy;
use crate::prompts::PlanningMode;
use crate::queue::SessionQueueConfig;
use crate::run::RunRecord;
use crate::subagent_task_tracker::{SubagentStatus, SubagentTaskSnapshot};
use crate::tools::ArtifactStore;
use crate::trace::TraceEvent;
use crate::verification::VerificationReport;
use base64::Engine as _;
use tempfile::tempdir;

fn encoded_file_path(root: &std::path::Path, category: &str, id: &str) -> std::path::PathBuf {
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(id.as_bytes());
    root.join("v1")
        .join(category)
        .join(format!("id_{key}.json"))
}

fn create_test_session_data() -> SessionData {
    SessionData {
        id: "test-session-1".to_string(),
        config: SessionConfig {
            name: "Test Session".to_string(),
            workspace: "/tmp/workspace".to_string(),
            system_prompt: Some("You are helpful.".to_string()),
            max_context_length: 200000,
            auto_compact: false,
            auto_compact_threshold: DEFAULT_AUTO_COMPACT_THRESHOLD,
            storage_type: crate::config::StorageBackend::File,
            queue_config: None,
            confirmation_policy: None,
            permission_policy: None,
            enforce_active_skill_tool_restrictions: false,
            max_parallel_tasks: None,
            auto_delegation: None,
            parent_id: None,
            security_config: None,
            hook_engine: None,
            planning_mode: PlanningMode::default(),
            goal_tracking: false,
            tool_presentation_profile: crate::tools::ToolPresentationProfileV1::default(),
            tool_result_transform_policy: crate::tools::ToolResultTransformPolicyV1::default(),
        },
        state: SessionState::Active,
        messages: vec![
            Message::user("Hello"),
            Message {
                role: "assistant".to_string(),
                content: vec![crate::llm::ContentBlock::Text {
                    text: "Hi there!".to_string(),
                }],
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: Default::default(),
            },
        ],
        context_usage: ContextUsage {
            used_tokens: 100,
            max_tokens: 200000,
            percent: 0.0005,
            turns: 2,
        },
        total_usage: TokenUsage {
            prompt_tokens: 50,
            completion_tokens: 50,
            total_tokens: 100,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        tool_names: vec!["bash".to_string(), "read".to_string()],
        thinking_enabled: false,
        thinking_budget: None,
        created_at: 1700000000,
        updated_at: 1700000100,
        llm_config: None,
        tasks: vec![],
        parent_id: None,
        tenant_id: None,
        principal: None,
        agent_template_id: None,
        correlation_id: None,
        durable_memory_binding: None,
        cognitive_package_binding: None,
        immutable_content_adapter_binding: None,
        total_cost: 0.0,
        model_name: None,
        cost_records: Vec::new(),
    }
}

fn create_test_verification_report() -> VerificationReport {
    VerificationReport::new(
        "program:test",
        vec![
            crate::verification::VerificationCheck::required("check:test", "test", "Run tests")
                .with_status(crate::verification::VerificationStatus::Passed),
        ],
    )
}

async fn create_test_run_records() -> Vec<RunRecord> {
    let runs = crate::run::InMemoryRunStore::new();
    let run = runs.create_run("session/a", "persist run").await;
    runs.record_event(
        &run.id,
        crate::agent::AgentEvent::Start {
            prompt: "persist run".to_string(),
        },
    )
    .await;
    runs.records().await
}

async fn create_test_snapshot() -> SessionSnapshotV1 {
    let artifacts = ArtifactStore::new();
    artifacts.put(crate::tools::ToolArtifact {
        artifact_id: "tool-output:test:snapshot".to_string(),
        artifact_uri: "a3s://tool-output/test/snapshot".to_string(),
        tool_name: "test".to_string(),
        content: "snapshot artifact".to_string(),
        original_bytes: 17,
        shown_bytes: 17,
    });
    let trace_events = vec![TraceEvent::tool_execution(
        "read",
        true,
        0,
        std::time::Duration::from_millis(4),
        17,
        None,
    )];
    let subagent_tasks = vec![SubagentTaskSnapshot {
        task_id: "task-snapshot".to_string(),
        parent_session_id: "test-session-1".to_string(),
        child_session_id: "child-snapshot".to_string(),
        agent: "general".to_string(),
        description: "persist snapshot".to_string(),
        status: SubagentStatus::Completed,
        started_ms: 1,
        updated_ms: 2,
        finished_ms: Some(2),
        output: Some("done".to_string()),
        success: Some(true),
        source_anchors: vec![ToolSourceAnchor {
            tool: "read".to_string(),
            url_or_path: "docs/source.md".to_string(),
        }],
        progress: Vec::new(),
    }];

    SessionSnapshotV1::new(
        create_test_session_data(),
        &artifacts,
        trace_events,
        create_test_run_records().await,
        vec![create_test_verification_report()],
        subagent_tasks,
    )
}

#[tokio::test]
async fn snapshot_fork_rebinds_every_top_level_session_owner() {
    let mut snapshot = create_test_snapshot().await;
    for record in &mut snapshot.run_records {
        record.snapshot.session_id = snapshot.session.id.clone();
    }
    snapshot.validate_for_session("test-session-1").unwrap();

    let fork = snapshot
        .fork_for_session("fork-session", "/tmp/fork-workspace")
        .unwrap();

    assert_eq!(fork.session.id, "fork-session");
    assert_eq!(fork.session.config.workspace, "/tmp/fork-workspace");
    assert!(fork
        .run_records
        .iter()
        .all(|record| record.snapshot.session_id == "fork-session"));
    assert!(fork
        .subagent_tasks
        .iter()
        .all(|task| task.parent_session_id == "fork-session"));
    fork.validate_for_session("fork-session").unwrap();
}

// ========================================================================
// FileSessionStore Tests
// ========================================================================

#[tokio::test]
async fn test_file_store_save_and_load() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();

    let session = create_test_session_data();

    // Save
    store.save(&session).await.unwrap();

    // Load
    let loaded = store.load(&session.id).await.unwrap();
    assert!(loaded.is_some());

    let loaded = loaded.unwrap();
    assert_eq!(loaded.id, session.id);
    assert_eq!(loaded.config.name, session.config.name);
    assert_eq!(loaded.messages.len(), 2);
    assert_eq!(loaded.state, SessionState::Active);
}

#[tokio::test]
async fn file_store_commits_and_loads_one_complete_snapshot_generation() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();
    let snapshot = create_test_snapshot().await;

    store.save_snapshot(&snapshot).await.unwrap();

    assert!(store.capabilities().atomic_session_snapshots);
    let persisted_path = encoded_file_path(dir.path(), "sessions", "test-session-1");
    let persisted: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&persisted_path).await.unwrap()).unwrap();
    assert_eq!(persisted["schema_version"], SESSION_SNAPSHOT_SCHEMA_VERSION);
    assert_eq!(persisted["session"]["id"], "test-session-1");
    assert!(!dir.path().join("artifacts").exists());
    assert!(!dir.path().join("traces").exists());
    assert!(!dir.path().join("runs").exists());
    assert!(!dir.path().join("verification").exists());
    assert!(!dir.path().join("subagent_tasks").exists());
    assert_eq!(
        std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .count(),
        1,
        "successful atomic commit must not leave a temporary file"
    );

    let loaded = store
        .load_snapshot("test-session-1")
        .await
        .unwrap()
        .expect("snapshot");
    assert_eq!(loaded.session.config.name, "Test Session");
    assert_eq!(loaded.artifacts.len(), 1);
    assert_eq!(loaded.trace_events.len(), 1);
    assert_eq!(loaded.run_records.len(), 1);
    assert_eq!(loaded.verification_reports.len(), 1);
    assert_eq!(loaded.subagent_tasks.len(), 1);
    assert_eq!(
        loaded.subagent_tasks[0].source_anchors[0].url_or_path,
        "docs/source.md"
    );

    // The historical SessionStore::load API remains source-compatible and
    // projects the SessionData portion from an aggregate file.
    assert_eq!(
        store
            .load("test-session-1")
            .await
            .unwrap()
            .expect("session")
            .config
            .name,
        "Test Session"
    );
}

#[tokio::test]
async fn file_store_load_snapshot_falls_back_to_legacy_fragments() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();
    let snapshot = create_test_snapshot().await;
    let id = snapshot.session.id.clone();

    // Reproduce the pre-SnapshotV1 on-disk layout.
    tokio::fs::write(
        dir.path().join(format!("{id}.json")),
        serde_json::to_vec_pretty(&snapshot.session).unwrap(),
    )
    .await
    .unwrap();
    snapshot
        .artifact_store()
        .save_to_dir(dir.path().join("artifacts").join(&id))
        .unwrap();
    for (category, value) in [
        (
            "traces",
            serde_json::to_vec_pretty(&snapshot.trace_events).unwrap(),
        ),
        (
            "runs",
            serde_json::to_vec_pretty(&snapshot.run_records).unwrap(),
        ),
        (
            "verification",
            serde_json::to_vec_pretty(&snapshot.verification_reports).unwrap(),
        ),
        (
            "subagent_tasks",
            serde_json::to_vec_pretty(&snapshot.subagent_tasks).unwrap(),
        ),
    ] {
        let category_dir = dir.path().join(category);
        tokio::fs::create_dir_all(&category_dir).await.unwrap();
        tokio::fs::write(category_dir.join(format!("{id}.json")), value)
            .await
            .unwrap();
    }

    let loaded = store
        .load_snapshot(&id)
        .await
        .unwrap()
        .expect("legacy snapshot");
    assert_eq!(loaded.artifacts.len(), 1);
    assert_eq!(loaded.trace_events.len(), 1);
    assert_eq!(loaded.run_records.len(), 1);
    assert_eq!(loaded.verification_reports.len(), 1);
    assert_eq!(loaded.subagent_tasks.len(), 1);
    assert_eq!(
        loaded.subagent_tasks[0].source_anchors[0].url_or_path,
        "docs/source.md"
    );

    // A new aggregate generation is authoritative even when old fragment
    // files remain on disk. Empty fields must clear, not inherit, old state.
    let replacement = SessionSnapshotV1::session_only(snapshot.session);
    store.save_snapshot(&replacement).await.unwrap();
    let loaded = store
        .load_snapshot(&id)
        .await
        .unwrap()
        .expect("replacement snapshot");
    assert!(loaded.artifacts.is_empty());
    assert!(loaded.trace_events.is_empty());
    assert!(loaded.run_records.is_empty());
    assert!(loaded.verification_reports.is_empty());
    assert!(loaded.subagent_tasks.is_empty());
}

#[tokio::test]
async fn file_store_rejects_future_snapshot_versions() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();
    let mut snapshot = create_test_snapshot().await;
    snapshot.schema_version = SESSION_SNAPSHOT_SCHEMA_VERSION + 1;

    assert!(store.save_snapshot(&snapshot).await.is_err());

    // Simulate a snapshot written by a newer implementation to cover reads.
    let path = encoded_file_path(dir.path(), "sessions", "test-session-1");
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&path, serde_json::to_vec(&snapshot).unwrap())
        .await
        .unwrap();
    let error = store.load_snapshot("test-session-1").await.unwrap_err();
    assert!(error.to_string().contains("not loadable"));
}

#[tokio::test]
async fn test_file_store_load_nonexistent() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();

    let loaded = store.load("nonexistent").await.unwrap();
    assert!(loaded.is_none());
}

#[tokio::test]
async fn test_file_store_delete() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();

    let session = create_test_session_data();
    store.save(&session).await.unwrap();

    // Verify exists
    assert!(store.exists(&session.id).await.unwrap());

    // Delete
    store.delete(&session.id).await.unwrap();

    // Verify gone
    assert!(!store.exists(&session.id).await.unwrap());
    assert!(store.load(&session.id).await.unwrap().is_none());
}

#[tokio::test]
async fn test_file_store_save_and_load_artifacts() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();
    let artifacts = ArtifactStore::new();
    artifacts.put(crate::tools::ToolArtifact {
        artifact_id: "tool-output:test:a".to_string(),
        artifact_uri: "a3s://tool-output/test/a".to_string(),
        tool_name: "test".to_string(),
        content: "artifact content".to_string(),
        original_bytes: 16,
        shown_bytes: 4,
    });

    store.save_artifacts("session/a", &artifacts).await.unwrap();
    let loaded = store
        .load_artifacts("session/a")
        .await
        .unwrap()
        .expect("artifacts");

    assert_eq!(loaded.len(), 1);
    assert_eq!(
        loaded
            .get("a3s://tool-output/test/a")
            .expect("artifact")
            .content,
        "artifact content"
    );
}

#[tokio::test]
async fn test_file_store_save_and_load_trace_events() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();
    let event = TraceEvent::tool_execution(
        "read",
        true,
        0,
        std::time::Duration::from_millis(9),
        12,
        Some(&serde_json::json!({
            "artifact": {
                "artifact_uri": "a3s://tool-output/read/abc"
            }
        })),
    );

    store
        .save_trace_events("session/a", std::slice::from_ref(&event))
        .await
        .unwrap();
    let loaded = store
        .load_trace_events("session/a")
        .await
        .unwrap()
        .expect("trace events");

    assert_eq!(loaded, vec![event]);
}

#[tokio::test]
async fn test_file_store_save_and_load_run_records() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();
    let records = create_test_run_records().await;

    store.save_run_records("session/a", &records).await.unwrap();
    let loaded = store
        .load_run_records("session/a")
        .await
        .unwrap()
        .expect("run records");

    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].snapshot.prompt, "persist run");
    assert_eq!(loaded[0].events.len(), 1);
}

#[tokio::test]
async fn test_file_store_save_and_load_verification_reports() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();
    let report = create_test_verification_report();

    store
        .save_verification_reports("session/a", std::slice::from_ref(&report))
        .await
        .unwrap();
    let loaded = store
        .load_verification_reports("session/a")
        .await
        .unwrap()
        .expect("verification reports");

    assert_eq!(loaded, vec![report]);
}

#[tokio::test]
async fn test_memory_store_save_load_and_delete_artifacts() {
    let store = MemorySessionStore::new();
    let session = create_test_session_data();
    store.save(&session).await.unwrap();
    let artifacts = ArtifactStore::new();
    artifacts.put(crate::tools::ToolArtifact {
        artifact_id: "tool-output:test:a".to_string(),
        artifact_uri: "a3s://tool-output/test/a".to_string(),
        tool_name: "test".to_string(),
        content: "artifact content".to_string(),
        original_bytes: 16,
        shown_bytes: 4,
    });

    store.save_artifacts(&session.id, &artifacts).await.unwrap();
    assert!(store
        .load_artifacts(&session.id)
        .await
        .unwrap()
        .expect("artifacts")
        .get("a3s://tool-output/test/a")
        .is_some());

    store.delete(&session.id).await.unwrap();
    assert!(store.load_artifacts(&session.id).await.unwrap().is_none());
}

#[tokio::test]
async fn memory_store_replaces_complete_snapshot_generation_atomically() {
    let store = MemorySessionStore::new();
    let mut snapshot = create_test_snapshot().await;
    store.save_snapshot(&snapshot).await.unwrap();

    assert!(store.capabilities().atomic_session_snapshots);
    snapshot.artifacts[0].content = "mutated after save".to_string();
    let loaded = store
        .load_snapshot("test-session-1")
        .await
        .unwrap()
        .expect("snapshot");
    assert_eq!(loaded.artifacts[0].content, "snapshot artifact");
    assert_eq!(loaded.trace_events.len(), 1);
    assert_eq!(loaded.run_records.len(), 1);
    assert_eq!(loaded.verification_reports.len(), 1);
    assert_eq!(loaded.subagent_tasks.len(), 1);

    let mut session = loaded.session;
    session.config.name = "replacement".to_string();
    store
        .save_snapshot(&SessionSnapshotV1::session_only(session))
        .await
        .unwrap();
    let replacement = store
        .load_snapshot("test-session-1")
        .await
        .unwrap()
        .expect("replacement");
    assert_eq!(replacement.session.config.name, "replacement");
    assert!(replacement.artifacts.is_empty());
    assert!(replacement.trace_events.is_empty());
    assert!(replacement.run_records.is_empty());
    assert!(replacement.verification_reports.is_empty());
    assert!(replacement.subagent_tasks.is_empty());
}

#[test]
fn snapshot_rehydration_does_not_apply_smaller_default_artifact_limits() {
    let artifacts = ArtifactStore::with_limits(crate::tools::ArtifactStoreLimits {
        max_artifacts: 300,
        max_bytes: 1024 * 1024,
    });
    for index in 0..257 {
        artifacts.put(crate::tools::ToolArtifact {
            artifact_id: format!("artifact-{index}"),
            artifact_uri: format!("a3s://artifact/{index}"),
            tool_name: "test".to_string(),
            content: "x".to_string(),
            original_bytes: 1,
            shown_bytes: 1,
        });
    }

    let snapshot = SessionSnapshotV1::new(
        create_test_session_data(),
        &artifacts,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    assert_eq!(snapshot.artifact_store().len(), 257);
}

#[derive(Default)]
struct LegacyOnlyStore {
    save_calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl SessionStore for LegacyOnlyStore {
    async fn save(&self, _session: &SessionData) -> anyhow::Result<()> {
        self.save_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    async fn load(&self, _id: &str) -> anyhow::Result<Option<SessionData>> {
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

    fn backend_name(&self) -> &str {
        "legacy-only"
    }
}

#[tokio::test]
async fn aggregate_save_default_fails_without_fragmented_side_effects() {
    let store = LegacyOnlyStore::default();
    let snapshot = SessionSnapshotV1::session_only(create_test_session_data());

    let error = store.save_snapshot(&snapshot).await.unwrap_err();
    assert!(error
        .to_string()
        .contains("does not support aggregate session snapshots"));
    assert_eq!(
        store.save_calls.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[tokio::test]
async fn test_memory_store_save_load_and_delete_trace_events() {
    let store = MemorySessionStore::new();
    let session = create_test_session_data();
    let event = TraceEvent::tool_execution(
        "grep",
        false,
        1,
        std::time::Duration::from_millis(2),
        24,
        None,
    );

    store.save(&session).await.unwrap();
    store
        .save_trace_events(&session.id, std::slice::from_ref(&event))
        .await
        .unwrap();
    let loaded = store
        .load_trace_events(&session.id)
        .await
        .unwrap()
        .expect("trace events");
    assert_eq!(loaded, vec![event]);

    store.delete(&session.id).await.unwrap();
    assert!(store
        .load_trace_events(&session.id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn test_memory_store_save_load_and_delete_run_records() {
    let store = MemorySessionStore::new();
    let session = create_test_session_data();
    let records = create_test_run_records().await;

    store.save(&session).await.unwrap();
    store.save_run_records(&session.id, &records).await.unwrap();
    let loaded = store
        .load_run_records(&session.id)
        .await
        .unwrap()
        .expect("run records");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].events.len(), 1);

    store.delete(&session.id).await.unwrap();
    assert!(store.load_run_records(&session.id).await.unwrap().is_none());
}

#[tokio::test]
async fn test_memory_store_save_load_and_delete_verification_reports() {
    let store = MemorySessionStore::new();
    let session = create_test_session_data();
    let report = create_test_verification_report();

    store.save(&session).await.unwrap();
    store
        .save_verification_reports(&session.id, std::slice::from_ref(&report))
        .await
        .unwrap();
    let loaded = store
        .load_verification_reports(&session.id)
        .await
        .unwrap()
        .expect("verification reports");
    assert_eq!(loaded, vec![report]);

    store.delete(&session.id).await.unwrap();
    assert!(store
        .load_verification_reports(&session.id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn test_file_store_list() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();

    // Initially empty
    let list = store.list().await.unwrap();
    assert!(list.is_empty());

    // Add sessions
    for i in 1..=3 {
        let mut session = create_test_session_data();
        session.id = format!("session-{}", i);
        store.save(&session).await.unwrap();
    }

    // List should have 3 sessions
    let list = store.list().await.unwrap();
    assert_eq!(list.len(), 3);
    assert!(list.contains(&"session-1".to_string()));
    assert!(list.contains(&"session-2".to_string()));
    assert!(list.contains(&"session-3".to_string()));
}

#[tokio::test]
async fn test_file_store_overwrite() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();

    let mut session = create_test_session_data();
    store.save(&session).await.unwrap();

    // Modify and save again
    session.messages.push(Message::user("Another message"));
    session.updated_at = 1700000200;
    store.save(&session).await.unwrap();

    // Load and verify
    let loaded = store.load(&session.id).await.unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 3);
    assert_eq!(loaded.updated_at, 1700000200);
}

#[tokio::test]
async fn test_file_store_path_traversal_prevention() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();

    // Attempt path traversal - should be sanitized
    let mut session = create_test_session_data();
    session.id = "../../../etc/passwd".to_string();
    store.save(&session).await.unwrap();

    // File should be in the store directory, not /etc/passwd
    let files: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(files.len(), 1);

    // Should still be loadable with sanitized ID
    let loaded = store.load(&session.id).await.unwrap();
    assert!(loaded.is_some());
}

#[tokio::test]
async fn test_file_store_with_policies() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();

    let mut session = create_test_session_data();
    session.config.confirmation_policy = Some(ConfirmationPolicy::enabled());
    session.config.permission_policy = Some(PermissionPolicy::new().allow("Bash(cargo:*)"));
    session.config.queue_config = Some(SessionQueueConfig::default());
    session.config.enforce_active_skill_tool_restrictions = true;

    store.save(&session).await.unwrap();

    let loaded = store.load(&session.id).await.unwrap().unwrap();
    assert!(loaded.config.confirmation_policy.is_some());
    assert!(loaded.config.permission_policy.is_some());
    assert!(loaded.config.queue_config.is_some());
    assert!(loaded.config.enforce_active_skill_tool_restrictions);
}

#[tokio::test]
async fn test_file_store_with_llm_config() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();

    let mut session = create_test_session_data();
    session.llm_config = Some(LlmConfigData {
        provider: "anthropic".to_string(),
        model: "claude-3-5-sonnet-20241022".to_string(),
        api_key: Some("secret".to_string()), // Should NOT be saved
        base_url: None,
    });

    store.save(&session).await.unwrap();

    let loaded = store.load(&session.id).await.unwrap().unwrap();
    let llm_config = loaded.llm_config.unwrap();
    assert_eq!(llm_config.provider, "anthropic");
    assert_eq!(llm_config.model, "claude-3-5-sonnet-20241022");
    // API key should not be persisted
    assert!(llm_config.api_key.is_none());
}

// ========================================================================
// MemorySessionStore Tests
// ========================================================================

#[tokio::test]
async fn test_memory_store_save_and_load() {
    let store = MemorySessionStore::new();
    let session = create_test_session_data();

    store.save(&session).await.unwrap();

    let loaded = store.load(&session.id).await.unwrap();
    assert!(loaded.is_some());
    assert_eq!(loaded.unwrap().id, session.id);
}

#[tokio::test]
async fn test_memory_store_delete() {
    let store = MemorySessionStore::new();
    let session = create_test_session_data();

    store.save(&session).await.unwrap();
    assert!(store.exists(&session.id).await.unwrap());

    store.delete(&session.id).await.unwrap();
    assert!(!store.exists(&session.id).await.unwrap());
}

#[tokio::test]
async fn test_memory_store_list() {
    let store = MemorySessionStore::new();

    for i in 1..=3 {
        let mut session = create_test_session_data();
        session.id = format!("session-{}", i);
        store.save(&session).await.unwrap();
    }

    let list = store.list().await.unwrap();
    assert_eq!(list.len(), 3);
}

// ========================================================================
// SessionData Tests
// ========================================================================

#[test]
fn test_session_data_serialization() {
    let session = create_test_session_data();
    let json = serde_json::to_string(&session).unwrap();
    let parsed: SessionData = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.id, session.id);
    assert_eq!(parsed.messages.len(), session.messages.len());
}

#[test]
fn test_tool_names_from_definitions() {
    let tools = vec![
        crate::llm::ToolDefinition {
            name: "bash".to_string(),
            description: "Execute bash".to_string(),
            parameters: serde_json::json!({}),
        },
        crate::llm::ToolDefinition {
            name: "read".to_string(),
            description: "Read file".to_string(),
            parameters: serde_json::json!({}),
        },
    ];

    let names = SessionData::tool_names_from_definitions(&tools);
    assert_eq!(names, vec!["bash", "read"]);
}

// ========================================================================
// Sanitization Tests
// ========================================================================

#[tokio::test]
async fn test_file_store_backslash_sanitization() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();

    let mut session = create_test_session_data();
    session.id = r"foo\bar\baz".to_string();
    store.save(&session).await.unwrap();

    let loaded = store.load(&session.id).await.unwrap();
    assert!(loaded.is_some());

    let loaded = loaded.unwrap();
    assert_eq!(loaded.id, session.id);

    // The collision-free layout never embeds path separators in a filename.
    let expected_path = encoded_file_path(dir.path(), "sessions", &session.id);
    assert!(expected_path.exists());
}

#[tokio::test]
async fn test_file_store_mixed_separator_sanitization() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();

    let mut session = create_test_session_data();
    session.id = r"foo/bar\baz..qux".to_string();
    store.save(&session).await.unwrap();

    let loaded = store.load(&session.id).await.unwrap();
    assert!(loaded.is_some());

    let loaded = loaded.unwrap();
    assert_eq!(loaded.id, session.id);

    let expected_path = encoded_file_path(dir.path(), "sessions", &session.id);
    assert!(expected_path.exists());
}

#[tokio::test]
async fn file_store_ids_that_collided_in_the_legacy_layout_remain_distinct() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();

    let mut slash = create_test_session_data();
    slash.id = "tenant/session".to_string();
    slash.config.name = "slash".to_string();
    let mut underscore = create_test_session_data();
    underscore.id = "tenant_session".to_string();
    underscore.config.name = "underscore".to_string();

    store.save(&slash).await.unwrap();
    store.save(&underscore).await.unwrap();

    assert_eq!(
        store.load(&slash.id).await.unwrap().unwrap().config.name,
        "slash"
    );
    assert_eq!(
        store
            .load(&underscore.id)
            .await
            .unwrap()
            .unwrap()
            .config
            .name,
        "underscore"
    );
    assert_ne!(
        encoded_file_path(dir.path(), "sessions", &slash.id),
        encoded_file_path(dir.path(), "sessions", &underscore.id)
    );
    assert_eq!(
        store.list().await.unwrap(),
        vec![slash.id.clone(), underscore.id.clone()]
    );

    store.delete(&slash.id).await.unwrap();
    assert!(store.load(&slash.id).await.unwrap().is_none());
    assert!(store.load(&underscore.id).await.unwrap().is_some());
}

#[tokio::test]
async fn legacy_session_collision_never_returns_another_sessions_payload() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();
    let mut stored = create_test_session_data();
    stored.id = "tenant_session".to_string();
    tokio::fs::write(
        dir.path().join("tenant_session.json"),
        serde_json::to_vec(&stored).unwrap(),
    )
    .await
    .unwrap();

    let error = store.load("tenant/session").await.unwrap_err();
    assert!(error.to_string().contains("key collision"));
    assert!(!store.exists("tenant/session").await.unwrap());
    assert_eq!(
        store.load("tenant_session").await.unwrap().unwrap().id,
        "tenant_session"
    );
}

// ========================================================================
// Error Recovery Tests
// ========================================================================

#[tokio::test]
async fn test_file_store_corrupted_json_recovery() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();

    // Manually write invalid JSON to a session file
    let corrupted_path = dir.path().join("test-id.json");
    tokio::fs::write(&corrupted_path, b"not valid json {{{")
        .await
        .unwrap();

    // Loading should return an error, not panic
    let result = store.load("test-id").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn file_store_rejects_oversized_json_before_allocation() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();
    let session_id = "oversized-session";
    let path = encoded_file_path(dir.path(), "sessions", session_id);
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    let file = tokio::fs::File::create(&path).await.unwrap();
    file.set_len(super::file_store::MAX_FILE_STORE_JSON_BYTES + 1)
        .await
        .unwrap();
    drop(file);

    let error = store.load(session_id).await.unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("exceeds"), "unexpected error: {message}");
    assert!(
        message.contains("byte limit"),
        "unexpected error: {message}"
    );
}

// ========================================================================
// Exists Tests
// ========================================================================

#[tokio::test]
async fn test_file_store_exists() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();

    let session = create_test_session_data();

    // Not yet saved
    assert!(!store.exists(&session.id).await.unwrap());

    // Save and verify exists
    store.save(&session).await.unwrap();
    assert!(store.exists(&session.id).await.unwrap());

    // Delete and verify gone
    store.delete(&session.id).await.unwrap();
    assert!(!store.exists(&session.id).await.unwrap());
}

#[tokio::test]
async fn test_memory_store_exists() {
    let store = MemorySessionStore::new();

    // Unknown id
    assert!(!store.exists("unknown-id").await.unwrap());

    // Save and verify exists
    let session = create_test_session_data();
    store.save(&session).await.unwrap();
    assert!(store.exists(&session.id).await.unwrap());
}

#[tokio::test]
async fn test_file_store_health_check() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();
    assert!(store.health_check().await.is_ok());
    assert_eq!(store.backend_name(), "file");
}

#[tokio::test]
async fn test_file_store_health_check_bad_dir() {
    let store = FileSessionStore {
        dir: std::path::PathBuf::from("/nonexistent/path/that/does/not/exist"),
        write_lock: tokio::sync::Mutex::new(()),
        wal: FileSessionStoreWal::new("/nonexistent/path/that/does/not/exist"),
        next_wal_sequence: std::sync::atomic::AtomicU64::new(1),
        held_writer_lease: tokio::sync::Mutex::new(None),
        commit_watch: super::watch::commit_watch_channel().0,
        encryption: None,
    };
    assert!(store.health_check().await.is_err());
}

#[tokio::test]
async fn test_memory_store_health_check() {
    let store = MemorySessionStore::new();
    assert!(store.health_check().await.is_ok());
    assert_eq!(store.backend_name(), "memory");
}

// ========================================================================
// Session Resume Boundary Tests
// ========================================================================

#[tokio::test]
async fn test_file_store_load_empty_file() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();

    // Write an empty file — JSON parse must fail gracefully, not panic
    let empty_path = dir.path().join("empty-session.json");
    tokio::fs::write(&empty_path, b"").await.unwrap();

    let result = store.load("empty-session").await;
    assert!(
        result.is_err(),
        "Empty file must return error, not Ok(None)"
    );
}

#[tokio::test]
async fn test_file_store_load_partial_json() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();

    // Truncated JSON — simulates a crash mid-write
    let partial_path = dir.path().join("partial-session.json");
    tokio::fs::write(&partial_path, b"{\"id\":\"partial-session\",\"message")
        .await
        .unwrap();

    let result = store.load("partial-session").await;
    assert!(result.is_err(), "Partial JSON must return error");
}

#[tokio::test]
async fn test_file_store_concurrent_save() {
    let dir = tempdir().unwrap();
    let store = std::sync::Arc::new(FileSessionStore::new(dir.path()).await.unwrap());

    let session = create_test_session_data();
    let id = session.id.clone();

    // First save to create the file
    store.save(&session).await.unwrap();

    // Spawn multiple concurrent saves — last write wins, no corruption
    let mut handles = Vec::new();
    for _ in 0..5 {
        let s = store.clone();
        let sess = session.clone();
        handles.push(tokio::spawn(async move { s.save(&sess).await }));
    }
    for h in handles {
        h.await.unwrap().unwrap();
    }

    // File must be loadable after concurrent writes
    let loaded = store.load(&id).await.unwrap();
    assert!(loaded.is_some());
    assert_eq!(loaded.unwrap().id, id);
}

#[cfg(windows)]
#[test]
fn test_file_store_atomic_replace_retries_only_transient_windows_errors() {
    use std::io::Error;

    for raw_os_error in [5, 32, 33] {
        assert!(super::file_store::windows_atomic_replace_retry_delay(
            &Error::from_raw_os_error(raw_os_error),
            1,
        )
        .is_some());
    }
    assert!(super::file_store::windows_atomic_replace_retry_delay(
        &Error::from_raw_os_error(87),
        1,
    )
    .is_none());
    assert!(
        super::file_store::windows_atomic_replace_retry_delay(&Error::from_raw_os_error(5), 6,)
            .is_none()
    );
}

#[tokio::test]
async fn test_file_store_load_nonexistent_returns_none() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();

    let result = store.load("does-not-exist-at-all").await.unwrap();
    assert!(result.is_none(), "Missing session must return Ok(None)");
}

fn sample_checkpoint(run_id: &str) -> crate::loop_checkpoint::LoopCheckpoint {
    crate::loop_checkpoint::LoopCheckpoint {
        schema_version: crate::loop_checkpoint::LOOP_CHECKPOINT_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        session_id: "s-1".to_string(),
        capability_binding: None,
        turn: 2,
        messages: vec![Message::user("hi")],
        total_usage: TokenUsage::default(),
        tool_calls_count: 1,
        verification_reports: Vec::new(),
        convergence: Default::default(),
        checkpoint_ms: 1_700_000_000_000,
    }
}

#[tokio::test]
async fn test_memory_store_delete_loop_checkpoint() {
    let store = MemorySessionStore::new();
    store
        .save_loop_checkpoint("run-x", &sample_checkpoint("run-x"))
        .await
        .unwrap();
    assert!(store.load_loop_checkpoint("run-x").await.unwrap().is_some());

    store.delete_loop_checkpoint("run-x").await.unwrap();
    assert!(
        store.load_loop_checkpoint("run-x").await.unwrap().is_none(),
        "checkpoint must be gone after delete"
    );

    // Deleting a non-existent checkpoint is a no-op success.
    store.delete_loop_checkpoint("never-existed").await.unwrap();
}

#[tokio::test]
async fn test_file_store_delete_loop_checkpoint() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();
    store
        .save_loop_checkpoint("run-y", &sample_checkpoint("run-y"))
        .await
        .unwrap();
    let loaded = store.load_loop_checkpoint("run-y").await.unwrap();
    assert_eq!(loaded.unwrap().run_id, "run-y");

    store.delete_loop_checkpoint("run-y").await.unwrap();
    assert!(store.load_loop_checkpoint("run-y").await.unwrap().is_none());

    // Idempotent on a missing file.
    store.delete_loop_checkpoint("run-y").await.unwrap();
}

#[tokio::test]
async fn test_file_store_checkpoint_write_is_atomic_no_temp_leftovers() {
    // The crash-atomic write uses a temp file + rename. After a normal
    // save, no `.tmp` files should be left behind in the checkpoint dir.
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();
    store
        .save_loop_checkpoint("run-z", &sample_checkpoint("run-z"))
        .await
        .unwrap();

    let ckpt_dir = dir.path().join("v1").join("loop_checkpoints");
    let mut entries = tokio::fs::read_dir(&ckpt_dir).await.unwrap();
    let mut names = Vec::new();
    while let Some(e) = entries.next_entry().await.unwrap() {
        names.push(e.file_name().to_string_lossy().to_string());
    }
    assert!(
        names.iter().all(|n| !n.contains(".tmp")),
        "no temp files should remain after atomic write, got: {names:?}"
    );
    assert!(
        names.iter().any(|n| n
            == encoded_file_path(dir.path(), "loop_checkpoints", "run-z")
                .file_name()
                .unwrap()
                .to_string_lossy()
                .as_ref()),
        "the final checkpoint file must exist, got: {names:?}"
    );
}

fn sample_workflow_checkpoint(wf_id: &str) -> crate::orchestration::WorkflowCheckpoint {
    use crate::orchestration::{
        StepOutcome, WorkflowCheckpoint, WorkflowStepRecord, WORKFLOW_CHECKPOINT_SCHEMA_VERSION,
    };
    let outcome = |id: &str, structured| StepOutcome {
        task_id: id.to_string(),
        session_id: format!("task-run-{id}"),
        agent: "explore".to_string(),
        output: format!("out-{id}"),
        success: true,
        structured,
        source_anchors: Vec::new(),
    };
    WorkflowCheckpoint {
        schema_version: WORKFLOW_CHECKPOINT_SCHEMA_VERSION,
        workflow_id: wf_id.to_string(),
        steps: vec![
            WorkflowStepRecord {
                task_id: "a".into(),
                outcome: outcome("a", None),
                result_receipt: None,
            },
            WorkflowStepRecord {
                task_id: "b".into(),
                outcome: outcome("b", Some(serde_json::json!({ "k": 1 }))),
                result_receipt: None,
            },
        ],
        checkpoint_ms: 1_700_000_000_000,
    }
}

#[tokio::test]
async fn test_file_store_workflow_checkpoint_roundtrip() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();
    let cp = sample_workflow_checkpoint("wf-1");
    store.save_workflow_checkpoint("wf-1", &cp).await.unwrap();

    let loaded = store
        .load_workflow_checkpoint("wf-1")
        .await
        .unwrap()
        .expect("present");
    assert_eq!(loaded, cp);
    assert_eq!(loaded.completed().len(), 2);

    store.delete_workflow_checkpoint("wf-1").await.unwrap();
    assert!(store
        .load_workflow_checkpoint("wf-1")
        .await
        .unwrap()
        .is_none());
    // Idempotent on a missing file.
    store.delete_workflow_checkpoint("wf-1").await.unwrap();
}

#[tokio::test]
async fn test_file_store_workflow_receipt_survives_restart() {
    let dir = tempdir().unwrap();
    let spec = AgentStepSpec::new("receipt-step", "explore", "inspect", "bounded prompt");
    let outcome = crate::orchestration::StepOutcome {
        task_id: "receipt-step".to_string(),
        session_id: "task-run-receipt-step".to_string(),
        agent: "explore".to_string(),
        output: "bounded output".to_string(),
        success: true,
        structured: None,
        source_anchors: Vec::new(),
    };
    let receipt = workflow_step_result_receipt("wf-receipt", &spec, &outcome, None).unwrap();
    let mut completed = std::collections::HashMap::new();
    completed.insert(outcome.task_id.clone(), outcome.clone());
    let mut receipts = std::collections::HashMap::new();
    receipts.insert(outcome.task_id.clone(), receipt.clone());
    let checkpoint = crate::orchestration::WorkflowCheckpoint::from_completed_with_receipts(
        "wf-receipt",
        &completed,
        &receipts,
        1,
    );

    let store = FileSessionStore::new(dir.path()).await.unwrap();
    store
        .save_workflow_checkpoint("wf-receipt", &checkpoint)
        .await
        .unwrap();
    drop(store);

    let reopened = FileSessionStore::new(dir.path()).await.unwrap();
    let loaded = reopened
        .load_workflow_checkpoint("wf-receipt")
        .await
        .unwrap()
        .expect("receipt checkpoint survives reopen");
    loaded.ensure_loadable().unwrap();
    assert_eq!(loaded.steps[0].result_receipt, Some(receipt));
}

#[tokio::test]
async fn test_memory_store_rejects_future_workflow_checkpoint() {
    let store = MemorySessionStore::new();
    let mut future = sample_workflow_checkpoint("wf-future");
    future.schema_version = crate::orchestration::WORKFLOW_CHECKPOINT_SCHEMA_VERSION + 1;
    store
        .save_workflow_checkpoint("wf-future", &future)
        .await
        .unwrap();
    let err = store
        .load_workflow_checkpoint("wf-future")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("schema version"), "got: {err}");

    store
        .save_workflow_checkpoint("wf-ok", &sample_workflow_checkpoint("wf-ok"))
        .await
        .unwrap();
    assert!(store
        .load_workflow_checkpoint("wf-ok")
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn test_file_store_rejects_future_workflow_checkpoint() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();
    let mut future = sample_workflow_checkpoint("wf-f");
    future.schema_version = crate::orchestration::WORKFLOW_CHECKPOINT_SCHEMA_VERSION + 1;
    store
        .save_workflow_checkpoint("wf-f", &future)
        .await
        .unwrap();
    let err = store.load_workflow_checkpoint("wf-f").await.unwrap_err();
    assert!(err.to_string().contains("schema version"), "got: {err}");
}

#[tokio::test]
async fn test_file_store_workflow_checkpoint_atomic_no_temp_leftovers() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();
    store
        .save_workflow_checkpoint("wf-z", &sample_workflow_checkpoint("wf-z"))
        .await
        .unwrap();

    let ckpt_dir = dir.path().join("v1").join("workflow_checkpoints");
    let mut entries = tokio::fs::read_dir(&ckpt_dir).await.unwrap();
    let mut names = Vec::new();
    while let Some(e) = entries.next_entry().await.unwrap() {
        names.push(e.file_name().to_string_lossy().to_string());
    }
    assert!(
        names.iter().all(|n| !n.contains(".tmp")),
        "no temp leftovers after atomic write, got: {names:?}"
    );
    assert!(
        names.iter().any(|n| n
            == encoded_file_path(dir.path(), "workflow_checkpoints", "wf-z")
                .file_name()
                .unwrap()
                .to_string_lossy()
                .as_ref()),
        "the final workflow checkpoint file must exist, got: {names:?}"
    );
}

#[test]
fn store_capabilities_default_to_explicit_negotiation() {
    use super::SessionStoreCapabilities;

    // Every KRN-6 guarantee starts unadvertised: hosts must negotiate before
    // relying on CAS, append-only logs, fencing, encryption, watch, or
    // reference-aware artifact GC. Only proven guarantees are set.
    let defaults = SessionStoreCapabilities::default();
    assert!(defaults.atomic_session_snapshots == defaults.aggregate_cas);
    assert!(!defaults.aggregate_cas);
    assert!(!defaults.append_only_event_log);
    assert!(!defaults.lease_fencing);
    assert!(!defaults.encrypted_at_rest);
    assert!(!defaults.watch);
    assert!(!defaults.reference_aware_artifact_gc);

    // Memory advertises atomic snapshots, aggregate CAS, reference-aware
    // artifact GC, and commit watch (STORE-CAS1 / STORE-GC2 / STORE-WATCH1).
    // FileSessionStore additionally advertises append-only WAL (STORE-WAL1)
    // and writer lease fencing (STORE-LEASE1).
    let memory = crate::store::memory_store::MemorySessionStore::default();
    assert_eq!(
        memory.capabilities(),
        SessionStoreCapabilities {
            atomic_session_snapshots: true,
            aggregate_cas: true,
            reference_aware_artifact_gc: true,
            watch: true,
            ..SessionStoreCapabilities::default()
        }
    );
}

#[tokio::test]
async fn file_store_encryption_at_rest_round_trips_and_hides_plaintext() {
    let dir = tempdir().unwrap();
    let key = [9u8; 32];
    let store = FileSessionStore::with_encryption_key(dir.path(), &key)
        .await
        .unwrap();
    assert!(store.capabilities().encrypted_at_rest);

    let snapshot = create_test_snapshot().await;
    store.save_snapshot(&snapshot).await.unwrap();

    let path = encoded_file_path(dir.path(), "sessions", "test-session-1");
    let on_disk = tokio::fs::read(&path).await.unwrap();
    assert!(
        SessionStoreAtRestCipher::is_sealed(&on_disk),
        "snapshot must be sealed at rest"
    );
    assert!(
        !String::from_utf8_lossy(&on_disk).contains("Test Session"),
        "session plaintext must not appear on disk"
    );

    let reopened = FileSessionStore::with_encryption_key(dir.path(), &key)
        .await
        .unwrap();
    let loaded = reopened
        .load_snapshot("test-session-1")
        .await
        .unwrap()
        .expect("decryptable snapshot");
    assert_eq!(loaded.session.config.name, "Test Session");

    let wrong = FileSessionStore::with_encryption_key(dir.path(), &[1u8; 32])
        .await
        .unwrap();
    let err = wrong
        .load_snapshot("test-session-1")
        .await
        .expect_err("wrong key must fail closed");
    assert!(
        err.to_string().contains("decrypt") || err.to_string().contains("at-rest"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn file_and_memory_store_watch_commits_after_snapshot() {
    let dir = tempdir().unwrap();
    let file = FileSessionStore::new(dir.path()).await.unwrap();
    assert!(file.capabilities().watch);
    let memory = MemorySessionStore::default();
    assert!(memory.capabilities().watch);

    let mut file_watch = file.watch_commits().await.unwrap();
    let mut memory_watch = memory.watch_commits().await.unwrap();
    let snapshot = create_test_snapshot().await;
    let digest = snapshot_content_digest(&snapshot).unwrap();

    file.save_snapshot(&snapshot).await.unwrap();
    memory.save_snapshot(&snapshot).await.unwrap();

    let file_event = file_watch.recv().await.unwrap();
    let memory_event = memory_watch.recv().await.unwrap();
    assert_eq!(file_event.session_id, "test-session-1");
    assert_eq!(file_event.snapshot_digest, digest);
    assert_eq!(memory_event.session_id, "test-session-1");
    assert_eq!(memory_event.snapshot_digest, digest);
}

#[tokio::test]
async fn file_and_memory_store_persist_artifact_retention_roots() {
    let dir = tempdir().unwrap();
    let file = FileSessionStore::new(dir.path()).await.unwrap();
    assert!(file.capabilities().reference_aware_artifact_gc);
    let memory = MemorySessionStore::default();
    assert!(memory.capabilities().reference_aware_artifact_gc);

    let artifacts = ArtifactStore::with_limits(crate::tools::ArtifactStoreLimits {
        max_artifacts: 2,
        max_bytes: 16 * 1024 * 1024,
    });
    for (name, content) in [("keep", "keep-bytes"), ("drop", "drop-bytes")] {
        artifacts.put(crate::tools::ToolArtifact {
            artifact_id: format!("tool-output:test:{name}"),
            artifact_uri: format!("a3s://tool-output/test/{name}"),
            tool_name: "test".to_string(),
            content: content.to_string(),
            original_bytes: content.len(),
            shown_bytes: content.len(),
        });
    }
    artifacts.pin_uris(["a3s://tool-output/test/keep"]);

    let session = create_test_session_data();
    let snapshot = SessionSnapshotV1::new(
        session,
        &artifacts,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    file.save_snapshot(&snapshot).await.unwrap();
    memory.save_snapshot(&snapshot).await.unwrap();

    let file_store = file
        .load_snapshot("test-session-1")
        .await
        .unwrap()
        .expect("file")
        .artifact_store();
    let memory_store = memory
        .load_snapshot("test-session-1")
        .await
        .unwrap()
        .expect("memory")
        .artifact_store();
    assert!(file_store
        .retained_uris()
        .contains("a3s://tool-output/test/keep"));
    assert_eq!(file_store.gc_unreferenced(), 1);
    assert!(file_store.get("a3s://tool-output/test/keep").is_some());
    assert!(file_store.get("a3s://tool-output/test/drop").is_none());
    assert_eq!(memory_store.gc_unreferenced(), 1);
    assert!(memory_store.get("a3s://tool-output/test/keep").is_some());
}

#[tokio::test]
async fn file_store_writer_lease_takeover_fences_stale_holder() {
    let dir = tempdir().unwrap();
    let first = FileSessionStore::new(dir.path()).await.unwrap();
    assert!(first.capabilities().lease_fencing);
    let lease_a = first.acquire_writer_lease("worker-a").await.unwrap();
    assert_eq!(lease_a.epoch, 1);

    let snapshot = create_test_snapshot().await;
    first.save_snapshot(&snapshot).await.unwrap();

    let second = FileSessionStore::new(dir.path()).await.unwrap();
    let lease_b = second.acquire_writer_lease("worker-b").await.unwrap();
    assert_eq!(lease_b.epoch, 2);
    assert_eq!(
        second.writer_lease().await.unwrap().unwrap().holder_id,
        "worker-b"
    );

    let mut stale = snapshot.clone();
    stale.session.config.name = "stale-after-takeover".into();
    let err = first
        .save_snapshot(&stale)
        .await
        .expect_err("stale holder must fail closed after takeover");
    assert!(
        err.to_string().contains("lease lost") || err.to_string().contains("taken over"),
        "unexpected error: {err}"
    );

    let mut next = snapshot.clone();
    next.session.config.name = "holder-b".into();
    second.save_snapshot(&next).await.unwrap();
    let loaded = second
        .load_snapshot("test-session-1")
        .await
        .unwrap()
        .expect("snapshot");
    assert_eq!(loaded.session.config.name, "holder-b");
}

#[tokio::test]
async fn file_and_memory_store_cas_match_writes_and_mismatch_skips() {
    let dir = tempdir().unwrap();
    let file = FileSessionStore::new(dir.path()).await.unwrap();
    assert!(file.capabilities().aggregate_cas);
    let memory = crate::store::memory_store::MemorySessionStore::default();
    assert!(memory.capabilities().aggregate_cas);

    let snapshot = create_test_snapshot().await;
    let digest = snapshot_content_digest(&snapshot).unwrap();

    assert!(file.save_snapshot_cas(&snapshot, None).await.unwrap());
    assert!(memory.save_snapshot_cas(&snapshot, None).await.unwrap());

    let mut next = snapshot.clone();
    next.session.config.name = "cas-next".into();
    assert!(file.save_snapshot_cas(&next, Some(&digest)).await.unwrap());
    assert!(memory
        .save_snapshot_cas(&next, Some(&digest))
        .await
        .unwrap());

    let mut stale = next.clone();
    stale.session.config.name = "cas-stale".into();
    let wrong = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
    assert!(!file.save_snapshot_cas(&stale, Some(wrong)).await.unwrap());
    assert!(!memory.save_snapshot_cas(&stale, Some(wrong)).await.unwrap());

    let loaded_file = file
        .load_snapshot("test-session-1")
        .await
        .unwrap()
        .expect("file snapshot");
    let loaded_memory = memory
        .load_snapshot("test-session-1")
        .await
        .unwrap()
        .expect("memory snapshot");
    assert_eq!(loaded_file.session.config.name, "cas-next");
    assert_eq!(loaded_memory.session.config.name, "cas-next");
}

#[tokio::test]
async fn file_store_wal_records_intent_and_commit_for_each_snapshot() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();
    assert!(store.capabilities().append_only_event_log);
    let snapshot = create_test_snapshot().await;
    store.save_snapshot(&snapshot).await.unwrap();

    let wal = FileSessionStoreWal::new(dir.path());
    let (entries, next) = wal.load_entries().await.unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].phase, SessionStoreWalPhaseV1::Intent);
    assert_eq!(entries[1].phase, SessionStoreWalPhaseV1::Committed);
    assert_eq!(entries[0].sequence, entries[1].sequence);
    assert_eq!(next, entries[0].sequence + 1);
    assert_eq!(
        entries[0].snapshot_digest,
        snapshot_content_digest(&snapshot).unwrap()
    );
}

#[tokio::test]
async fn file_store_wal_recovery_seals_open_intent_when_snapshot_matches() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();
    let snapshot = create_test_snapshot().await;
    store.save_snapshot(&snapshot).await.unwrap();

    // Simulate crash after atomic replace but before commit: drop the trailing
    // Committed line while leaving the Intent and durable snapshot intact.
    let wal_path = FileSessionStoreWal::new(dir.path()).path().to_path_buf();
    let raw = tokio::fs::read_to_string(&wal_path).await.unwrap();
    let mut lines: Vec<&str> = raw.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(lines.len(), 2);
    lines.pop();
    tokio::fs::write(&wal_path, format!("{}\n", lines[0]))
        .await
        .unwrap();

    let recovered = FileSessionStore::new(dir.path()).await.unwrap();
    let loaded = recovered
        .load_snapshot("test-session-1")
        .await
        .unwrap()
        .expect("snapshot survives crash window");
    assert_eq!(loaded.session.id, "test-session-1");

    let (entries, _) = FileSessionStoreWal::new(dir.path())
        .load_entries()
        .await
        .unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry.phase, SessionStoreWalPhaseV1::Committed)),
        "recovery must seal the open intent when the durable snapshot matches"
    );
}

#[tokio::test]
async fn file_store_wal_rejects_duplicate_intent_sequences_on_reopen() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();
    let snapshot = create_test_snapshot().await;
    store.save_snapshot(&snapshot).await.unwrap();

    let wal = FileSessionStoreWal::new(dir.path());
    let digest = snapshot_content_digest(&snapshot).unwrap();
    let duplicate = SessionStoreWalEntryV1::new(
        1,
        "test-session-1",
        digest,
        SessionStoreWalPhaseV1::Intent,
        99,
    )
    .unwrap();
    wal.append(&duplicate).await.unwrap();
    let err = FileSessionStoreWal::new(dir.path())
        .load_entries()
        .await
        .expect_err("duplicate intent sequence must fail closed");
    assert!(
        err.to_string().contains("conflicts"),
        "unexpected error: {err}"
    );
    assert!(FileSessionStoreWal::is_sequence_conflict(&err));
}

#[tokio::test]
async fn file_store_recovering_open_quarantines_corrupt_wal() {
    let dir = tempdir().unwrap();
    let store = FileSessionStore::new(dir.path()).await.unwrap();
    let snapshot = create_test_snapshot().await;
    store.save_snapshot(&snapshot).await.unwrap();
    drop(store);

    let wal = FileSessionStoreWal::new(dir.path());
    let digest = snapshot_content_digest(&snapshot).unwrap();
    let duplicate = SessionStoreWalEntryV1::new(
        1,
        "other-session",
        digest,
        SessionStoreWalPhaseV1::Intent,
        99,
    )
    .unwrap();
    wal.append(&duplicate).await.unwrap();

    assert!(
        FileSessionStore::new(dir.path()).await.is_err(),
        "strict open must still fail closed"
    );

    let recovered = FileSessionStore::new_recovering_corrupt_wal(dir.path())
        .await
        .expect("recovering open quarantines and reopens");
    let loaded = recovered
        .load_snapshot(&snapshot.session.id)
        .await
        .unwrap()
        .expect("durable snapshot survives WAL quarantine");
    assert_eq!(loaded.session.id, snapshot.session.id);
    assert!(
        !wal.path().exists(),
        "corrupt WAL must be moved aside on recovery"
    );
}

#[tokio::test]
async fn file_store_wal_rejects_intent_then_committed_for_different_session() {
    let dir = tempdir().unwrap();
    let wal = FileSessionStoreWal::new(dir.path());
    let snapshot = create_test_snapshot().await;
    let digest = snapshot_content_digest(&snapshot).unwrap();
    let intent = SessionStoreWalEntryV1::new(
        1,
        "session-a",
        digest.clone(),
        SessionStoreWalPhaseV1::Intent,
        1,
    )
    .unwrap();
    let committed =
        SessionStoreWalEntryV1::new(1, "session-b", digest, SessionStoreWalPhaseV1::Committed, 2)
            .unwrap();
    wal.append(&intent).await.unwrap();
    wal.append(&committed).await.unwrap();
    let err = wal
        .load_entries()
        .await
        .expect_err("cross-session Intent→Committed must fail");
    assert!(err.to_string().contains("conflicts"));
}
