//! Persisted session runtime contract.
//!
//! This module owns how an in-memory session becomes one `SessionSnapshotV1`
//! generation and how that same generation rehydrates runtime state.

use super::{AgentSession, SessionOptions};
use crate::agent::{AgentConfig, AgentResult};
use crate::error::{read_or_recover, write_or_recover, CodeError, Result};
use crate::llm::Message;
use crate::store::{LlmConfigData, SessionData, SessionSnapshotV1, SessionStore};
use crate::tools::ToolExecutor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Mutable persisted fields that are not otherwise represented by a live
/// `AgentSession` field. Keeping them together prevents each save generation
/// from rebuilding usage, tasks, cost data, and creation metadata from empty
/// defaults.
#[derive(Default)]
pub(super) struct SessionPersistenceState {
    baseline: Option<SessionData>,
    total_usage: crate::llm::TokenUsage,
    tasks: Vec<crate::planning::Task>,
}

#[derive(Clone)]
struct SessionPersistenceSeed {
    baseline: Option<SessionData>,
    total_usage: crate::llm::TokenUsage,
    tasks: Vec<crate::planning::Task>,
}

impl SessionPersistenceState {
    pub(super) fn record_usage(&mut self, usage: &crate::llm::TokenUsage) {
        self.total_usage.prompt_tokens = self
            .total_usage
            .prompt_tokens
            .saturating_add(usage.prompt_tokens);
        self.total_usage.completion_tokens = self
            .total_usage
            .completion_tokens
            .saturating_add(usage.completion_tokens);
        self.total_usage.total_tokens = self
            .total_usage
            .total_tokens
            .saturating_add(usage.total_tokens);
        self.total_usage.cache_read_tokens =
            add_optional_usage(self.total_usage.cache_read_tokens, usage.cache_read_tokens);
        self.total_usage.cache_write_tokens = add_optional_usage(
            self.total_usage.cache_write_tokens,
            usage.cache_write_tokens,
        );
    }

    pub(super) fn replace_tasks(&mut self, tasks: Vec<crate::planning::Task>) {
        self.tasks = tasks;
    }

    fn restore(&mut self, session: SessionData) {
        self.total_usage = session.total_usage.clone();
        self.tasks = session.tasks.clone();
        self.baseline = Some(session);
    }

    fn seed(&self) -> SessionPersistenceSeed {
        SessionPersistenceSeed {
            baseline: self.baseline.clone(),
            total_usage: self.total_usage.clone(),
            tasks: self.tasks.clone(),
        }
    }

    fn record_saved(&mut self, session: SessionData) {
        self.baseline = Some(session);
    }
}

fn add_optional_usage(left: Option<usize>, right: Option<usize>) -> Option<usize> {
    match (left, right) {
        (None, None) => None,
        (left, right) => Some(
            left.unwrap_or_default()
                .saturating_add(right.unwrap_or_default()),
        ),
    }
}

#[derive(Clone)]
pub(super) struct SessionPersistenceContext {
    session_store: Option<Arc<dyn SessionStore>>,
    session_id: String,
    workspace: PathBuf,
    config: AgentConfig,
    model_name: String,
    tool_executor: Arc<ToolExecutor>,
    trace_sink: crate::trace::InMemoryTraceSink,
    run_store: Arc<crate::run::InMemoryRunStore>,
    history: Arc<RwLock<Vec<Message>>>,
    verification_reports: Arc<RwLock<Vec<crate::verification::VerificationReport>>>,
    subagent_tasks: Arc<crate::subagent_task_tracker::InMemorySubagentTaskTracker>,
    persistence_state: Arc<RwLock<SessionPersistenceState>>,
    tenant_id: Option<String>,
    principal: Option<String>,
    agent_template_id: Option<String>,
    correlation_id: Option<String>,
    cognitive_package_binding: Option<crate::cognitive_context::CognitivePackageBindingV1>,
    tool_result_transform_policy: crate::tools::ToolResultTransformPolicyV1,
    auto_save: bool,
}

impl SessionPersistenceContext {
    pub(super) fn from_session(session: &AgentSession) -> Self {
        Self {
            session_store: session.session_store.clone(),
            session_id: session.session_id.clone(),
            workspace: session.workspace.clone(),
            config: session.config.clone(),
            model_name: session.model_name.clone(),
            tool_executor: Arc::clone(&session.tool_executor),
            trace_sink: session.trace_sink.clone(),
            run_store: Arc::clone(&session.run_store),
            history: Arc::clone(&session.history),
            verification_reports: Arc::clone(&session.verification_reports),
            subagent_tasks: Arc::clone(&session.subagent_tasks),
            persistence_state: Arc::clone(&session.persistence_state),
            tenant_id: session.tenant_id.clone(),
            principal: session.principal.clone(),
            agent_template_id: session.agent_template_id.clone(),
            correlation_id: session.correlation_id.clone(),
            cognitive_package_binding: session
                .cognitive_context
                .as_ref()
                .map(|context| context.binding().clone()),
            tool_result_transform_policy: session.tool_result_transform_policy.clone(),
            auto_save: session.auto_save,
        }
    }

    pub(super) fn record_result(&self, result: &AgentResult) {
        *write_or_recover(&self.history) = result.messages.clone();
        if !result.verification_reports.is_empty() {
            write_or_recover(&self.verification_reports)
                .extend(result.verification_reports.clone());
        }
    }

    pub(super) async fn save(&self) -> Result<()> {
        let store = match &self.session_store {
            Some(store) => store,
            None => return Ok(()),
        };

        let history = read_or_recover(&self.history).clone();
        let verification_reports = read_or_recover(&self.verification_reports).clone();
        let seed = read_or_recover(&self.persistence_state).seed();
        let data = build_session_data_snapshot(SessionDataSnapshotInput {
            session_id: &self.session_id,
            workspace: &self.workspace,
            config: &self.config,
            model_name: &self.model_name,
            history,
            tenant_id: self.tenant_id.as_deref(),
            principal: self.principal.as_deref(),
            agent_template_id: self.agent_template_id.as_deref(),
            correlation_id: self.correlation_id.as_deref(),
            cognitive_package_binding: self.cognitive_package_binding.as_ref(),
            tool_result_transform_policy: &self.tool_result_transform_policy,
            seed,
        })
        .await;

        let snapshot = SessionSnapshotV1::new(
            data,
            &self.tool_executor.artifact_store(),
            self.trace_sink.events(),
            self.run_store.records().await,
            verification_reports,
            self.subagent_tasks.list().await,
        );
        snapshot
            .validate_for_session(&self.session_id)
            .map_err(|error| {
                CodeError::Session(format!(
                    "Refusing to save invalid session snapshot {}: {error:#}",
                    self.session_id
                ))
            })?;
        store.save_snapshot(&snapshot).await.map_err(|error| {
            CodeError::Session(format!(
                "Failed to save session {}: {error:#}",
                self.session_id
            ))
        })?;
        write_or_recover(&self.persistence_state).record_saved(snapshot.session.clone());
        tracing::debug!("Session {} saved", self.session_id);
        Ok(())
    }

    pub(super) async fn auto_save_if_enabled(&self) {
        if self.auto_save {
            if let Err(e) = self.save().await {
                tracing::warn!("Auto-save failed for session {}: {}", self.session_id, e);
            }
        }
    }

    /// Delete the loop checkpoint for `run_id` once the run has reached a
    /// terminal state in-process. The checkpoint exists only to survive a
    /// process crash; once the run returns (completed / failed / cancelled)
    /// it is dead weight. No-op when no store is configured. Errors are
    /// warn-logged — a failed cleanup must never mask the run's result.
    pub(super) async fn clear_loop_checkpoint(&self, run_id: &str) {
        let Some(store) = &self.session_store else {
            return;
        };
        if let Err(e) = store.delete_loop_checkpoint(run_id).await {
            tracing::warn!(
                run_id = %run_id,
                session_id = %self.session_id,
                "Failed to delete loop checkpoint on run completion: {}",
                e
            );
        }
    }
}

pub(super) async fn load_session_snapshot(
    store: &Arc<dyn SessionStore>,
    session_id: &str,
) -> Result<SessionSnapshotV1> {
    let snapshot = store.load_snapshot(session_id).await.map_err(|error| {
        CodeError::Session(format!("Failed to load session {session_id}: {error:#}"))
    })?;

    let snapshot =
        snapshot.ok_or_else(|| CodeError::Session(format!("Session not found: {}", session_id)))?;
    snapshot.validate_for_session(session_id).map_err(|error| {
        CodeError::Session(format!(
            "Refusing invalid snapshot returned for session {session_id}: {error:#}"
        ))
    })?;
    Ok(snapshot)
}

pub(super) fn apply_persisted_runtime_options(
    mut opts: SessionOptions,
    data: &SessionData,
) -> Result<SessionOptions> {
    let model_was_explicit = opts.model.is_some();
    opts.session_id = Some(data.id.clone());

    if opts.model.is_none() {
        opts.model = persisted_model_ref(data);
    }
    if opts.queue_config.is_none() {
        opts.queue_config = data.config.queue_config.clone();
    }
    if opts.confirmation_manager.is_none() && opts.confirmation_policy.is_none() {
        opts.confirmation_policy = data.config.confirmation_policy.clone();
    }
    if opts.permission_checker.is_none() && opts.permission_policy.is_none() {
        if let Some(policy) = data.config.permission_policy.clone() {
            opts = opts.with_permission_policy(policy);
        }
    }
    if opts.enforce_active_skill_tool_restrictions.is_none() {
        opts.enforce_active_skill_tool_restrictions =
            Some(data.config.enforce_active_skill_tool_restrictions);
    }
    if opts.max_parallel_tasks.is_none() {
        opts.max_parallel_tasks = data.config.max_parallel_tasks;
    }
    if opts.auto_delegation.is_none() {
        opts.auto_delegation = data.config.auto_delegation.clone();
    }
    if opts.max_context_tokens.is_none()
        && !model_was_explicit
        && data.config.max_context_length > 0
    {
        opts.max_context_tokens = Some(data.config.max_context_length as usize);
    }
    match opts.tool_result_transform_policy.as_ref() {
        Some(requested) if requested != &data.config.tool_result_transform_policy => {
            return Err(CodeError::SessionConfiguration {
                field: "tool_result_transform_policy",
                message: "resume policy differs from the exact Tool result transform policy retained by the session snapshot".to_string(),
            });
        }
        Some(_) => {}
        None => {
            opts.tool_result_transform_policy =
                Some(data.config.tool_result_transform_policy.clone());
        }
    }

    // Identity labels: caller-supplied values take precedence (the resume
    // caller may want to relabel for a new tenant/principal). Otherwise
    // restore from the persisted snapshot.
    if opts.tenant_id.is_none() {
        opts.tenant_id = data.tenant_id.clone();
    }
    if opts.principal.is_none() {
        opts.principal = data.principal.clone();
    }
    if opts.agent_template_id.is_none() {
        opts.agent_template_id = data.agent_template_id.clone();
    }
    if opts.correlation_id.is_none() {
        opts.correlation_id = data.correlation_id.clone();
    }

    match (
        data.cognitive_package_binding.as_ref(),
        opts.cognitive_context.as_ref(),
    ) {
        (Some(persisted), Some(injected)) if injected.binding() == persisted => {}
        (Some(_), Some(_)) => {
            return Err(CodeError::SessionConfiguration {
                field: "cognitive_context",
                message: "resume provider binding differs from the exact cognitive package retained by the session snapshot".to_string(),
            });
        }
        (Some(_), None) => {
            return Err(CodeError::SessionConfiguration {
                field: "cognitive_context",
                message: "resuming a cognitive-package session requires the host to re-inject a provider for the persisted exact generation".to_string(),
            });
        }
        (None, Some(_)) => {
            return Err(CodeError::SessionConfiguration {
                field: "cognitive_context",
                message: "an existing unbound session cannot acquire a cognitive package during resume; create a new bound session".to_string(),
            });
        }
        (None, None) => {}
    }

    Ok(opts)
}

pub(super) fn ensure_artifact_restore_capacity(
    opts: &mut SessionOptions,
    snapshot: &SessionSnapshotV1,
) {
    // ToolExecutor fixes its artifact limits during session construction, so
    // reserve enough capacity before building the resumed session. Explicit
    // caller limits remain effective whenever they are already larger.
    let requested = opts.artifact_store_limits.unwrap_or_default();
    let persisted = snapshot.artifact_store_requirements();
    opts.artifact_store_limits = Some(crate::tools::ArtifactStoreLimits {
        max_artifacts: requested.max_artifacts.max(persisted.max_artifacts),
        max_bytes: requested.max_bytes.max(persisted.max_bytes),
    });
}

pub(super) async fn restore_persisted_session_state(
    session: &AgentSession,
    snapshot: SessionSnapshotV1,
) -> Result<()> {
    snapshot
        .validate_for_session(&session.session_id)
        .map_err(|error| {
            CodeError::Session(format!(
                "Refusing to restore invalid snapshot into session {}: {error:#}",
                session.session_id
            ))
        })?;
    let restored_store = snapshot.artifact_store();
    write_or_recover(&session.persistence_state).restore(snapshot.session.clone());
    *write_or_recover(&session.history) = snapshot.session.messages;

    let target_store = session.tool_executor.artifact_store();
    for artifact in restored_store.artifacts() {
        target_store.put(artifact);
    }

    session.trace_sink.replace_events(snapshot.trace_events);
    session
        .run_store
        .replace_records(snapshot.run_records)
        .await;
    *write_or_recover(&session.verification_reports) = snapshot.verification_reports;
    session
        .subagent_tasks
        .replace_snapshots(snapshot.subagent_tasks)
        .await;

    Ok(())
}

struct SessionDataSnapshotInput<'a> {
    session_id: &'a str,
    workspace: &'a Path,
    config: &'a AgentConfig,
    model_name: &'a str,
    history: Vec<Message>,
    tenant_id: Option<&'a str>,
    principal: Option<&'a str>,
    agent_template_id: Option<&'a str>,
    correlation_id: Option<&'a str>,
    cognitive_package_binding: Option<&'a crate::cognitive_context::CognitivePackageBindingV1>,
    tool_result_transform_policy: &'a crate::tools::ToolResultTransformPolicyV1,
    seed: SessionPersistenceSeed,
}

async fn build_session_data_snapshot(input: SessionDataSnapshotInput<'_>) -> SessionData {
    let confirmation_policy = match &input.config.confirmation_manager {
        Some(manager) => Some(manager.policy().await),
        None => input.config.confirmation_policy.clone(),
    };
    let model_name = persisted_model_name(input.model_name);
    let now = chrono::Utc::now().timestamp();
    let mut data = input.seed.baseline.unwrap_or_else(|| SessionData {
        id: input.session_id.to_string(),
        config: crate::store::SessionConfig::default(),
        state: crate::store::SessionState::Active,
        messages: Vec::new(),
        context_usage: crate::store::ContextUsage::default(),
        total_usage: crate::llm::TokenUsage::default(),
        total_cost: 0.0,
        model_name: None,
        cost_records: Vec::new(),
        tool_names: Vec::new(),
        thinking_enabled: false,
        thinking_budget: None,
        created_at: now,
        updated_at: now,
        llm_config: None,
        tasks: Vec::new(),
        parent_id: None,
        tenant_id: None,
        principal: None,
        agent_template_id: None,
        correlation_id: None,
        cognitive_package_binding: None,
    });

    data.id = input.session_id.to_string();
    data.config.workspace = input.workspace.display().to_string();
    data.config.system_prompt = Some(input.config.prompt_slots.build());
    data.config.max_context_length = input.config.max_context_tokens.min(u32::MAX as usize) as u32;
    data.config.auto_compact = input.config.auto_compact;
    data.config.auto_compact_threshold = input.config.auto_compact_threshold;
    data.config.queue_config = input.config.queue_config.clone();
    data.config.confirmation_policy = confirmation_policy;
    data.config.permission_policy = input.config.permission_policy.clone();
    data.config.enforce_active_skill_tool_restrictions =
        input.config.enforce_active_skill_tool_restrictions;
    data.config.max_parallel_tasks = Some(input.config.max_parallel_tasks);
    data.config.auto_delegation = Some(input.config.auto_delegation.clone());
    data.config.planning_mode = input.config.planning_mode;
    data.config.goal_tracking = input.config.goal_tracking;
    data.config.tool_result_transform_policy = input.tool_result_transform_policy.clone();
    data.messages = input.history;
    data.total_usage = input.seed.total_usage;
    data.model_name = model_name;
    data.tool_names = SessionData::tool_names_from_definitions(&input.config.tools);
    data.updated_at = now;
    data.llm_config = model_config_data(input.model_name);
    data.tasks = input.seed.tasks;
    data.tenant_id = input.tenant_id.map(str::to_string);
    data.principal = input.principal.map(str::to_string);
    data.agent_template_id = input.agent_template_id.map(str::to_string);
    data.correlation_id = input.correlation_id.map(str::to_string);
    data.cognitive_package_binding = input.cognitive_package_binding.cloned();
    data
}

fn persisted_model_ref(data: &SessionData) -> Option<String> {
    if let Some(llm_config) = &data.llm_config {
        return Some(format!("{}/{}", llm_config.provider, llm_config.model));
    }
    data.model_name
        .as_ref()
        .filter(|model_name| model_name.contains('/'))
        .cloned()
}

fn persisted_model_name(model_name: &str) -> Option<String> {
    if model_name.is_empty() || model_name == "unknown" {
        None
    } else {
        Some(model_name.to_string())
    }
}

fn model_config_data(model_name: &str) -> Option<LlmConfigData> {
    let (provider, model) = model_name.split_once('/')?;
    Some(LlmConfigData {
        provider: provider.to_string(),
        model: model.to_string(),
        api_key: None,
        base_url: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentEvent;
    use crate::run::{RunEventRecord, RunRecord, RunSnapshot, RunStatus};
    use crate::store::{ContextUsage, SessionConfig, SessionState};
    use crate::subagent_task_tracker::{SubagentStatus, SubagentTaskSnapshot};

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
                created_at_ms: 1,
                updated_at_ms: 2,
                result_text: Some("done".to_string()),
                error: None,
                event_count,
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
            subagent_tasks: Arc::new(
                crate::subagent_task_tracker::InMemorySubagentTaskTracker::new(),
            ),
            persistence_state: Arc::new(RwLock::new(SessionPersistenceState::default())),
            tenant_id: None,
            principal: None,
            agent_template_id: None,
            correlation_id: None,
            cognitive_package_binding: None,
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
            subagent_tasks: Arc::new(
                crate::subagent_task_tracker::InMemorySubagentTaskTracker::new(),
            ),
            persistence_state,
            tenant_id: None,
            principal: None,
            agent_template_id: None,
            correlation_id: None,
            cognitive_package_binding: None,
            tool_result_transform_policy:
                crate::tools::ToolResultTransformPolicyV1::context_efficient(),
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
}
