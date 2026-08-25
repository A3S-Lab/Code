use super::*;
use crate::agent::AgentEvent;
use crate::run::{RunEventRecord, RunRecord, RunSnapshot, RunStatus};
use crate::store::{ContextUsage, SessionConfig, SessionState};
use crate::subagent_task_tracker::{SubagentStatus, SubagentTaskSnapshot};

struct ResumeImmutableContentAdapter;

#[async_trait::async_trait]
impl crate::tools::ImmutableContentAdapter for ResumeImmutableContentAdapter {
    fn name(&self) -> &str {
        "resume-immutable-content"
    }

    async fn put(
        &self,
        _request: &crate::tools::ImmutableContentWriteRequestV1<'_>,
    ) -> crate::tools::ImmutableContentResult<crate::tools::ImmutableContentReferenceV1> {
        Err(crate::tools::ImmutableContentError::Provider(
            "not used by resume tests".to_string(),
        ))
    }
}

fn immutable_content_binding(marker: char) -> crate::tools::ImmutableContentAdapterBindingV1 {
    crate::tools::ImmutableContentAdapterBindingV1::new(
        format!("sha256:{}", marker.to_string().repeat(64)),
        1024 * 1024,
    )
    .unwrap()
}

fn immutable_content_session(
    binding: crate::tools::ImmutableContentAdapterBindingV1,
) -> crate::tools::ImmutableContentAdapterSession {
    crate::tools::ImmutableContentAdapterSession::new(
        binding,
        Arc::new(ResumeImmutableContentAdapter),
    )
    .unwrap()
}

#[derive(Default)]
struct SnapshotOnlyStore {
    aggregate_saves: std::sync::atomic::AtomicUsize,
    legacy_saves: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl SessionStore for SnapshotOnlyStore {
    async fn save(&self, _session: &SessionData) -> anyhow::Result<()> {
        self.legacy_saves
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

    async fn save_snapshot(&self, snapshot: &SessionSnapshotV1) -> anyhow::Result<()> {
        snapshot.ensure_loadable()?;
        self.aggregate_saves
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    fn backend_name(&self) -> &str {
        "snapshot-only-test"
    }
}

struct ReturningSnapshotStore {
    snapshot: SessionSnapshotV1,
}

#[async_trait::async_trait]
impl SessionStore for ReturningSnapshotStore {
    async fn save(&self, _session: &SessionData) -> anyhow::Result<()> {
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

    async fn load_snapshot(&self, _id: &str) -> anyhow::Result<Option<SessionSnapshotV1>> {
        // Deliberately ignore the requested key. Core must not trust a
        // custom backend to enforce payload identity or invariants.
        Ok(Some(self.snapshot.clone()))
    }

    fn backend_name(&self) -> &str {
        "returning-snapshot-test"
    }
}

fn persisted_data(model_name: Option<&str>, llm: Option<(&str, &str)>) -> SessionData {
    SessionData {
        id: "session-1".to_string(),
        config: SessionConfig::default(),
        state: SessionState::Active,
        messages: Vec::new(),
        context_usage: ContextUsage::default(),
        total_usage: crate::llm::TokenUsage::default(),
        total_cost: 0.0,
        model_name: model_name.map(ToOwned::to_owned),
        cost_records: Vec::new(),
        tool_names: Vec::new(),
        thinking_enabled: false,
        thinking_budget: None,
        created_at: 0,
        updated_at: 0,
        llm_config: llm.map(|(provider, model)| LlmConfigData {
            provider: provider.to_string(),
            model: model.to_string(),
            api_key: None,
            base_url: None,
        }),
        tasks: Vec::new(),
        parent_id: None,
        tenant_id: None,
        principal: None,
        agent_template_id: None,
        correlation_id: None,
        cognitive_package_binding: None,
        immutable_content_adapter_binding: None,
    }
}

fn snapshot_for(session_id: &str) -> SessionSnapshotV1 {
    let mut data = persisted_data(None, None);
    data.id = session_id.to_string();
    SessionSnapshotV1::session_only(data)
}

fn run_record(
    session_id: &str,
    run_id: &str,
    sequences: &[usize],
    event_count: usize,
) -> RunRecord {
    RunRecord {
        snapshot: RunSnapshot {
            id: run_id.to_string(),
            session_id: session_id.to_string(),
            status: RunStatus::Completed,
            prompt: "persisted run".to_string(),
            cognitive_package_binding: None,
            capability_binding: None,
            created_at_ms: 1,
            updated_at_ms: 2,
            result_text: Some("done".to_string()),
            error: None,
            event_count,
            workspace_change_set: None,
        },
        events: sequences
            .iter()
            .map(|sequence| RunEventRecord {
                sequence: *sequence,
                timestamp_ms: *sequence as u64,
                event: AgentEvent::TextDelta {
                    text: format!("event-{sequence}"),
                },
            })
            .collect(),
    }
}

fn transform_bound_run_record(
    session_id: &str,
    run_id: &str,
    policy: &crate::tools::ToolResultTransformPolicyV1,
) -> RunRecord {
    let binding = crate::tools::ToolResultTransformBindingV1::from_policy(policy).unwrap();
    let metadata = crate::tools::attach_tool_result_evidence_with_transform_binding(
        None,
        "complete Tool result",
        "projected Tool result",
        crate::tools::ToolResultLossModeV1::HeadTail,
        &binding,
    )
    .unwrap();
    RunRecord {
        snapshot: RunSnapshot {
            id: run_id.to_string(),
            session_id: session_id.to_string(),
            status: RunStatus::Completed,
            prompt: "persisted run".to_string(),
            cognitive_package_binding: None,
            capability_binding: None,
            created_at_ms: 1,
            updated_at_ms: 2,
            result_text: Some("done".to_string()),
            error: None,
            event_count: 1,
            workspace_change_set: None,
        },
        events: vec![RunEventRecord {
            sequence: 0,
            timestamp_ms: 1,
            event: AgentEvent::ToolEnd {
                id: "tool-1".to_string(),
                name: "test".to_string(),
                args: None,
                output: "projected Tool result".to_string(),
                exit_code: 0,
                metadata: Some(metadata),
                error_kind: None,
            },
        }],
    }
}

fn subagent_task(parent_session_id: &str) -> SubagentTaskSnapshot {
    SubagentTaskSnapshot {
        task_id: "task-1".to_string(),
        parent_session_id: parent_session_id.to_string(),
        child_session_id: "child-1".to_string(),
        agent: "test".to_string(),
        description: "persisted task".to_string(),
        status: SubagentStatus::Completed,
        started_ms: 1,
        updated_ms: 2,
        finished_ms: Some(2),
        output: Some("done".to_string()),
        success: Some(true),
        source_anchors: Vec::new(),
        progress: Vec::new(),
    }
}

async fn load_from_returning_store(
    requested_id: &str,
    snapshot: SessionSnapshotV1,
) -> Result<SessionSnapshotV1> {
    let store: Arc<dyn SessionStore> = Arc::new(ReturningSnapshotStore { snapshot });
    load_session_snapshot(&store, requested_id).await
}

#[tokio::test]
async fn load_rejects_custom_store_snapshot_for_another_session() {
    let error = load_from_returning_store("session-a", snapshot_for("session-b"))
        .await
        .expect_err("custom store must not substitute another session payload");

    let message = error.to_string();
    assert!(message.contains("session-a"), "{message}");
    assert!(message.contains("session-b"), "{message}");
    assert!(message.contains("snapshot payload"), "{message}");
}

#[tokio::test]
async fn load_rejects_cross_session_run_record() {
    let mut snapshot = snapshot_for("session-a");
    snapshot.run_records = vec![run_record("session-b", "run-1", &[0], 1)];

    let error = load_from_returning_store("session-a", snapshot)
        .await
        .expect_err("run records must belong to their snapshot session");
    let message = error.to_string();
    assert!(message.contains("run-1"), "{message}");
    assert!(message.contains("session-a"), "{message}");
    assert!(message.contains("session-b"), "{message}");
}

#[tokio::test]
async fn load_rejects_duplicate_run_event_sequence() {
    let mut snapshot = snapshot_for("session-a");
    snapshot.run_records = vec![run_record("session-a", "run-1", &[7, 7], 8)];

    let error = load_from_returning_store("session-a", snapshot)
        .await
        .expect_err("duplicate replay sequence must be rejected");
    let message = error.to_string();
    assert!(message.contains("run-1"), "{message}");
    assert!(message.contains("not strictly greater"), "{message}");
}

#[tokio::test]
async fn load_accepts_trimmed_run_event_sequence_and_preserves_cursor() {
    let mut snapshot = snapshot_for("session-a");
    snapshot.run_records = vec![run_record("session-a", "run-1", &[7, 8, 9], 10)];

    let loaded = load_from_returning_store("session-a", snapshot)
        .await
        .expect("FIFO-trimmed retained events are valid");
    let record = &loaded.run_records[0];
    assert_eq!(record.snapshot.event_count, 10);
    assert_eq!(
        record
            .events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![7, 8, 9]
    );
}

#[tokio::test]
async fn load_rejects_duplicate_run_id() {
    let mut snapshot = snapshot_for("session-a");
    snapshot.run_records = vec![
        run_record("session-a", "run-1", &[0], 1),
        run_record("session-a", "run-1", &[1], 2),
    ];

    let error = load_from_returning_store("session-a", snapshot)
        .await
        .expect_err("run ids must be unique within a snapshot");
    let message = error.to_string();
    assert!(message.contains("duplicate run id"), "{message}");
    assert!(message.contains("run-1"), "{message}");
}

#[tokio::test]
async fn load_rejects_tampered_immutable_content_binding() {
    let mut snapshot = snapshot_for("session-a");
    let mut binding = immutable_content_binding('c');
    binding.maximum_bytes += 1;
    snapshot.session.immutable_content_adapter_binding = Some(binding);

    let error = load_from_returning_store("session-a", snapshot)
        .await
        .expect_err("tampered immutable-content authority must not load");
    let message = error.to_string();
    assert!(
        message.contains("immutable-content adapter binding"),
        "{message}"
    );
    assert!(message.contains("binding_digest"), "{message}");
}

#[tokio::test]
async fn load_rejects_tool_result_transform_policy_drift() {
    let retained_policy = crate::tools::ToolResultTransformPolicyV1::context_efficient();
    let mut snapshot = snapshot_for("session-a");
    snapshot.run_records = vec![transform_bound_run_record(
        "session-a",
        "run-1",
        &retained_policy,
    )];
    snapshot.session.config.tool_result_transform_policy =
        crate::tools::ToolResultTransformPolicyV1::conservative();

    let error = load_from_returning_store("session-a", snapshot)
        .await
        .expect_err("a retained Tool transform must match the exact Session policy");
    let message = error.to_string();
    assert!(
        message.contains("Tool result transform binding"),
        "{message}"
    );
    assert!(message.contains("run-1"), "{message}");
}

#[tokio::test]
async fn load_accepts_exact_tool_result_transform_binding() {
    let policy = crate::tools::ToolResultTransformPolicyV1::context_efficient();
    let mut snapshot = snapshot_for("session-a");
    snapshot.session.config.tool_result_transform_policy = policy.clone();
    snapshot.run_records = vec![transform_bound_run_record("session-a", "run-1", &policy)];

    load_from_returning_store("session-a", snapshot)
        .await
        .expect("exact Tool transform evidence must remain replayable");
}

#[tokio::test]
async fn load_rejects_event_count_behind_retained_sequence() {
    let mut snapshot = snapshot_for("session-a");
    snapshot.run_records = vec![run_record("session-a", "run-1", &[7, 8, 9], 9)];

    let error = load_from_returning_store("session-a", snapshot)
        .await
        .expect_err("event_count must cover the highest retained sequence");
    let message = error.to_string();
    assert!(message.contains("event_count 9"), "{message}");
    assert!(message.contains("expected at least 10"), "{message}");
}

#[tokio::test]
async fn load_validates_known_subagent_parent_but_accepts_legacy_unknown_parent() {
    let mut legacy_snapshot = snapshot_for("session-a");
    legacy_snapshot.subagent_tasks = vec![subagent_task("")];
    load_from_returning_store("session-a", legacy_snapshot)
        .await
        .expect("legacy task snapshots can lack a parent id");

    let mut foreign_snapshot = snapshot_for("session-a");
    foreign_snapshot.subagent_tasks = vec![subagent_task("session-b")];
    let error = load_from_returning_store("session-a", foreign_snapshot)
        .await
        .expect_err("known subagent parent must match snapshot session");
    let message = error.to_string();
    assert!(message.contains("task-1"), "{message}");
    assert!(message.contains("session-a"), "{message}");
    assert!(message.contains("session-b"), "{message}");
}

#[test]
fn persisted_runtime_options_prefer_llm_config() {
    let data = persisted_data(Some("anthropic/old"), Some(("openai", "gpt-4o")));
    let opts = apply_persisted_runtime_options(SessionOptions::new(), &data).unwrap();
    assert_eq!(opts.session_id.as_deref(), Some("session-1"));
    assert_eq!(opts.model.as_deref(), Some("openai/gpt-4o"));
}

#[test]
fn persisted_runtime_options_fall_back_to_model_name() {
    let data = persisted_data(Some("openai/gpt-4o"), None);
    let opts = apply_persisted_runtime_options(SessionOptions::new(), &data).unwrap();
    assert_eq!(opts.model.as_deref(), Some("openai/gpt-4o"));
}

#[test]
fn persisted_tool_result_policy_is_inherited_and_cannot_drift() {
    let mut data = persisted_data(Some("openai/gpt-4o"), None);
    let policy = crate::tools::ToolResultTransformPolicyV1::context_efficient();
    data.config.tool_result_transform_policy = policy.clone();

    let inherited = apply_persisted_runtime_options(SessionOptions::new(), &data).unwrap();
    assert_eq!(inherited.tool_result_transform_policy, Some(policy.clone()));

    let matching = apply_persisted_runtime_options(
        SessionOptions::new().with_tool_result_transform_policy(policy),
        &data,
    )
    .unwrap();
    assert_eq!(
        matching.tool_result_transform_policy,
        Some(crate::tools::ToolResultTransformPolicyV1::context_efficient())
    );

    let error = apply_persisted_runtime_options(
        SessionOptions::new().with_tool_result_transform_policy(
            crate::tools::ToolResultTransformPolicyV1::conservative(),
        ),
        &data,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CodeError::SessionConfiguration {
            field: "tool_result_transform_policy",
            ..
        }
    ));
}

#[test]
fn persisted_immutable_content_adapter_requires_exact_host_reinjection() {
    let mut data = persisted_data(Some("openai/gpt-4o"), None);
    let binding = immutable_content_binding('a');
    data.immutable_content_adapter_binding = Some(binding.clone());

    let missing = apply_persisted_runtime_options(SessionOptions::new(), &data).unwrap_err();
    assert!(matches!(
        missing,
        CodeError::SessionConfiguration {
            field: "immutable_content_adapter",
            ..
        }
    ));

    let drifted = apply_persisted_runtime_options(
        SessionOptions::new().with_immutable_content_adapter(immutable_content_session(
            immutable_content_binding('b'),
        )),
        &data,
    )
    .unwrap_err();
    assert!(matches!(
        drifted,
        CodeError::SessionConfiguration {
            field: "immutable_content_adapter",
            ..
        }
    ));

    let exact = apply_persisted_runtime_options(
        SessionOptions::new()
            .with_immutable_content_adapter(immutable_content_session(binding.clone())),
        &data,
    )
    .unwrap();
    assert_eq!(
        exact
            .immutable_content_adapter
            .as_ref()
            .map(crate::tools::ImmutableContentAdapterSession::binding),
        Some(&binding)
    );

    let unbound = persisted_data(Some("openai/gpt-4o"), None);
    let acquired = apply_persisted_runtime_options(
        SessionOptions::new().with_immutable_content_adapter(immutable_content_session(binding)),
        &unbound,
    )
    .unwrap_err();
    assert!(matches!(
        acquired,
        CodeError::SessionConfiguration {
            field: "immutable_content_adapter",
            ..
        }
    ));
}

#[test]
fn persisted_tool_presentation_profile_is_inherited_and_cannot_drift() {
    let mut data = persisted_data(Some("openai/gpt-4o"), None);
    let profile = crate::tools::ToolPresentationProfileV1::code();
    data.config.tool_presentation_profile = profile.clone();

    let inherited = apply_persisted_runtime_options(SessionOptions::new(), &data).unwrap();
    assert_eq!(inherited.tool_presentation_profile, Some(profile.clone()));

    let matching = apply_persisted_runtime_options(
        SessionOptions::new().with_tool_presentation_profile(profile),
        &data,
    )
    .unwrap();
    assert_eq!(
        matching.tool_presentation_profile,
        Some(crate::tools::ToolPresentationProfileV1::code())
    );

    let error = apply_persisted_runtime_options(
        SessionOptions::new()
            .with_tool_presentation_profile(crate::tools::ToolPresentationProfileV1::direct()),
        &data,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CodeError::SessionConfiguration {
            field: "tool_presentation_profile",
            ..
        }
    ));
}

#[test]
fn persisted_context_window_does_not_override_an_explicit_model_change() {
    let mut data = persisted_data(Some("openai/gpt-4o"), None);
    data.config.max_context_length = 128_000;

    let restored = apply_persisted_runtime_options(SessionOptions::new(), &data).unwrap();
    assert_eq!(restored.max_context_tokens, Some(128_000));

    let switched = apply_persisted_runtime_options(
        SessionOptions::new().with_model("anthropic/claude"),
        &data,
    )
    .unwrap();
    assert_eq!(switched.max_context_tokens, None);

    let overridden = apply_persisted_runtime_options(
        SessionOptions::new().with_max_context_tokens(64_000),
        &data,
    )
    .unwrap();
    assert_eq!(overridden.max_context_tokens, Some(64_000));
}

#[test]
fn model_config_never_persists_secret_material() {
    let data = model_config_data("openai/gpt-4o").expect("model config");
    assert_eq!(data.provider, "openai");
    assert_eq!(data.model, "gpt-4o");
    assert!(data.api_key.is_none());
    assert!(data.base_url.is_none());
}

#[tokio::test]
async fn session_save_uses_exactly_one_aggregate_store_call() {
    let concrete_store = Arc::new(SnapshotOnlyStore::default());
    let session_store: Arc<dyn SessionStore> = concrete_store.clone();
    let context = SessionPersistenceContext {
        session_store: Some(session_store),
        session_id: "aggregate-save-test".to_string(),
        workspace: PathBuf::from("/tmp/aggregate-save-test"),
        config: AgentConfig::default(),
        model_name: "openai/test-model".to_string(),
        tool_executor: Arc::new(ToolExecutor::new("/tmp/aggregate-save-test".to_string())),
        trace_sink: crate::trace::InMemoryTraceSink::new(),
        run_store: Arc::new(crate::run::InMemoryRunStore::new()),
        history: Arc::new(RwLock::new(vec![Message::user("persist me")])),
        verification_reports: Arc::new(RwLock::new(Vec::new())),
        subagent_tasks: Arc::new(crate::subagent_task_tracker::InMemorySubagentTaskTracker::new()),
        persistence_state: Arc::new(RwLock::new(SessionPersistenceState::default())),
        tenant_id: None,
        principal: None,
        agent_template_id: None,
        correlation_id: None,
        capability_catalog: None,
        cognitive_package_binding: None,
        immutable_content_adapter_binding: None,
        tool_result_transform_policy: crate::tools::ToolResultTransformPolicyV1::default(),
        auto_save: false,
    };

    context.save().await.unwrap();

    assert_eq!(
        concrete_store
            .aggregate_saves
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        concrete_store
            .legacy_saves
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[tokio::test]
async fn repeated_saves_preserve_restored_metadata_usage_cost_and_tasks() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let session_store: Arc<dyn SessionStore> = store.clone();
    let mut baseline = persisted_data(Some("openai/old-model"), None);
    baseline.id = "lossless-save".to_string();
    baseline.config.name = "Important session".to_string();
    baseline.config.storage_type = crate::config::StorageBackend::Custom;
    baseline.config.parent_id = Some("parent-session".to_string());
    baseline.context_usage = ContextUsage {
        used_tokens: 900,
        max_tokens: 10_000,
        percent: 0.09,
        turns: 7,
    };
    baseline.total_usage = crate::llm::TokenUsage {
        prompt_tokens: 600,
        completion_tokens: 300,
        total_tokens: 900,
        cache_read_tokens: Some(25),
        cache_write_tokens: None,
    };
    baseline.total_cost = 1.25;
    baseline.created_at = 42;
    baseline.tasks = vec![crate::planning::Task::new("task-1", "preserve me")];

    let persistence_state = Arc::new(RwLock::new(SessionPersistenceState::default()));
    write_or_recover(&persistence_state).restore(baseline);
    write_or_recover(&persistence_state).record_usage(&crate::llm::TokenUsage {
        prompt_tokens: 10,
        completion_tokens: 5,
        total_tokens: 15,
        cache_read_tokens: Some(2),
        cache_write_tokens: Some(3),
    });
    let context = SessionPersistenceContext {
        session_store: Some(session_store.clone()),
        session_id: "lossless-save".to_string(),
        workspace: PathBuf::from("/tmp/lossless-save"),
        config: AgentConfig::default(),
        model_name: "openai/new-model".to_string(),
        tool_executor: Arc::new(ToolExecutor::new("/tmp/lossless-save".to_string())),
        trace_sink: crate::trace::InMemoryTraceSink::new(),
        run_store: Arc::new(crate::run::InMemoryRunStore::new()),
        history: Arc::new(RwLock::new(vec![Message::user("new history")])),
        verification_reports: Arc::new(RwLock::new(Vec::new())),
        subagent_tasks: Arc::new(crate::subagent_task_tracker::InMemorySubagentTaskTracker::new()),
        persistence_state,
        tenant_id: None,
        principal: None,
        agent_template_id: None,
        correlation_id: None,
        capability_catalog: None,
        cognitive_package_binding: None,
        immutable_content_adapter_binding: None,
        tool_result_transform_policy: crate::tools::ToolResultTransformPolicyV1::context_efficient(
        ),
        auto_save: false,
    };

    context.save().await.unwrap();
    context.save().await.unwrap();
    let saved = session_store
        .load_snapshot("lossless-save")
        .await
        .unwrap()
        .unwrap()
        .session;

    assert_eq!(saved.config.name, "Important session");
    assert_eq!(
        saved.config.storage_type,
        crate::config::StorageBackend::Custom
    );
    assert_eq!(saved.config.parent_id.as_deref(), Some("parent-session"));
    assert_eq!(saved.context_usage.used_tokens, 900);
    assert_eq!(saved.total_usage.prompt_tokens, 610);
    assert_eq!(saved.total_usage.completion_tokens, 305);
    assert_eq!(saved.total_usage.total_tokens, 915);
    assert_eq!(saved.total_usage.cache_read_tokens, Some(27));
    assert_eq!(saved.total_usage.cache_write_tokens, Some(3));
    assert_eq!(saved.total_cost, 1.25);
    assert_eq!(saved.created_at, 42);
    assert_eq!(saved.tasks.len(), 1);
    assert_eq!(saved.tasks[0].content, "preserve me");
    assert_eq!(saved.messages[0].text(), "new history");
    assert_eq!(saved.model_name.as_deref(), Some("openai/new-model"));
    assert_eq!(
        saved.config.tool_result_transform_policy,
        crate::tools::ToolResultTransformPolicyV1::context_efficient()
    );
}
