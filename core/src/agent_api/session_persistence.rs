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
    capability_catalog: Option<Arc<crate::capability::CapabilityCatalog>>,
    /// Session-static or resume-seed fallback. The current projected
    /// Knowledge value, when present, is sampled from `capability_catalog` at
    /// the save boundary instead of being captured before Run admission.
    cognitive_package_binding: Option<crate::cognitive_context::CognitivePackageBindingV1>,
    immutable_content_adapter_binding: Option<crate::tools::ImmutableContentAdapterBindingV1>,
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
            capability_catalog: Some(Arc::clone(&session.capability_catalog)),
            cognitive_package_binding: session
                .cognitive_context
                .as_ref()
                .map(|context| context.binding().clone()),
            immutable_content_adapter_binding: session
                .tool_executor
                .registry()
                .immutable_content_adapter()
                .map(|adapter| adapter.binding().clone()),
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

        let snapshot = self.capture_snapshot().await?;
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

    /// Materialize one validated semantic generation without publishing it to
    /// the configured Session store. This ordinary Session view samples the
    /// current catalog binding that the next Run would observe.
    pub(super) async fn capture_snapshot(&self) -> Result<SessionSnapshotV1> {
        let cognitive_package_binding = self.current_cognitive_package_binding();
        self.capture_snapshot_with_cognitive_binding(cognitive_package_binding)
            .await
    }

    /// Materialize the semantic half of an exact live checkpoint.
    ///
    /// A Session catalog may cut over while an older Run is still active. The
    /// logical checkpoint belongs to that admitted Run, so its portable
    /// Session snapshot must retain the source Run's frozen cognitive
    /// authority rather than sampling the generation visible to the next
    /// Run. `None` is also authoritative here: an unbound source Run must not
    /// acquire a newly installed Knowledge provider during recovery.
    pub(super) async fn capture_checkpoint_snapshot(
        &self,
        checkpoint: &crate::loop_checkpoint::LoopCheckpoint,
    ) -> Result<SessionSnapshotV1> {
        checkpoint
            .ensure_owned_by(&checkpoint.run_id, &self.session_id)
            .map_err(|error| {
                CodeError::Session(format!(
                    "Refusing live checkpoint for session {}: {error:#}",
                    self.session_id
                ))
            })?;
        let source_run = self
            .run_store
            .snapshot(&checkpoint.run_id)
            .await
            .ok_or_else(|| {
                CodeError::Session(format!(
                    "Refusing live checkpoint for missing source Run '{}' in session {}",
                    checkpoint.run_id, self.session_id
                ))
            })?;
        if source_run.session_id != self.session_id {
            return Err(CodeError::Session(format!(
                "Refusing live checkpoint for source Run '{}' owned by session '{}' instead of '{}'",
                checkpoint.run_id, source_run.session_id, self.session_id
            )));
        }
        self.capture_snapshot_with_cognitive_binding(source_run.cognitive_package_binding)
            .await
    }

    async fn capture_snapshot_with_cognitive_binding(
        &self,
        cognitive_package_binding: Option<crate::cognitive_context::CognitivePackageBindingV1>,
    ) -> Result<SessionSnapshotV1> {
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
            cognitive_package_binding: cognitive_package_binding.as_ref(),
            immutable_content_adapter_binding: self.immutable_content_adapter_binding.as_ref(),
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
        Ok(snapshot)
    }

    fn current_cognitive_package_binding(
        &self,
    ) -> Option<crate::cognitive_context::CognitivePackageBindingV1> {
        self.capability_catalog
            .as_ref()
            .and_then(|catalog| {
                let projection = catalog.pin();
                let binding = projection
                    .projection()
                    .iter()
                    .find_map(|(_, value)| match value {
                        crate::capability::CapabilityValue::Knowledge(knowledge) => {
                            Some(knowledge.binding().clone())
                        }
                        _ => None,
                    });
                binding
            })
            .or_else(|| self.cognitive_package_binding.clone())
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
    match opts.tool_presentation_profile.as_ref() {
        Some(requested) if requested != &data.config.tool_presentation_profile => {
            return Err(CodeError::SessionConfiguration {
                field: "tool_presentation_profile",
                message: "resume profile differs from the exact Tool presentation profile retained by the session snapshot".to_string(),
            });
        }
        Some(_) => {}
        None => {
            opts.tool_presentation_profile = Some(data.config.tool_presentation_profile.clone());
        }
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
        data.immutable_content_adapter_binding.as_ref(),
        opts.immutable_content_adapter.as_ref(),
    ) {
        (Some(persisted), Some(injected)) if injected.binding() == persisted => {}
        (Some(_), Some(_)) => {
            return Err(CodeError::SessionConfiguration {
                field: "immutable_content_adapter",
                message: "resume adapter binding differs from the exact immutable-content authority retained by the session snapshot".to_string(),
            });
        }
        (Some(_), None) => {
            return Err(CodeError::SessionConfiguration {
                field: "immutable_content_adapter",
                message: "resuming an immutable-content-bound session requires the host to re-inject an adapter for the persisted exact authority".to_string(),
            });
        }
        (None, Some(_)) => {
            return Err(CodeError::SessionConfiguration {
                field: "immutable_content_adapter",
                message: "an existing unbound session cannot acquire an immutable-content authority during resume; create a new bound session".to_string(),
            });
        }
        (None, None) => {}
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
    immutable_content_adapter_binding: Option<&'a crate::tools::ImmutableContentAdapterBindingV1>,
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
        immutable_content_adapter_binding: None,
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
    data.config.tool_presentation_profile = input.config.tool_presentation_profile.clone();
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
    data.immutable_content_adapter_binding = input.immutable_content_adapter_binding.cloned();
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
mod tests;
