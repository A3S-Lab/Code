//! Immutable identity, cancellation, event, and governance scope for one run.

use super::{AgentEvent, AgentLoop};
use crate::budget::BudgetGuard;
use crate::harness_evidence::{
    HarnessEvidenceError, ModelCallObservation, ModelInputSnapshotV1, ModelPresentationSnapshotV1,
    RunCapabilityEvidenceSource, RunCapabilitySnapshotV1, ToolResultContextUsageV1,
};
use crate::hitl::ConfirmationProvider;
use crate::permissions::PermissionChecker;
use crate::tools::{AgentEventBarrier, ToolContext};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

/// Governance resources snapshotted when a run starts.
///
/// Runtime overrides are resolved before this value is created. Keeping the
/// snapshot beside the run identity prevents helper LLM calls from silently
/// consulting a different budget scope halfway through a run.
#[derive(Clone, Default)]
pub(crate) struct InvocationGovernance {
    budget_guard: Option<Arc<dyn BudgetGuard>>,
    permission_checker: Option<Arc<dyn PermissionChecker>>,
    confirmation_manager: Option<Arc<dyn ConfirmationProvider>>,
}

impl InvocationGovernance {
    pub(crate) fn budget_guard(&self) -> Option<&Arc<dyn BudgetGuard>> {
        self.budget_guard.as_ref()
    }
}

fn snapshot_permission_checker(
    checker: Option<&Arc<dyn PermissionChecker>>,
) -> Option<Arc<dyn PermissionChecker>> {
    checker.map(|checker| {
        checker
            .snapshot_for_run()
            .unwrap_or_else(|| Arc::clone(checker))
    })
}

fn snapshot_confirmation_manager(
    provider: Option<&Arc<dyn ConfirmationProvider>>,
) -> Option<Arc<dyn ConfirmationProvider>> {
    provider.map(|provider| {
        provider
            .snapshot_for_run()
            .unwrap_or_else(|| Arc::clone(provider))
    })
}

/// The single source of truth for metadata shared by every operation in a run.
#[derive(Clone)]
pub(crate) struct InvocationContext {
    run_id: Arc<str>,
    session_id: Arc<str>,
    memory_context_incarnation: Arc<str>,
    cancellation: CancellationToken,
    event_tx: Option<mpsc::Sender<AgentEvent>>,
    retired_fact_event_tx: Option<mpsc::WeakSender<AgentEvent>>,
    agent_event_tx: Option<broadcast::Sender<AgentEvent>>,
    agent_event_barrier: Option<AgentEventBarrier>,
    governance: InvocationGovernance,
    run_control: Option<Arc<crate::run_control::RunControlInbox>>,
    model_evidence: Option<ModelEvidenceState>,
    memory_context_sequence: Arc<AtomicU64>,
}

#[derive(Clone)]
struct ModelEvidenceState {
    source: Arc<RunCapabilityEvidenceSource>,
    call_sequence: Arc<AtomicU64>,
    last_capability_digest: Arc<Mutex<Option<String>>>,
}

pub(crate) struct CapturedModelEvidence {
    pub(super) capability: RunCapabilitySnapshotV1,
    pub(super) presentation: ModelPresentationSnapshotV1,
    pub(super) input: ModelInputSnapshotV1,
    pub(super) tool_result_context: ToolResultContextUsageV1,
}

/// Sender-free material selected from the original invocation before spawn.
/// The source here is always the SAME source type's frozen environment.
#[derive(Clone)]
pub(crate) struct FactInvocationView {
    run_id: Arc<str>,
    session_id: Arc<str>,
    cancellation: CancellationToken,
    budget_guard: Option<Arc<dyn BudgetGuard>>,
    lineage: Arc<AtomicU64>,
    call_sequence: Arc<AtomicU64>,
    source: RunCapabilityEvidenceSource,
}

impl FactInvocationView {
    pub(super) fn session_id(&self) -> &str {
        &self.session_id
    }
    pub(super) fn run_id(&self) -> &str {
        &self.run_id
    }
    pub(super) fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }
    pub(super) fn budget_guard(&self) -> Option<&Arc<dyn BudgetGuard>> {
        self.budget_guard.as_ref()
    }

    pub(super) fn same_original(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.lineage, &other.lineage)
            && Arc::ptr_eq(&self.call_sequence, &other.call_sequence)
            && same_guard(self.budget_guard.as_ref(), other.budget_guard.as_ref())
    }

    pub(super) fn capture(
        &self,
        observation: ModelCallObservation<'_>,
    ) -> Result<CapturedModelEvidence, HarnessEvidenceError> {
        capture_evidence(&self.source, &self.call_sequence, observation)
    }
}

fn same_guard(left: Option<&Arc<dyn BudgetGuard>>, right: Option<&Arc<dyn BudgetGuard>>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => Arc::ptr_eq(left, right),
        _ => false,
    }
}

fn capture_evidence(
    source: &RunCapabilityEvidenceSource,
    sequence: &AtomicU64,
    observation: ModelCallObservation<'_>,
) -> Result<CapturedModelEvidence, HarnessEvidenceError> {
    let previous = sequence
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
            value.checked_add(1)
        })
        .map_err(|_| HarnessEvidenceError::CallSequenceExhausted)?;
    let (capability, presentation, input, tool_result_context) =
        source.capture_with_presentation(previous + 1, observation)?;
    Ok(CapturedModelEvidence {
        capability,
        presentation,
        input,
        tool_result_context,
    })
}

impl std::fmt::Debug for InvocationContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvocationContext")
            .field("run_id", &self.run_id)
            .field("session_id", &self.session_id)
            .field("cancelled", &self.cancellation.is_cancelled())
            .field("has_event_tx", &self.event_tx.is_some())
            .field("has_agent_event_tx", &self.agent_event_tx.is_some())
            .field(
                "has_agent_event_barrier",
                &self.agent_event_barrier.is_some(),
            )
            .field("has_budget_guard", &self.governance.budget_guard.is_some())
            .field(
                "has_permission_checker",
                &self.governance.permission_checker.is_some(),
            )
            .field(
                "has_confirmation_manager",
                &self.governance.confirmation_manager.is_some(),
            )
            .field("has_run_control", &self.run_control.is_some())
            .field("has_model_evidence", &self.model_evidence.is_some())
            .finish()
    }
}

impl InvocationContext {
    pub(crate) fn new(
        run_id: impl Into<Arc<str>>,
        session_id: impl Into<Arc<str>>,
        cancellation: CancellationToken,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
        governance: InvocationGovernance,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            session_id: session_id.into(),
            // Host IdGenerator values are guaranteed unique only within one
            // process. A Code-owned incarnation keeps durable-memory context
            // identities distinct when a resumed session legitimately reuses
            // an evicted run ID after another process starts.
            memory_context_incarnation: Arc::from(uuid::Uuid::new_v4().to_string()),
            cancellation,
            event_tx,
            retired_fact_event_tx: None,
            agent_event_tx: None,
            agent_event_barrier: None,
            governance,
            run_control: None,
            model_evidence: None,
            memory_context_sequence: Arc::new(AtomicU64::new(0)),
        }
    }

    fn with_model_evidence(mut self, source: RunCapabilityEvidenceSource) -> Self {
        self.model_evidence = Some(ModelEvidenceState {
            source: Arc::new(source),
            call_sequence: Arc::new(AtomicU64::new(0)),
            last_capability_digest: Arc::new(Mutex::new(None)),
        });
        self
    }

    /// Bind high-level tool events to the run that owns this invocation.
    ///
    /// The session-level channel remains available for direct session tools,
    /// but agent-run tools must use this sender so a background child from an
    /// earlier run cannot be attributed to a later run in the same session.
    #[allow(dead_code)]
    pub(crate) fn with_agent_events(
        mut self,
        tx: broadcast::Sender<AgentEvent>,
        barrier: AgentEventBarrier,
    ) -> Self {
        self.agent_event_tx = Some(tx);
        self.agent_event_barrier = Some(barrier);
        self
    }

    /// Bind the cooperative control inbox owned by this run.
    pub(crate) fn with_run_control(
        mut self,
        control: Arc<crate::run_control::RunControlInbox>,
    ) -> Self {
        self.run_control = Some(control);
        self
    }

    pub(crate) fn run_id(&self) -> &str {
        &self.run_id
    }

    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }

    pub(crate) fn session_id_option(&self) -> Option<&str> {
        (!self.session_id.is_empty()).then_some(self.session_id())
    }

    pub(crate) fn memory_context_incarnation(&self) -> &str {
        &self.memory_context_incarnation
    }

    pub(crate) fn next_memory_context_sequence(&self) -> Option<u64> {
        self.memory_context_sequence
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .ok()
            .map(|previous| previous + 1)
    }

    pub(crate) fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    pub(crate) fn run_control(&self) -> Option<Arc<crate::run_control::RunControlInbox>> {
        self.run_control.clone()
    }

    pub(crate) fn event_tx(&self) -> &Option<mpsc::Sender<AgentEvent>> {
        &self.event_tx
    }

    /// Release only this completed fact invocation's strong host sender.
    /// The original channel identity remains passive matching data; every
    /// identity, governance resource and evidence sequence stays unchanged.
    pub(super) fn retire_fact_host_events(mut self) -> Self {
        if let Some(actual) = self.event_tx.take() {
            self.retired_fact_event_tx = Some(actual.downgrade());
        }
        self
    }

    pub(super) fn matches_parts(
        &self,
        session_id: Option<&str>,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) -> bool {
        let same_events = match (&self.event_tx, &self.retired_fact_event_tx, event_tx) {
            (None, None, None) => true,
            (Some(bound), _, Some(requested)) => bound.same_channel(requested),
            (None, Some(bound), Some(requested)) => bound
                .upgrade()
                .is_some_and(|actual| actual.same_channel(requested)),
            _ => false,
        };
        self.session_id() == session_id.unwrap_or("") && same_events
    }

    pub(crate) fn governance(&self) -> &InvocationGovernance {
        &self.governance
    }

    pub(super) fn freeze_for_fact_attempt(&self) -> anyhow::Result<FactInvocationView> {
        let state = self.model_evidence.as_ref().ok_or_else(|| {
            anyhow::anyhow!("fact model invocation has no original evidence source")
        })?;
        Ok(FactInvocationView {
            run_id: Arc::clone(&self.run_id),
            session_id: Arc::clone(&self.session_id),
            cancellation: self.cancellation.clone(),
            budget_guard: self.governance.budget_guard.clone(),
            lineage: Arc::clone(&self.memory_context_sequence),
            call_sequence: Arc::clone(&state.call_sequence),
            source: state.source.freeze_for_fact_attempt(),
        })
    }

    pub(super) fn matches_fact_view(&self, view: &FactInvocationView) -> bool {
        Arc::ptr_eq(&self.memory_context_sequence, &view.lineage)
            && self
                .model_evidence
                .as_ref()
                .is_some_and(|state| Arc::ptr_eq(&state.call_sequence, &view.call_sequence))
            && same_guard(
                self.governance.budget_guard.as_ref(),
                view.budget_guard.as_ref(),
            )
    }

    pub(super) async fn fact_capability_is_current(&self, digest: &str) -> bool {
        let Some(state) = &self.model_evidence else {
            return false;
        };
        state.last_capability_digest.lock().await.as_deref() == Some(digest)
    }

    pub(super) async fn fact_capability_projected(&self, digest: String) {
        if let Some(state) = &self.model_evidence {
            *state.last_capability_digest.lock().await = Some(digest);
        }
    }

    pub(crate) fn capture_model_evidence(
        &self,
        observation: ModelCallObservation<'_>,
    ) -> Result<Option<CapturedModelEvidence>, HarnessEvidenceError> {
        let Some(state) = &self.model_evidence else {
            return Ok(None);
        };
        capture_evidence(&state.source, &state.call_sequence, observation).map(Some)
    }

    pub(crate) async fn send_capability_if_changed(
        &self,
        tx: &mpsc::Sender<AgentEvent>,
        call_sequence: u64,
        capability: RunCapabilitySnapshotV1,
    ) -> bool {
        let Some(state) = &self.model_evidence else {
            return true;
        };
        let digest = capability.snapshot_digest.clone();
        let mut last_digest = state.last_capability_digest.lock().await;
        if last_digest.as_deref() == Some(digest.as_str()) {
            return true;
        }
        let send_result = tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => return false,
            result = tx.send(AgentEvent::RunCapabilityBound {
                call_sequence,
                snapshot: capability,
            }) => result,
        };
        if send_result.is_ok() {
            *last_digest = Some(digest);
        }
        true
    }

    /// Install run identity and cancellation into a tool context before any
    /// direct, queued, nested, or delegated tool invocation begins.
    pub(crate) fn bind_tool_context(&self, mut context: ToolContext) -> ToolContext {
        if !self.session_id.is_empty() {
            context = context.with_session_id(self.session_id.to_string());
        }
        if !self.run_id.is_empty() {
            context = context.with_run_id(self.run_id.to_string());
        }
        if let Some(tx) = &self.agent_event_tx {
            context = context.with_agent_event_tx(tx.clone());
        }
        if let Some(barrier) = &self.agent_event_barrier {
            context = context.with_agent_event_barrier(barrier.clone());
        }
        context
            .with_run_governance(
                self.governance.permission_checker.clone(),
                self.governance.confirmation_manager.clone(),
            )
            .with_cancellation(self.cancellation.clone())
    }

    /// Clone an agent loop and install this invocation's run-owned tool scope.
    /// All planning, model-tool, nested-tool, and queue paths spawned from the
    /// clone inherit the same event sender and acknowledgement barrier.
    pub(crate) fn bind_agent_loop(&self, agent: &AgentLoop) -> AgentLoop {
        let mut scoped = agent.clone();
        scoped.config.permission_checker = self.governance.permission_checker.clone();
        scoped.config.confirmation_manager = self.governance.confirmation_manager.clone();
        scoped.tool_context = self.bind_tool_context(scoped.tool_context);
        scoped.bound_invocation = Some(self.clone());
        scoped
    }
}

impl AgentLoop {
    pub(crate) fn invocation_context(
        &self,
        run_id: impl Into<Arc<str>>,
        session_id: Option<&str>,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
        cancellation: CancellationToken,
    ) -> InvocationContext {
        let governance = InvocationGovernance {
            budget_guard: self.config.budget_guard.clone(),
            permission_checker: snapshot_permission_checker(
                self.config.permission_checker.as_ref(),
            ),
            confirmation_manager: snapshot_confirmation_manager(
                self.config.confirmation_manager.as_ref(),
            ),
        };
        let evidence = RunCapabilityEvidenceSource::from_agent_with_permission_checker(
            &self.config,
            Arc::clone(&self.tool_context.workspace_services),
            governance.permission_checker.as_ref(),
            governance.confirmation_manager.is_some(),
        );
        InvocationContext::new(
            run_id,
            Arc::<str>::from(session_id.unwrap_or("")),
            cancellation,
            event_tx,
            governance,
        )
        .with_model_evidence(evidence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::{PermissionChecker, PermissionDecision};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct MutablePermission {
        deny: Arc<AtomicBool>,
    }

    struct FrozenPermission {
        deny: bool,
    }

    impl PermissionChecker for MutablePermission {
        fn snapshot_for_run(&self) -> Option<Arc<dyn PermissionChecker>> {
            Some(Arc::new(FrozenPermission {
                deny: self.deny.load(Ordering::SeqCst),
            }))
        }

        fn check(&self, _tool_name: &str, _args: &serde_json::Value) -> PermissionDecision {
            if self.deny.load(Ordering::SeqCst) {
                PermissionDecision::Deny
            } else {
                PermissionDecision::Allow
            }
        }
    }

    impl PermissionChecker for FrozenPermission {
        fn check(&self, _tool_name: &str, _args: &serde_json::Value) -> PermissionDecision {
            if self.deny {
                PermissionDecision::Deny
            } else {
                PermissionDecision::Allow
            }
        }
    }

    struct OriginalGuard;

    #[async_trait::async_trait]
    impl BudgetGuard for OriginalGuard {
        async fn check_before_llm(&self, _: &str, _: usize) -> crate::budget::BudgetDecision {
            crate::budget::BudgetDecision::Allow
        }

        async fn record_after_llm(&self, _: &str, _: &crate::llm::TokenUsage) {}
    }

    #[test]
    fn retired_fact_host_lane_releases_sender_and_preserves_original_identity() {
        let (sender, mut receiver) = mpsc::channel(1);
        let token = CancellationToken::new();
        let guard: Arc<dyn BudgetGuard> = Arc::new(OriginalGuard);
        let context = InvocationContext::new(
            "original-run",
            "original-session",
            token.clone(),
            Some(sender.clone()),
            InvocationGovernance {
                budget_guard: Some(Arc::clone(&guard)),
                ..Default::default()
            },
        );
        let lineage = Arc::clone(&context.memory_context_sequence);
        let incarnation = context.memory_context_incarnation().to_owned();
        assert_eq!(context.next_memory_context_sequence(), Some(1));

        let retired = context.retire_fact_host_events().retire_fact_host_events();
        assert_eq!(retired.run_id(), "original-run");
        assert_eq!(retired.session_id(), "original-session");
        assert_eq!(retired.memory_context_incarnation(), incarnation);
        assert!(Arc::ptr_eq(&retired.memory_context_sequence, &lineage));
        assert!(Arc::ptr_eq(
            retired.governance.budget_guard.as_ref().unwrap(),
            &guard
        ));
        assert_eq!(retired.next_memory_context_sequence(), Some(2));
        assert!(retired.matches_parts(Some("original-session"), &Some(sender.clone())));
        assert!(retired.event_tx.is_none());
        assert!(!retired.cancellation.is_cancelled());
        token.cancel();
        assert!(retired.cancellation.is_cancelled());
        drop(sender);
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn retired_fact_host_lane_rejects_foreign_and_absent_channels() {
        let (sender, _) = mpsc::channel(1);
        let (foreign, _) = mpsc::channel(1);
        let retired = InvocationContext::new(
            "original-run",
            "original-session",
            CancellationToken::new(),
            Some(sender.clone()),
            InvocationGovernance::default(),
        )
        .retire_fact_host_events();
        assert!(retired.matches_parts(Some("original-session"), &Some(sender)));
        assert!(!retired.matches_parts(Some("original-session"), &Some(foreign)));
        assert!(!retired.matches_parts(Some("original-session"), &None));
        assert!(!retired.matches_parts(Some("foreign-session"), &None));
        let absent = InvocationContext::new(
            "absent-run",
            "original-session",
            CancellationToken::new(),
            None,
            InvocationGovernance::default(),
        )
        .retire_fact_host_events();
        assert!(absent.matches_parts(Some("original-session"), &None));
    }

    #[test]
    fn binding_installs_run_cancellation_and_session_identity() {
        let token = CancellationToken::new();
        let context = InvocationContext::new(
            Arc::<str>::from("run-1"),
            Arc::<str>::from("session-1"),
            token.clone(),
            None,
            InvocationGovernance::default(),
        );
        let tool_context = context.bind_tool_context(ToolContext::new(PathBuf::from(".")));

        assert_eq!(tool_context.session_id.as_deref(), Some("session-1"));
        assert_eq!(tool_context.run_id(), Some("run-1"));
        assert!(!tool_context.is_cancelled());
        token.cancel();
        assert!(tool_context.is_cancelled());
    }

    #[test]
    fn invocation_clones_share_one_monotonic_memory_context_sequence() {
        let context = InvocationContext::new(
            "run-1",
            "session-1",
            CancellationToken::new(),
            None,
            InvocationGovernance::default(),
        );
        let clone = context.clone();

        assert_eq!(
            context.memory_context_incarnation(),
            clone.memory_context_incarnation()
        );
        assert_eq!(context.next_memory_context_sequence(), Some(1));
        assert_eq!(clone.next_memory_context_sequence(), Some(2));
        assert_eq!(context.next_memory_context_sequence(), Some(3));
    }

    #[test]
    fn separate_invocations_never_share_a_memory_context_incarnation() {
        let first = InvocationContext::new(
            "run-1",
            "session-1",
            CancellationToken::new(),
            None,
            InvocationGovernance::default(),
        );
        let second = InvocationContext::new(
            "run-1",
            "session-1",
            CancellationToken::new(),
            None,
            InvocationGovernance::default(),
        );

        assert_ne!(
            first.memory_context_incarnation(),
            second.memory_context_incarnation()
        );
    }

    #[test]
    fn agent_invocation_freezes_permission_governance_once_per_run() {
        let workspace = tempfile::tempdir().unwrap();
        let deny = Arc::new(AtomicBool::new(false));
        let live = Arc::new(MutablePermission {
            deny: Arc::clone(&deny),
        });
        let executor = Arc::new(crate::tools::ToolExecutor::new(
            workspace.path().to_string_lossy().into_owned(),
        ));
        let agent = AgentLoop::new(
            Arc::new(crate::agent::tests::MockLlmClient::new(Vec::new())),
            executor,
            ToolContext::new(workspace.path().to_path_buf()),
            crate::agent::AgentConfig {
                permission_checker: Some(live.clone()),
                ..Default::default()
            },
        );
        let invocation = agent.invocation_context(
            "run-snapshot",
            Some("session"),
            None,
            CancellationToken::new(),
        );

        deny.store(true, Ordering::SeqCst);
        assert_eq!(
            live.check("write", &serde_json::json!({})),
            PermissionDecision::Deny
        );

        let scoped = invocation.bind_agent_loop(&agent);
        assert_eq!(
            scoped
                .config
                .permission_checker
                .as_ref()
                .unwrap()
                .check("write", &serde_json::json!({})),
            PermissionDecision::Allow
        );
        let tool_context =
            invocation.bind_tool_context(ToolContext::new(workspace.path().to_path_buf()));
        assert!(tool_context.has_run_governance());
        assert_eq!(
            tool_context
                .run_permission_checker()
                .unwrap()
                .check("write", &serde_json::json!({})),
            PermissionDecision::Allow
        );
    }
}
