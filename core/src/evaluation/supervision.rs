//! Host-policy boundary supervision for auxiliary evaluations.

use super::auxiliary_run::{
    AuxiliaryCapabilityProfileV1, AuxiliaryModeV1, AuxiliaryRunError, AuxiliaryRunHandle,
    AuxiliaryRunService, AuxiliaryRunSpecV1,
};
use super::dispatch_ledger::{
    EvaluationDispatchClaimOutcome, EvaluationDispatchLedger, EVALUATION_DISPATCH_LEASE_GRACE_MS,
    EVALUATION_DISPATCH_MIN_LEASE_MS,
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
use std::time::Duration;
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
    #[error("evaluation dispatch ledger failed: {0}")]
    DispatchLedger(String),
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
    /// Claims that have been admitted in the durable ledger and still need a
    /// terminal completion receipt. Values are request digests so a stale
    /// watcher can never remove a newer takeover claim.
    ledger_claims: HashMap<String, String>,
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
    dispatch_ledger: Option<Arc<dyn EvaluationDispatchLedger>>,
    owner_id: String,
}

impl std::fmt::Debug for EvaluationSupervisor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EvaluationSupervisor")
            .field("cancelled", &self.cancellation.is_cancelled())
            .field("durable_dispatch_ledger", &self.dispatch_ledger.is_some())
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
        Self::with_optional_dispatch_ledger(journal, reader, auxiliary, policy, None)
    }

    /// Construct a supervisor whose dispatch claims survive process restart.
    /// The ledger is still only a replay/lease mechanism; result persistence
    /// and host authorization remain separate concerns.
    pub fn with_dispatch_ledger(
        journal: Arc<dyn ExecutionFactJournal>,
        reader: Arc<dyn EvidenceReader>,
        auxiliary: Arc<dyn AuxiliaryRunService>,
        policy: Arc<dyn EvaluationPolicy>,
        dispatch_ledger: Arc<dyn EvaluationDispatchLedger>,
    ) -> Self {
        Self::with_optional_dispatch_ledger(
            journal,
            reader,
            auxiliary,
            policy,
            Some(dispatch_ledger),
        )
    }

    fn with_optional_dispatch_ledger(
        journal: Arc<dyn ExecutionFactJournal>,
        reader: Arc<dyn EvidenceReader>,
        auxiliary: Arc<dyn AuxiliaryRunService>,
        policy: Arc<dyn EvaluationPolicy>,
        dispatch_ledger: Option<Arc<dyn EvaluationDispatchLedger>>,
    ) -> Self {
        Self {
            journal,
            reader,
            auxiliary,
            policy,
            cancellation: CancellationToken::new(),
            state: Arc::new(Mutex::new(SupervisorState::default())),
            dispatch_ledger,
            owner_id: format!("evaluation-supervisor-{}", uuid::Uuid::new_v4()),
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
        let claims = {
            let mut state = lock_state(&self.state);
            let claims = state.ledger_claims.drain().collect::<Vec<_>>();
            state.in_flight.clear();
            state.last_dispatch_ms.clear();
            state.admitting.clear();
            state.dispatched.clear();
            claims
        };
        if let Some(ledger) = &self.dispatch_ledger {
            for (dispatch_id, request_digest) in claims {
                if let Err(error) = ledger
                    .release(&dispatch_id, &request_digest, &self.owner_id)
                    .await
                {
                    tracing::warn!(
                        dispatch_id = %dispatch_id,
                        error = %error,
                        "failed to release evaluation dispatch claim during shutdown"
                    );
                }
            }
        }
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
        self.journal.append(fact.clone())?;
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
        let dispatch_id = deterministic_auxiliary_id(&fact, &plan.purpose)?;
        let request_digest = dispatch_request_digest(&fact, &plan)?;
        let now = now_ms();
        {
            let mut state = lock_state(&self.state);
            if state.dispatched.contains(&dispatch_key) || state.admitting.contains(&dispatch_key) {
                // A journal replay and a first append can race before either
                // caller reaches this state lock. The dispatch key is the
                // supervisor's admission identity, so an already reserved or
                // committed key must never create a second evaluator.
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

        let mut ledger_claimed = false;
        if let Some(ledger) = &self.dispatch_ledger {
            let lease_ms = dispatch_lease_ms(plan.timeout_ms);
            let claim = ledger
                .claim(&dispatch_id, &request_digest, &self.owner_id, now, lease_ms)
                .await
                .map_err(|error| SupervisorError::DispatchLedger(error.to_string()))?;
            match claim {
                EvaluationDispatchClaimOutcome::Claimed { .. } => {
                    lock_state(&self.state)
                        .ledger_claims
                        .insert(dispatch_id.clone(), request_digest.clone());
                    ledger_claimed = true;
                }
                EvaluationDispatchClaimOutcome::Completed => {
                    reservation.release();
                    return Ok(EvaluationDispatch {
                        outcome: EvaluationDispatchOutcome::Ignored,
                        fact,
                        handle: None,
                    });
                }
                EvaluationDispatchClaimOutcome::Busy { .. } => {
                    reservation.release();
                    return Ok(EvaluationDispatch {
                        outcome: EvaluationDispatchOutcome::Suppressed,
                        fact,
                        handle: None,
                    });
                }
                EvaluationDispatchClaimOutcome::Conflict => {
                    reservation.release();
                    return Err(SupervisorError::DispatchLedger(
                        "dispatch identity conflicts with a different request".to_string(),
                    ));
                }
            }
        }

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
                if ledger_claimed {
                    self.release_dispatch_claim(&dispatch_id, &request_digest)
                        .await;
                }
                reservation.release();
                return Err(SupervisorError::Evidence(error.to_string()));
            }
        };
        if self.cancellation.is_cancelled() {
            if ledger_claimed {
                self.release_dispatch_claim(&dispatch_id, &request_digest)
                    .await;
            }
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
        spec.id = dispatch_id.clone();
        let handle = match self
            .auxiliary
            .spawn(spec, evidence, Some(self.cancellation.child_token()))
            .await
        {
            Ok(handle) => handle,
            Err(error) => {
                if ledger_claimed {
                    self.release_dispatch_claim(&dispatch_id, &request_digest)
                        .await;
                }
                reservation.release();
                return Err(error.into());
            }
        };
        reservation.commit();
        let state = Arc::clone(&self.state);
        let watcher = handle.clone();
        let ledger = self.dispatch_ledger.clone();
        let owner_id = self.owner_id.clone();
        let watcher_dispatch_id = dispatch_id.clone();
        let watcher_request_digest = request_digest.clone();
        let lease_ms = dispatch_lease_ms(plan.timeout_ms);
        tokio::spawn(async move {
            let mut claim_owned = ledger.is_some();
            if let Some(ledger) = &ledger {
                let heartbeat_ms = (lease_ms / 3).max(1);
                let mut heartbeat = tokio::time::interval(Duration::from_millis(heartbeat_ms));
                // `interval` ticks immediately once; the initial claim already
                // has a full lease, so the first renewal can wait one period.
                heartbeat.tick().await;
                loop {
                    tokio::select! {
                        _ = watcher.wait() => break,
                        _ = heartbeat.tick() => {
                            match ledger
                                .renew(
                                    &watcher_dispatch_id,
                                    &watcher_request_digest,
                                    &owner_id,
                                    now_ms(),
                                    lease_ms,
                                )
                                .await
                            {
                                Ok(true) => {}
                                Ok(false) => {
                                    // Shutdown or a newer supervisor has
                                    // fenced this watcher out. It may still
                                    // settle its local handle, but must not
                                    // publish a stale completion receipt.
                                    claim_owned = false;
                                }
                                Err(error) => {
                                    tracing::warn!(
                                        dispatch_id = %watcher_dispatch_id,
                                        error = %error,
                                        "evaluation dispatch lease renewal failed"
                                    );
                                }
                            }
                        }
                    }
                    if !claim_owned {
                        let _ = watcher.wait().await;
                        break;
                    }
                }
                if claim_owned {
                    let _ = ledger
                        .complete(
                            &watcher_dispatch_id,
                            &watcher_request_digest,
                            &owner_id,
                            now_ms(),
                        )
                        .await;
                }
            } else {
                let _ = watcher.wait().await;
            }
            let mut state = lock_state(&state);
            if let Some(pending) = state.in_flight.get_mut(&key) {
                *pending = pending.saturating_sub(1);
                if *pending == 0 {
                    state.in_flight.remove(&key);
                }
            }
            if state
                .ledger_claims
                .get(&watcher_dispatch_id)
                .is_some_and(|digest| digest == &watcher_request_digest)
            {
                state.ledger_claims.remove(&watcher_dispatch_id);
            }
        });
        Ok(EvaluationDispatch {
            outcome: EvaluationDispatchOutcome::Dispatched,
            fact,
            handle: Some(handle),
        })
    }

    async fn release_dispatch_claim(&self, dispatch_id: &str, request_digest: &str) {
        if let Some(ledger) = &self.dispatch_ledger {
            if let Err(error) = ledger
                .release(dispatch_id, request_digest, &self.owner_id)
                .await
            {
                tracing::warn!(
                    dispatch_id = %dispatch_id,
                    error = %error,
                    "failed to release evaluation dispatch claim"
                );
            }
        }
        let mut state = lock_state(&self.state);
        if state
            .ledger_claims
            .get(dispatch_id)
            .is_some_and(|digest| digest == request_digest)
        {
            state.ledger_claims.remove(dispatch_id);
        }
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

fn dispatch_request_digest(
    fact: &ExecutionFactV1,
    plan: &EvaluationPlanV1,
) -> Result<String, SupervisorError> {
    let plan_digest = digest_json("a3s.code.evaluation-plan.identity.v1", plan)
        .map_err(|error| SupervisorError::Evidence(error.to_string()))?;
    digest_json(
        "a3s.code.evaluation-dispatch.request.v1",
        &serde_json::json!({
            "fact_digest": &fact.fact_digest,
            "purpose": &plan.purpose,
            "plan_digest": plan_digest,
        }),
    )
    .map_err(|error| SupervisorError::Evidence(error.to_string()))
}

fn dispatch_lease_ms(timeout_ms: Option<u64>) -> u64 {
    timeout_ms
        .unwrap_or(EVALUATION_DISPATCH_MIN_LEASE_MS)
        .saturating_add(EVALUATION_DISPATCH_LEASE_GRACE_MS)
        .max(EVALUATION_DISPATCH_MIN_LEASE_MS)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "supervision_tests.rs"]
mod tests;
