//! Host-policy boundary supervision for auxiliary evaluations.

use super::auxiliary_run::{
    AuxiliaryCapabilityProfileV1, AuxiliaryModeV1, AuxiliaryRunError, AuxiliaryRunHandle,
    AuxiliaryRunService, AuxiliaryRunSpecV1,
};
use super::evidence::{
    EvidenceContentModeV1, EvidenceLimitsV1, EvidenceReadRequestV1, EvidenceReader,
};
use super::identity::{digest_json, ExecutionFrameV1, ExecutionTargetV1};
use super::journal::{ExecutionFactJournal, ExecutionFactV1, JournalError};
use crate::run::RunEventRecord;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio_util::sync::CancellationToken;

pub const EVALUATION_PLAN_SCHEMA_V1: &str = "a3s.code.evaluation-plan.v1";
pub const EVALUATION_MAX_PENDING: usize = 1024;
pub const EVALUATION_MAX_COOLDOWN_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationBoundaryV1 {
    EveryEvent,
    TurnEnd,
    RunTerminal,
}

impl EvaluationBoundaryV1 {
    pub fn matches(self, fact: &ExecutionFactV1) -> bool {
        match self {
            Self::EveryEvent => true,
            Self::TurnEnd => fact.event_type == "turn_end",
            Self::RunTerminal => matches!(
                fact.event_type.as_str(),
                "agent_end" | "error" | "run_control_applied" | "persistence_failed"
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationPlanV1 {
    pub schema: String,
    pub boundary: EvaluationBoundaryV1,
    pub purpose: String,
    pub instruction: String,
    pub mode: AuxiliaryModeV1,
    pub capabilities: AuxiliaryCapabilityProfileV1,
    pub parent_ceiling: Option<AuxiliaryCapabilityProfileV1>,
    pub limits: EvidenceLimitsV1,
    pub content_mode: EvidenceContentModeV1,
    pub include_prompt: bool,
    pub include_terminal_text: bool,
    pub include_artifact_content: bool,
    pub max_pending: usize,
    pub cooldown_ms: u64,
    pub max_steps: u32,
    pub timeout_ms: Option<u64>,
    pub output_schema: Option<serde_json::Value>,
}

impl EvaluationPlanV1 {
    pub fn new(
        boundary: EvaluationBoundaryV1,
        purpose: impl Into<String>,
        instruction: impl Into<String>,
    ) -> Self {
        Self {
            schema: EVALUATION_PLAN_SCHEMA_V1.to_string(),
            boundary,
            purpose: purpose.into(),
            instruction: instruction.into(),
            mode: AuxiliaryModeV1::Detached,
            capabilities: AuxiliaryCapabilityProfileV1::tool_free(),
            parent_ceiling: None,
            limits: EvidenceLimitsV1::default(),
            content_mode: EvidenceContentModeV1::DigestOnly,
            include_prompt: false,
            include_terminal_text: false,
            include_artifact_content: false,
            max_pending: 1,
            cooldown_ms: 0,
            max_steps: 1,
            timeout_ms: None,
            output_schema: None,
        }
    }

    pub fn with_cooldown_ms(mut self, cooldown_ms: u64) -> Self {
        self.cooldown_ms = cooldown_ms;
        self
    }

    pub fn validate(&self) -> Result<(), SupervisorError> {
        if self.schema != EVALUATION_PLAN_SCHEMA_V1 {
            return Err(SupervisorError::InvalidPlan("schema"));
        }
        if self.purpose.is_empty() || self.purpose.len() > 256 || self.purpose.contains('\0') {
            return Err(SupervisorError::InvalidPlan("purpose"));
        }
        if self.instruction.is_empty() || self.instruction.len() > 128 * 1024 {
            return Err(SupervisorError::InvalidPlan("instruction"));
        }
        if self.max_pending == 0
            || self.max_pending > EVALUATION_MAX_PENDING
            || self.max_steps == 0
            || self.max_steps > super::auxiliary_run::AUXILIARY_MAX_STEPS
        {
            return Err(SupervisorError::InvalidPlan("limits"));
        }
        if self.cooldown_ms > EVALUATION_MAX_COOLDOWN_MS {
            return Err(SupervisorError::InvalidPlan("cooldown_ms"));
        }
        self.limits
            .validate()
            .map_err(|_| SupervisorError::InvalidPlan("evidence_limits"))?;
        self.capabilities
            .validate()
            .map_err(|_| SupervisorError::InvalidPlan("capabilities"))?;
        if let Some(ceiling) = self.parent_ceiling {
            ceiling
                .validate()
                .map_err(|_| SupervisorError::InvalidPlan("parent_ceiling"))?;
            if !self.capabilities.is_within(ceiling) {
                return Err(SupervisorError::CapabilityEscalation);
            }
        }
        if self
            .timeout_ms
            .is_some_and(|timeout| timeout == 0 || timeout > 24 * 60 * 60 * 1000)
        {
            return Err(SupervisorError::InvalidPlan("timeout_ms"));
        }
        if let Some(schema) = &self.output_schema {
            let encoded = serde_json::to_vec(schema)
                .map_err(|_| SupervisorError::InvalidPlan("output_schema"))?;
            if encoded.len() > 128 * 1024
                || jsonschema::draft202012::options().build(schema).is_err()
            {
                return Err(SupervisorError::InvalidPlan("output_schema"));
            }
        }
        Ok(())
    }
}

pub trait EvaluationPolicy: Send + Sync {
    fn plan(&self, fact: &ExecutionFactV1) -> Option<EvaluationPlanV1>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvaluationDispatchOutcome {
    Ignored,
    Suppressed,
    Dispatched,
}

#[derive(Debug, Clone)]
pub struct EvaluationDispatch {
    pub outcome: EvaluationDispatchOutcome,
    pub fact: ExecutionFactV1,
    pub handle: Option<AuxiliaryRunHandle>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SupervisorError {
    #[error("evaluation plan field `{0}` is invalid")]
    InvalidPlan(&'static str),
    #[error("evaluation plan would exceed its parent capability ceiling")]
    CapabilityEscalation,
    #[error("execution fact error: {0}")]
    Journal(#[from] JournalError),
    #[error("evidence read failed: {0}")]
    Evidence(String),
    #[error("auxiliary run failed to dispatch: {0}")]
    Auxiliary(#[from] AuxiliaryRunError),
}

#[derive(Default)]
struct SupervisorState {
    in_flight: HashMap<(ExecutionTargetV1, String), usize>,
    last_dispatch_ms: HashMap<(ExecutionTargetV1, String), u64>,
    /// Dispatch identities are retained for the lifetime of this supervisor.
    /// This prevents concurrent/replayed observations from creating a second
    /// auxiliary run after the first admission has succeeded. Hosts should
    /// scope a supervisor to the corresponding Session/Run lifetime.
    dispatched: HashSet<(ExecutionTargetV1, u64, String)>,
    /// Reservations made while evidence is being read or an auxiliary
    /// service is admitting a run. This is separate from `dispatched` so a
    /// cancelled/failed admission can be retried without creating a replay
    /// hole.
    admitting: HashSet<(ExecutionTargetV1, u64, String)>,
}

struct DispatchReservation {
    state: Arc<Mutex<SupervisorState>>,
    pending_key: (ExecutionTargetV1, String),
    dispatch_key: (ExecutionTargetV1, u64, String),
    dispatch_at_ms: u64,
    finished: bool,
}

impl DispatchReservation {
    fn commit(&mut self) {
        let mut state = lock_state(&self.state);
        state.admitting.remove(&self.dispatch_key);
        state.dispatched.insert(self.dispatch_key.clone());
        // A cooldown is charged only after evidence and auxiliary admission
        // succeed. Failed/cancelled admission can therefore retry the exact
        // fact immediately, while later facts still observe the cooldown.
        state
            .last_dispatch_ms
            .insert(self.pending_key.clone(), self.dispatch_at_ms);
        self.finished = true;
    }

    fn release(&mut self) {
        release_state(&self.state, &self.pending_key, &self.dispatch_key);
        self.finished = true;
    }
}

impl Drop for DispatchReservation {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        // Admission state is protected by a short, synchronous lock and does
        // not perform I/O. Releasing synchronously makes cancellation cleanup
        // deterministic: an immediately replayed observation cannot race a
        // deferred task that has not yet removed the reservation.
        release_state(&self.state, &self.pending_key, &self.dispatch_key);
    }
}

fn lock_state<'a>(state: &'a Arc<Mutex<SupervisorState>>) -> MutexGuard<'a, SupervisorState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn release_state(
    state: &Arc<Mutex<SupervisorState>>,
    pending_key: &(ExecutionTargetV1, String),
    dispatch_key: &(ExecutionTargetV1, u64, String),
) {
    let mut state = lock_state(state);
    if let Some(pending) = state.in_flight.get_mut(pending_key) {
        *pending = pending.saturating_sub(1);
        if *pending == 0 {
            state.in_flight.remove(pending_key);
        }
    }
    state.admitting.remove(dispatch_key);
    state.dispatched.remove(dispatch_key);
}

/// Connects an append-only fact journal to a host policy, evidence reader, and
/// auxiliary-run service.  The policy is the only component that decides when
/// an evaluation is useful; Core only enforces identity, bounds, and cleanup.
pub struct EvaluationSupervisor {
    journal: Arc<dyn ExecutionFactJournal>,
    reader: Arc<dyn EvidenceReader>,
    auxiliary: Arc<dyn AuxiliaryRunService>,
    policy: Arc<dyn EvaluationPolicy>,
    cancellation: CancellationToken,
    state: Arc<Mutex<SupervisorState>>,
}

impl std::fmt::Debug for EvaluationSupervisor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EvaluationSupervisor")
            .field("cancelled", &self.cancellation.is_cancelled())
            .finish()
    }
}

impl EvaluationSupervisor {
    pub fn new(
        journal: Arc<dyn ExecutionFactJournal>,
        reader: Arc<dyn EvidenceReader>,
        auxiliary: Arc<dyn AuxiliaryRunService>,
        policy: Arc<dyn EvaluationPolicy>,
    ) -> Self {
        Self {
            journal,
            reader,
            auxiliary,
            policy,
            cancellation: CancellationToken::new(),
            state: Arc::new(Mutex::new(SupervisorState::default())),
        }
    }

    pub fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    /// Cancel all admitted auxiliary work and release this supervisor's
    /// in-memory admission history. Auxiliary services still own their
    /// terminal snapshots; the parent cancellation token makes cooperative
    /// executors settle without blocking the caller.
    pub async fn shutdown(&self) {
        self.cancel();
        let mut state = lock_state(&self.state);
        state.in_flight.clear();
        state.last_dispatch_ms.clear();
        state.admitting.clear();
        state.dispatched.clear();
    }

    /// Return the number of auxiliary runs currently admitted but not yet
    /// terminal. This is an observation only and never grants admission.
    pub async fn pending_count(&self) -> usize {
        lock_state(&self.state).in_flight.values().sum()
    }

    /// Record one runtime event and, if the host policy selects the matching
    /// boundary, dispatch one isolated auxiliary run.  The returned handle is
    /// advisory: the parent execution is never implicitly blocked by the
    /// auxiliary result.
    pub async fn observe_event(
        &self,
        frame: ExecutionFrameV1,
        record: &RunEventRecord,
    ) -> Result<EvaluationDispatch, SupervisorError> {
        let fact = super::journal::ExecutionFactV1::from_run_event(frame, record)?;
        let append = self.journal.append(fact.clone())?;
        let Some(plan) = self.policy.plan(&fact) else {
            return Ok(EvaluationDispatch {
                outcome: EvaluationDispatchOutcome::Ignored,
                fact,
                handle: None,
            });
        };
        plan.validate()?;
        if !plan.boundary.matches(&fact) {
            return Ok(EvaluationDispatch {
                outcome: EvaluationDispatchOutcome::Ignored,
                fact,
                handle: None,
            });
        }
        let key = (fact.frame.target.clone(), plan.purpose.clone());
        let dispatch_key = (
            fact.frame.target.clone(),
            fact.sequence,
            plan.purpose.clone(),
        );
        let now = now_ms();
        {
            let mut state = lock_state(&self.state);
            if !append.appended
                && append.replayed
                && (state.dispatched.contains(&dispatch_key)
                    || state.admitting.contains(&dispatch_key))
            {
                // An exact replay of an already admitted fact is safe, but it
                // must never create another evaluator.
                return Ok(EvaluationDispatch {
                    outcome: EvaluationDispatchOutcome::Ignored,
                    fact,
                    handle: None,
                });
            }
            let pending = state.in_flight.get(&key).copied().unwrap_or(0);
            let last = state.last_dispatch_ms.get(&key).copied();
            if pending >= plan.max_pending
                || last.is_some_and(|last| now.saturating_sub(last) < plan.cooldown_ms)
                || self.cancellation.is_cancelled()
            {
                return Ok(EvaluationDispatch {
                    outcome: EvaluationDispatchOutcome::Suppressed,
                    fact,
                    handle: None,
                });
            }
            // Reserve the slot before reading evidence or spawning the
            // executor. Two events can arrive concurrently; reserving here
            // keeps max_pending a real admission bound rather than a
            // best-effort post-dispatch counter.
            *state.in_flight.entry(key.clone()).or_default() += 1;
            // Reserve before the async evidence read so two concurrent
            // observations of the same fact cannot both pass admission.
            state.admitting.insert(dispatch_key.clone());
        }
        let mut reservation = DispatchReservation {
            state: Arc::clone(&self.state),
            pending_key: key.clone(),
            dispatch_key: dispatch_key.clone(),
            dispatch_at_ms: now,
            finished: false,
        };

        let request = EvidenceReadRequestV1 {
            target: fact.frame.target.clone(),
            after_sequence: None,
            limits: plan.limits,
            content_mode: plan.content_mode,
            include_prompt: plan.include_prompt,
            include_terminal_text: plan.include_terminal_text,
            include_artifact_content: plan.include_artifact_content,
        };
        let evidence = match self.reader.read(request).await {
            Ok(evidence) => evidence,
            Err(error) => {
                reservation.release();
                return Err(SupervisorError::Evidence(error.to_string()));
            }
        };
        if self.cancellation.is_cancelled() {
            reservation.release();
            return Ok(EvaluationDispatch {
                outcome: EvaluationDispatchOutcome::Suppressed,
                fact,
                handle: None,
            });
        }
        let mut spec = AuxiliaryRunSpecV1::new(
            fact.frame.clone(),
            plan.purpose.clone(),
            plan.instruction,
            evidence.snapshot_digest.clone(),
        )
        .with_mode(plan.mode)
        .with_capabilities(plan.capabilities);
        if let Some(ceiling) = plan.parent_ceiling {
            spec = spec.with_parent_ceiling(ceiling);
        }
        spec.max_steps = plan.max_steps;
        spec.timeout_ms = plan.timeout_ms;
        spec.output_schema = plan.output_schema;
        let id = deterministic_auxiliary_id(&fact, &plan.purpose)?;
        spec.id = id;
        let handle = match self
            .auxiliary
            .spawn(spec, evidence, Some(self.cancellation.child_token()))
            .await
        {
            Ok(handle) => handle,
            Err(error) => {
                reservation.release();
                return Err(error.into());
            }
        };
        reservation.commit();
        let state = Arc::clone(&self.state);
        let watcher = handle.clone();
        tokio::spawn(async move {
            let _ = watcher.wait().await;
            let mut state = lock_state(&state);
            if let Some(pending) = state.in_flight.get_mut(&key) {
                *pending = pending.saturating_sub(1);
                if *pending == 0 {
                    state.in_flight.remove(&key);
                }
            }
        });
        Ok(EvaluationDispatch {
            outcome: EvaluationDispatchOutcome::Dispatched,
            fact,
            handle: Some(handle),
        })
    }
}

fn deterministic_auxiliary_id(
    fact: &ExecutionFactV1,
    purpose: &str,
) -> Result<String, SupervisorError> {
    let identity = serde_json::json!({
        "target": fact.frame.target.clone(),
        "sequence": fact.sequence,
        "purpose": purpose,
        "fact_digest": fact.fact_digest.clone(),
    });
    let digest = digest_json("a3s.code.evaluation-dispatch.v1", &identity)
        .map_err(|error| SupervisorError::Evidence(error.to_string()))?;
    Ok(format!("aux-{digest}"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentEvent;
    use crate::evaluation::auxiliary_run::{
        AuxiliaryExecutor, AuxiliaryRunContextV1, InMemoryAuxiliaryRunService,
    };
    use crate::evaluation::evidence::RunEvidenceReader;
    use crate::evaluation::evidence::{EvidenceError, EvidenceReadRequestV1};
    use crate::evaluation::identity::ExecutionTargetV1;
    use crate::evaluation::journal::InMemoryExecutionFactJournal;
    use crate::run::InMemoryRunStore;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    struct TurnPolicy;

    impl EvaluationPolicy for TurnPolicy {
        fn plan(&self, _fact: &ExecutionFactV1) -> Option<EvaluationPlanV1> {
            Some(EvaluationPlanV1::new(
                EvaluationBoundaryV1::TurnEnd,
                "turn-check",
                "inspect the bounded evidence",
            ))
        }
    }

    struct RecordingExecutor;

    #[async_trait]
    impl AuxiliaryExecutor for RecordingExecutor {
        async fn execute(
            &self,
            context: AuxiliaryRunContextV1,
        ) -> Result<serde_json::Value, AuxiliaryRunError> {
            Ok(serde_json::json!({
                "sequence": context.evidence.events.first().map(|event| event.sequence)
            }))
        }
    }

    #[tokio::test]
    async fn policy_is_boundary_and_replay_safe() {
        let runs = Arc::new(InMemoryRunStore::new());
        let run = runs
            .create_run_with_id("run-1".into(), "session-1", "prompt")
            .await;
        let target = ExecutionTargetV1::new("session-1", &run.id);
        let journal = Arc::new(InMemoryExecutionFactJournal::new());
        let service = Arc::new(InMemoryAuxiliaryRunService::new(Arc::new(
            RecordingExecutor,
        )));
        let supervisor = EvaluationSupervisor::new(
            journal,
            Arc::new(RunEvidenceReader::new(Arc::clone(&runs))),
            service,
            Arc::new(TurnPolicy),
        );
        let start = RunEventRecord {
            sequence: 0,
            timestamp_ms: 1,
            event: AgentEvent::TurnStart { turn: 1 },
        };
        let ignored = supervisor
            .observe_event(ExecutionFrameV1::root(target.clone()), &start)
            .await
            .unwrap();
        assert_eq!(ignored.outcome, EvaluationDispatchOutcome::Ignored);
        let end = RunEventRecord {
            sequence: 1,
            timestamp_ms: 2,
            event: AgentEvent::TurnEnd {
                turn: 1,
                usage: crate::llm::TokenUsage::default(),
            },
        };
        let dispatched = supervisor
            .observe_event(ExecutionFrameV1::root(target.clone()), &end)
            .await
            .unwrap();
        assert_eq!(dispatched.outcome, EvaluationDispatchOutcome::Dispatched);
        dispatched.handle.unwrap().wait().await.unwrap();
        let replay = supervisor
            .observe_event(ExecutionFrameV1::root(target), &end)
            .await
            .unwrap();
        assert_eq!(replay.outcome, EvaluationDispatchOutcome::Ignored);
    }

    #[tokio::test]
    async fn pending_cap_suppresses_without_blocking_parent() {
        struct SlowExecutor;
        #[async_trait]
        impl AuxiliaryExecutor for SlowExecutor {
            async fn execute(
                &self,
                context: AuxiliaryRunContextV1,
            ) -> Result<serde_json::Value, AuxiliaryRunError> {
                context.cancellation.cancelled().await;
                Err(AuxiliaryRunError::Cancelled)
            }
        }
        struct EveryEvent;
        impl EvaluationPolicy for EveryEvent {
            fn plan(&self, _fact: &ExecutionFactV1) -> Option<EvaluationPlanV1> {
                Some(EvaluationPlanV1::new(
                    EvaluationBoundaryV1::EveryEvent,
                    "one-at-a-time",
                    "wait",
                ))
            }
        }
        let runs = Arc::new(InMemoryRunStore::new());
        let run = runs
            .create_run_with_id("run-2".into(), "session-2", "prompt")
            .await;
        let target = ExecutionTargetV1::new("session-2", &run.id);
        let journal = Arc::new(InMemoryExecutionFactJournal::new());
        let service = Arc::new(InMemoryAuxiliaryRunService::new(Arc::new(SlowExecutor)));
        let supervisor = EvaluationSupervisor::new(
            journal,
            Arc::new(RunEvidenceReader::new(runs)),
            service,
            Arc::new(EveryEvent),
        );
        let event = |sequence| RunEventRecord {
            sequence,
            timestamp_ms: (sequence + 1) as u64,
            event: AgentEvent::TextDelta {
                text: format!("event-{sequence}"),
            },
        };
        let first = supervisor
            .observe_event(ExecutionFrameV1::root(target.clone()), &event(0))
            .await
            .unwrap();
        assert_eq!(first.outcome, EvaluationDispatchOutcome::Dispatched);
        let second = supervisor
            .observe_event(ExecutionFrameV1::root(target), &event(1))
            .await
            .unwrap();
        assert_eq!(second.outcome, EvaluationDispatchOutcome::Suppressed);
        assert_eq!(supervisor.pending_count().await, 1);
        first.handle.unwrap().cancel().await;
        supervisor.shutdown().await;
        assert_eq!(supervisor.pending_count().await, 0);
    }

    #[tokio::test]
    async fn failed_evidence_admission_can_retry_an_exact_replay() {
        struct FlakyReader {
            calls: AtomicUsize,
            inner: RunEvidenceReader,
        }

        #[async_trait]
        impl EvidenceReader for FlakyReader {
            async fn read(
                &self,
                request: EvidenceReadRequestV1,
            ) -> Result<super::super::evidence::EvidenceSnapshotV1, EvidenceError> {
                if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err(EvidenceError::RunNotFound);
                }
                self.inner.read(request).await
            }
        }

        struct EveryEvent;
        impl EvaluationPolicy for EveryEvent {
            fn plan(&self, _fact: &ExecutionFactV1) -> Option<EvaluationPlanV1> {
                Some(
                    EvaluationPlanV1::new(EvaluationBoundaryV1::EveryEvent, "retryable", "inspect")
                        .with_cooldown_ms(60_000),
                )
            }
        }

        let runs = Arc::new(InMemoryRunStore::new());
        let run = runs
            .create_run_with_id("retry-run".into(), "retry-session", "prompt")
            .await;
        let target = ExecutionTargetV1::new("retry-session", &run.id);
        let record = RunEventRecord {
            sequence: 0,
            timestamp_ms: 1,
            event: AgentEvent::TextDelta {
                text: "event".into(),
            },
        };
        runs.record_event(&run.id, record.event.clone()).await;
        let supervisor = EvaluationSupervisor::new(
            Arc::new(InMemoryExecutionFactJournal::new()),
            Arc::new(FlakyReader {
                calls: AtomicUsize::new(0),
                inner: RunEvidenceReader::new(Arc::clone(&runs)),
            }),
            Arc::new(InMemoryAuxiliaryRunService::new(Arc::new(
                RecordingExecutor,
            ))),
            Arc::new(EveryEvent),
        );
        assert!(matches!(
            supervisor
                .observe_event(ExecutionFrameV1::root(target.clone()), &record)
                .await,
            Err(SupervisorError::Evidence(_))
        ));
        let retry = supervisor
            .observe_event(ExecutionFrameV1::root(target), &record)
            .await
            .unwrap();
        assert_eq!(retry.outcome, EvaluationDispatchOutcome::Dispatched);
        retry.handle.unwrap().wait().await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_evidence_admission_releases_reservation_synchronously() {
        struct BlockingReader {
            calls: AtomicUsize,
            started: Notify,
            release: Notify,
            inner: RunEvidenceReader,
        }

        #[async_trait]
        impl EvidenceReader for BlockingReader {
            async fn read(
                &self,
                request: EvidenceReadRequestV1,
            ) -> Result<super::super::evidence::EvidenceSnapshotV1, EvidenceError> {
                if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    self.started.notify_one();
                    self.release.notified().await;
                }
                self.inner.read(request).await
            }
        }

        struct EveryEvent;
        impl EvaluationPolicy for EveryEvent {
            fn plan(&self, _fact: &ExecutionFactV1) -> Option<EvaluationPlanV1> {
                Some(EvaluationPlanV1::new(
                    EvaluationBoundaryV1::EveryEvent,
                    "cancel-retry",
                    "inspect",
                ))
            }
        }

        let runs = Arc::new(InMemoryRunStore::new());
        let run = runs
            .create_run_with_id("cancel-run".into(), "cancel-session", "prompt")
            .await;
        let record = RunEventRecord {
            sequence: 0,
            timestamp_ms: 1,
            event: AgentEvent::TextDelta {
                text: "event".into(),
            },
        };
        runs.record_event(&run.id, record.event.clone()).await;
        let target = ExecutionTargetV1::new("cancel-session", &run.id);
        let reader = Arc::new(BlockingReader {
            calls: AtomicUsize::new(0),
            started: Notify::new(),
            release: Notify::new(),
            inner: RunEvidenceReader::new(Arc::clone(&runs)),
        });
        let supervisor = Arc::new(EvaluationSupervisor::new(
            Arc::new(InMemoryExecutionFactJournal::new()),
            reader.clone(),
            Arc::new(InMemoryAuxiliaryRunService::new(Arc::new(
                RecordingExecutor,
            ))),
            Arc::new(EveryEvent),
        ));
        let task_supervisor = Arc::clone(&supervisor);
        let task_target = target.clone();
        let task_record = record.clone();
        let task = tokio::spawn(async move {
            task_supervisor
                .observe_event(ExecutionFrameV1::root(task_target), &task_record)
                .await
        });
        reader.started.notified().await;
        task.abort();
        assert!(task.await.is_err());
        assert_eq!(supervisor.pending_count().await, 0);

        let retry = supervisor
            .observe_event(ExecutionFrameV1::root(target), &record)
            .await
            .unwrap();
        assert_eq!(retry.outcome, EvaluationDispatchOutcome::Dispatched);
        retry.handle.unwrap().wait().await.unwrap();
    }
}
