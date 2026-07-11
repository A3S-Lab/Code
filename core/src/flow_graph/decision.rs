use super::decision_ledger::{
    FlowDecisionClaimOutcome, FlowDecisionLedger, MemoryFlowDecisionLedger,
};
use a3s_flow::RetryPolicy;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FlowDecision {
    ScheduleStep { step: FlowDecisionStep },
    ScheduleSteps { steps: Vec<FlowDecisionStep> },
    Complete { output: Value },
    Fail { error: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FlowDecisionStep {
    pub step_id: String,
    pub step_name: String,
    pub input: Value,
    #[serde(default)]
    pub retry: RetryPolicy,
}

/// A graph proposal submitted through a host-owned Flow boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FlowDecisionRequest {
    pub decision_id: String,
    pub run_id: String,
    pub authority_branch_id: String,
    pub causation_event_id: String,
    pub decision: FlowDecision,
}

#[async_trait]
pub trait FlowDecisionSink: Send + Sync {
    /// Submit using `request.decision_id` as the downstream idempotency key.
    /// Implementations must deduplicate that key because an expired lease can
    /// be reclaimed after a process crashes between submission and receipt.
    async fn submit(
        &self,
        request: &FlowDecisionRequest,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FlowDecisionHealthStatus {
    Healthy,
    Degraded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FlowDecisionHealthSnapshot {
    pub status: FlowDecisionHealthStatus,
    pub attempted: u64,
    pub claimed: u64,
    pub takeovers: u64,
    pub completed: u64,
    pub duplicates: u64,
    pub busy: u64,
    pub conflicts: u64,
    pub rejected: u64,
    pub lease_renewals: u64,
    pub lease_lost: u64,
    pub sink_failures: u64,
    pub ledger_failures: u64,
    pub failures: u64,
    pub cancellations: u64,
    pub in_flight: u64,
    pub average_dispatch_micros: u64,
    pub max_dispatch_micros: u64,
    pub last_success_at_ms: Option<u64>,
    pub last_failure_at_ms: Option<u64>,
}

#[derive(Default)]
struct FlowDecisionMetrics {
    attempted: AtomicU64,
    claimed: AtomicU64,
    takeovers: AtomicU64,
    completed: AtomicU64,
    duplicates: AtomicU64,
    busy: AtomicU64,
    conflicts: AtomicU64,
    rejected: AtomicU64,
    lease_renewals: AtomicU64,
    lease_lost: AtomicU64,
    sink_failures: AtomicU64,
    ledger_failures: AtomicU64,
    failures: AtomicU64,
    cancellations: AtomicU64,
    in_flight: AtomicU64,
    total_dispatch_micros: AtomicU64,
    max_dispatch_micros: AtomicU64,
    last_success_at_ms: AtomicU64,
    last_failure_at_ms: AtomicU64,
    degraded: AtomicBool,
}

/// Enforces production-branch authority and idempotent successful submission.
pub struct FlowDecisionDispatcher {
    production_branch_id: String,
    sink: Arc<dyn FlowDecisionSink>,
    ledger: Arc<dyn FlowDecisionLedger>,
    owner_id: String,
    lease_ms: u64,
    dispatch_lock: Mutex<()>,
    metrics: Arc<FlowDecisionMetrics>,
}

impl FlowDecisionDispatcher {
    pub fn new(production_branch_id: impl Into<String>, sink: Arc<dyn FlowDecisionSink>) -> Self {
        Self::with_ledger(
            production_branch_id,
            sink,
            Arc::new(MemoryFlowDecisionLedger::new()),
        )
    }

    pub fn with_ledger(
        production_branch_id: impl Into<String>,
        sink: Arc<dyn FlowDecisionSink>,
        ledger: Arc<dyn FlowDecisionLedger>,
    ) -> Self {
        Self {
            production_branch_id: production_branch_id.into(),
            sink,
            ledger,
            owner_id: format!("decision-dispatcher-{}", uuid::Uuid::new_v4()),
            lease_ms: 30_000,
            dispatch_lock: Mutex::new(()),
            metrics: Arc::new(FlowDecisionMetrics::default()),
        }
    }

    pub fn with_lease_ms(mut self, lease_ms: u64) -> Self {
        self.lease_ms = lease_ms.max(1);
        self
    }

    pub fn health(&self) -> FlowDecisionHealthSnapshot {
        let attempted = self.metrics.attempted.load(Ordering::Relaxed);
        let total_micros = self.metrics.total_dispatch_micros.load(Ordering::Relaxed);
        let last_success_at_ms = nonzero(self.metrics.last_success_at_ms.load(Ordering::Relaxed));
        let last_failure_at_ms = nonzero(self.metrics.last_failure_at_ms.load(Ordering::Relaxed));
        FlowDecisionHealthSnapshot {
            status: if self.metrics.degraded.load(Ordering::Relaxed) {
                FlowDecisionHealthStatus::Degraded
            } else {
                FlowDecisionHealthStatus::Healthy
            },
            attempted,
            claimed: self.metrics.claimed.load(Ordering::Relaxed),
            takeovers: self.metrics.takeovers.load(Ordering::Relaxed),
            completed: self.metrics.completed.load(Ordering::Relaxed),
            duplicates: self.metrics.duplicates.load(Ordering::Relaxed),
            busy: self.metrics.busy.load(Ordering::Relaxed),
            conflicts: self.metrics.conflicts.load(Ordering::Relaxed),
            rejected: self.metrics.rejected.load(Ordering::Relaxed),
            lease_renewals: self.metrics.lease_renewals.load(Ordering::Relaxed),
            lease_lost: self.metrics.lease_lost.load(Ordering::Relaxed),
            sink_failures: self.metrics.sink_failures.load(Ordering::Relaxed),
            ledger_failures: self.metrics.ledger_failures.load(Ordering::Relaxed),
            failures: self.metrics.failures.load(Ordering::Relaxed),
            cancellations: self.metrics.cancellations.load(Ordering::Relaxed),
            in_flight: self.metrics.in_flight.load(Ordering::Relaxed),
            average_dispatch_micros: total_micros.checked_div(attempted).unwrap_or(0),
            max_dispatch_micros: self.metrics.max_dispatch_micros.load(Ordering::Relaxed),
            last_success_at_ms,
            last_failure_at_ms,
        }
    }

    /// Returns `true` only when the sink accepted a new decision.
    pub async fn dispatch(
        &self,
        request: &FlowDecisionRequest,
    ) -> Result<bool, FlowDecisionDispatchError> {
        let mut in_flight = DecisionInFlight::new(Arc::clone(&self.metrics));
        let result = self.dispatch_inner(request).await;
        in_flight.finish();
        self.record_result(&result);
        result
    }

    async fn dispatch_inner(
        &self,
        request: &FlowDecisionRequest,
    ) -> Result<bool, FlowDecisionDispatchError> {
        if request.authority_branch_id != self.production_branch_id {
            return Err(FlowDecisionDispatchError::UnauthorizedBranch {
                expected: self.production_branch_id.clone(),
                actual: request.authority_branch_id.clone(),
            });
        }
        if request.decision_id.trim().is_empty() || request.causation_event_id.trim().is_empty() {
            return Err(FlowDecisionDispatchError::InvalidIdentity);
        }
        let _dispatch_guard = self.dispatch_lock.lock().await;
        let request_hash = request_hash(request)?;
        match self
            .ledger
            .claim(
                &request.decision_id,
                &request_hash,
                &self.owner_id,
                now_ms(),
                self.lease_ms,
            )
            .await
            .map_err(|error| FlowDecisionDispatchError::Ledger(error.to_string()))?
        {
            FlowDecisionClaimOutcome::Completed => return Ok(false),
            FlowDecisionClaimOutcome::Busy {
                lease_expires_at_ms,
            } => {
                return Err(FlowDecisionDispatchError::Busy {
                    lease_expires_at_ms,
                })
            }
            FlowDecisionClaimOutcome::Conflict => {
                return Err(FlowDecisionDispatchError::DecisionIdConflict(
                    request.decision_id.clone(),
                ))
            }
            FlowDecisionClaimOutcome::Claimed { attempt } => {
                self.metrics.claimed.fetch_add(1, Ordering::Relaxed);
                if attempt > 1 {
                    self.metrics.takeovers.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        let heartbeat_period = Duration::from_millis((self.lease_ms / 3).max(1));
        let first_heartbeat = tokio::time::Instant::now() + heartbeat_period;
        let mut heartbeat = tokio::time::interval_at(first_heartbeat, heartbeat_period);
        let submission = self.sink.submit(request);
        tokio::pin!(submission);
        let submission_result = loop {
            tokio::select! {
                result = &mut submission => break result,
                _ = heartbeat.tick() => {
                    let renewed = self.ledger
                        .renew(
                            &request.decision_id,
                            &request_hash,
                            &self.owner_id,
                            now_ms(),
                            self.lease_ms,
                        )
                        .await
                        .map_err(|error| FlowDecisionDispatchError::Ledger(error.to_string()))?;
                    if !renewed {
                        return Err(FlowDecisionDispatchError::LeaseLost(
                            request.decision_id.clone(),
                        ));
                    }
                    self.metrics.lease_renewals.fetch_add(1, Ordering::Relaxed);
                }
            }
        };
        if let Err(error) = submission_result {
            if let Err(release_error) = self
                .ledger
                .release(&request.decision_id, &request_hash, &self.owner_id)
                .await
            {
                tracing::warn!(decision_id = request.decision_id, error = %release_error, "failed to release Flow decision claim");
            }
            return Err(FlowDecisionDispatchError::Sink(error.to_string()));
        }
        self.ledger
            .complete(
                &request.decision_id,
                &request_hash,
                &self.owner_id,
                now_ms(),
            )
            .await
            .map_err(|error| FlowDecisionDispatchError::Ledger(error.to_string()))?;
        Ok(true)
    }

    fn record_result(&self, result: &Result<bool, FlowDecisionDispatchError>) {
        match result {
            Ok(true) => {
                self.metrics.completed.fetch_add(1, Ordering::Relaxed);
                self.metrics.degraded.store(false, Ordering::Relaxed);
                self.metrics
                    .last_success_at_ms
                    .store(now_ms(), Ordering::Relaxed);
            }
            Ok(false) => {
                self.metrics.duplicates.fetch_add(1, Ordering::Relaxed);
                self.metrics.degraded.store(false, Ordering::Relaxed);
                self.metrics
                    .last_success_at_ms
                    .store(now_ms(), Ordering::Relaxed);
            }
            Err(FlowDecisionDispatchError::Busy { .. }) => {
                self.metrics.busy.fetch_add(1, Ordering::Relaxed);
            }
            Err(FlowDecisionDispatchError::DecisionIdConflict(_)) => {
                self.metrics.conflicts.fetch_add(1, Ordering::Relaxed);
            }
            Err(
                FlowDecisionDispatchError::UnauthorizedBranch { .. }
                | FlowDecisionDispatchError::InvalidIdentity,
            ) => {
                self.metrics.rejected.fetch_add(1, Ordering::Relaxed);
            }
            Err(FlowDecisionDispatchError::LeaseLost(_)) => {
                self.metrics.lease_lost.fetch_add(1, Ordering::Relaxed);
                self.record_failure();
            }
            Err(FlowDecisionDispatchError::Ledger(_)) => {
                self.metrics.ledger_failures.fetch_add(1, Ordering::Relaxed);
                self.record_failure();
            }
            Err(FlowDecisionDispatchError::Sink(_)) => {
                self.metrics.sink_failures.fetch_add(1, Ordering::Relaxed);
                self.record_failure();
            }
        }
    }

    fn record_failure(&self) {
        self.metrics.failures.fetch_add(1, Ordering::Relaxed);
        self.metrics.degraded.store(true, Ordering::Relaxed);
        self.metrics
            .last_failure_at_ms
            .store(now_ms(), Ordering::Relaxed);
    }
}

struct DecisionInFlight {
    metrics: Arc<FlowDecisionMetrics>,
    started: Instant,
    completed: bool,
}

impl DecisionInFlight {
    fn new(metrics: Arc<FlowDecisionMetrics>) -> Self {
        metrics.attempted.fetch_add(1, Ordering::Relaxed);
        metrics.in_flight.fetch_add(1, Ordering::Relaxed);
        Self {
            metrics,
            started: Instant::now(),
            completed: false,
        }
    }

    fn finish(&mut self) {
        self.record_elapsed();
        self.metrics.in_flight.fetch_sub(1, Ordering::Relaxed);
        self.completed = true;
    }

    fn record_elapsed(&self) {
        let elapsed = self.started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
        self.metrics
            .total_dispatch_micros
            .fetch_add(elapsed, Ordering::Relaxed);
        self.metrics
            .max_dispatch_micros
            .fetch_max(elapsed, Ordering::Relaxed);
    }
}

impl Drop for DecisionInFlight {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        self.record_elapsed();
        self.metrics.in_flight.fetch_sub(1, Ordering::Relaxed);
        self.metrics.failures.fetch_add(1, Ordering::Relaxed);
        self.metrics.cancellations.fetch_add(1, Ordering::Relaxed);
        self.metrics.degraded.store(true, Ordering::Relaxed);
        self.metrics
            .last_failure_at_ms
            .store(now_ms(), Ordering::Relaxed);
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FlowDecisionDispatchError {
    #[error("graph branch is not authorized for production Flow decisions: expected `{expected}`, got `{actual}`")]
    UnauthorizedBranch { expected: String, actual: String },
    #[error("decision_id and causation_event_id must be non-empty")]
    InvalidIdentity,
    #[error("Flow decision `{0}` reuses an existing decision id with different content")]
    DecisionIdConflict(String),
    #[error("Flow decision is owned by another dispatcher until {lease_expires_at_ms}")]
    Busy { lease_expires_at_ms: u64 },
    #[error("Flow decision `{0}` lost its lease while the sink was running")]
    LeaseLost(String),
    #[error("Flow decision ledger failed: {0}")]
    Ledger(String),
    #[error("Flow decision sink failed: {0}")]
    Sink(String),
}

fn request_hash(request: &FlowDecisionRequest) -> Result<String, FlowDecisionDispatchError> {
    serde_json::to_vec(request)
        .map(sha256::digest)
        .map_err(|error| FlowDecisionDispatchError::Ledger(error.to_string()))
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn nonzero(value: u64) -> Option<u64> {
    (value != 0).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileFlowDecisionLedger, FlowDecisionLedger};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<String>>);

    #[async_trait]
    impl FlowDecisionSink for RecordingSink {
        async fn submit(
            &self,
            request: &FlowDecisionRequest,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.0.lock().await.push(request.decision_id.clone());
            Ok(())
        }
    }

    fn request(branch: &str) -> FlowDecisionRequest {
        FlowDecisionRequest {
            decision_id: "decision-1".into(),
            run_id: "run-1".into(),
            authority_branch_id: branch.into(),
            causation_event_id: "event-1".into(),
            decision: FlowDecision::Complete {
                output: Value::Null,
            },
        }
    }

    #[tokio::test]
    async fn submits_once_and_rejects_fork_branches() {
        let sink = Arc::new(RecordingSink::default());
        let dispatcher = FlowDecisionDispatcher::new("production", sink.clone());
        assert!(dispatcher.dispatch(&request("production")).await.unwrap());
        assert!(!dispatcher.dispatch(&request("production")).await.unwrap());
        assert_eq!(sink.0.lock().await.as_slice(), ["decision-1"]);
        assert!(matches!(
            dispatcher.dispatch(&request("fork")).await,
            Err(FlowDecisionDispatchError::UnauthorizedBranch { .. })
        ));
        let health = dispatcher.health();
        assert_eq!(health.status, FlowDecisionHealthStatus::Healthy);
        assert_eq!(health.attempted, 3);
        assert_eq!(health.claimed, 1);
        assert_eq!(health.completed, 1);
        assert_eq!(health.duplicates, 1);
        assert_eq!(health.rejected, 1);
        assert_eq!(health.in_flight, 0);
        assert!(health.last_success_at_ms.is_some());
    }

    #[tokio::test]
    async fn completed_receipt_survives_dispatcher_restart() {
        let sink = Arc::new(RecordingSink::default());
        let ledger = Arc::new(MemoryFlowDecisionLedger::new());
        let first = FlowDecisionDispatcher::with_ledger("production", sink.clone(), ledger.clone());
        assert!(first.dispatch(&request("production")).await.unwrap());
        let restarted = FlowDecisionDispatcher::with_ledger("production", sink.clone(), ledger);
        assert!(!restarted.dispatch(&request("production")).await.unwrap());
        assert_eq!(sink.0.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn decision_id_cannot_be_reused_with_different_content() {
        let sink = Arc::new(RecordingSink::default());
        let ledger = Arc::new(MemoryFlowDecisionLedger::new());
        let dispatcher = FlowDecisionDispatcher::with_ledger("production", sink, ledger);
        dispatcher.dispatch(&request("production")).await.unwrap();
        let mut conflicting = request("production");
        conflicting.decision = FlowDecision::Fail {
            error: "different".into(),
        };
        assert!(matches!(
            dispatcher.dispatch(&conflicting).await,
            Err(FlowDecisionDispatchError::DecisionIdConflict(_))
        ));
        assert_eq!(dispatcher.health().conflicts, 1);
    }

    struct FailingOnceSink(AtomicUsize);

    #[async_trait]
    impl FlowDecisionSink for FailingOnceSink {
        async fn submit(
            &self,
            _request: &FlowDecisionRequest,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(std::io::Error::other("transient").into());
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn sink_failure_releases_claim_for_retry() {
        let sink = Arc::new(FailingOnceSink(AtomicUsize::new(0)));
        let dispatcher = FlowDecisionDispatcher::with_ledger(
            "production",
            sink.clone(),
            Arc::new(MemoryFlowDecisionLedger::new()),
        );
        assert!(matches!(
            dispatcher.dispatch(&request("production")).await,
            Err(FlowDecisionDispatchError::Sink(_))
        ));
        assert!(dispatcher.dispatch(&request("production")).await.unwrap());
        assert_eq!(sink.0.load(Ordering::SeqCst), 2);
        let health = dispatcher.health();
        assert_eq!(health.status, FlowDecisionHealthStatus::Healthy);
        assert_eq!(health.sink_failures, 1);
        assert_eq!(health.failures, 1);
        assert_eq!(health.completed, 1);
    }

    #[tokio::test]
    async fn independent_file_ledgers_serialize_claim_and_allow_expired_takeover() {
        let directory = tempfile::tempdir().unwrap();
        let left = FileFlowDecisionLedger::new(directory.path());
        let right = FileFlowDecisionLedger::new(directory.path());
        let (left_claim, right_claim) = tokio::join!(
            left.claim("decision", "hash", "left", 100, 50),
            right.claim("decision", "hash", "right", 100, 50),
        );
        let claims = [left_claim.unwrap(), right_claim.unwrap()];
        assert_eq!(
            claims
                .iter()
                .filter(|claim| matches!(claim, FlowDecisionClaimOutcome::Claimed { .. }))
                .count(),
            1
        );
        assert_eq!(
            claims
                .iter()
                .filter(|claim| matches!(claim, FlowDecisionClaimOutcome::Busy { .. }))
                .count(),
            1
        );
        assert_eq!(
            right
                .claim("decision", "hash", "takeover", 151, 50)
                .await
                .unwrap(),
            FlowDecisionClaimOutcome::Claimed { attempt: 2 }
        );
        right
            .complete("decision", "hash", "takeover", 160)
            .await
            .unwrap();
        assert_eq!(right.prune_completed(161).await.unwrap(), 1);
        assert_eq!(
            left.claim("decision", "hash", "new", 162, 50)
                .await
                .unwrap(),
            FlowDecisionClaimOutcome::Claimed { attempt: 1 }
        );
    }

    #[tokio::test]
    async fn lease_renewal_requires_current_owner_and_preserves_attempt() {
        let ledger = MemoryFlowDecisionLedger::new();
        assert_eq!(
            ledger
                .claim("decision", "hash", "owner", 100, 30)
                .await
                .unwrap(),
            FlowDecisionClaimOutcome::Claimed { attempt: 1 }
        );
        assert!(!ledger
            .renew("decision", "hash", "other", 120, 30)
            .await
            .unwrap());
        assert!(ledger
            .renew("decision", "hash", "owner", 120, 30)
            .await
            .unwrap());
        assert_eq!(
            ledger
                .claim("decision", "hash", "other", 145, 30)
                .await
                .unwrap(),
            FlowDecisionClaimOutcome::Busy {
                lease_expires_at_ms: 150
            }
        );
        assert_eq!(
            ledger
                .claim("decision", "hash", "other", 151, 30)
                .await
                .unwrap(),
            FlowDecisionClaimOutcome::Claimed { attempt: 2 }
        );
        assert!(!ledger
            .renew("decision", "hash", "owner", 152, 30)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn file_ledger_persists_renewed_lease_across_instances() {
        let directory = tempfile::tempdir().unwrap();
        let owner = FileFlowDecisionLedger::new(directory.path());
        let competitor = FileFlowDecisionLedger::new(directory.path());
        assert_eq!(
            owner
                .claim("decision", "hash", "owner", 100, 20)
                .await
                .unwrap(),
            FlowDecisionClaimOutcome::Claimed { attempt: 1 }
        );
        assert!(owner
            .renew("decision", "hash", "owner", 115, 30)
            .await
            .unwrap());
        assert_eq!(
            competitor
                .claim("decision", "hash", "competitor", 125, 20)
                .await
                .unwrap(),
            FlowDecisionClaimOutcome::Busy {
                lease_expires_at_ms: 145
            }
        );
    }

    struct SlowCountingSink(AtomicUsize);

    #[async_trait]
    impl FlowDecisionSink for SlowCountingSink {
        async fn submit(
            &self,
            _request: &FlowDecisionRequest,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn competing_file_backed_dispatchers_submit_to_sink_once() {
        let directory = tempfile::tempdir().unwrap();
        let sink = Arc::new(SlowCountingSink(AtomicUsize::new(0)));
        let left = FlowDecisionDispatcher::with_ledger(
            "production",
            sink.clone(),
            Arc::new(FileFlowDecisionLedger::new(directory.path())),
        );
        let right = FlowDecisionDispatcher::with_ledger(
            "production",
            sink.clone(),
            Arc::new(FileFlowDecisionLedger::new(directory.path())),
        );
        let request = request("production");
        let (left_result, right_result) =
            tokio::join!(left.dispatch(&request), right.dispatch(&request));
        let results = [left_result, right_result];
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Ok(true)))
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(FlowDecisionDispatchError::Busy { .. })))
                .count(),
            1
        );
        assert_eq!(sink.0.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn heartbeat_prevents_takeover_while_sink_is_running() {
        let sink = Arc::new(SlowCountingSink(AtomicUsize::new(0)));
        let ledger = Arc::new(MemoryFlowDecisionLedger::new());
        let left = FlowDecisionDispatcher::with_ledger("production", sink.clone(), ledger.clone())
            .with_lease_ms(15);
        let right = FlowDecisionDispatcher::with_ledger("production", sink.clone(), ledger)
            .with_lease_ms(15);
        let request = request("production");
        let left_dispatch = left.dispatch(&request);
        let competing_dispatch = async {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            right.dispatch(&request).await
        };
        let (left_result, right_result) = tokio::join!(left_dispatch, competing_dispatch);
        assert!(left_result.unwrap());
        assert!(matches!(
            right_result,
            Err(FlowDecisionDispatchError::Busy { .. })
        ));
        assert_eq!(sink.0.load(Ordering::SeqCst), 1);
        let left_health = left.health();
        assert_eq!(left_health.completed, 1);
        assert!(left_health.lease_renewals > 0);
        assert_eq!(right.health().busy, 1);
    }

    #[tokio::test]
    async fn dispatcher_health_counts_expired_claim_takeover() {
        let ledger = Arc::new(MemoryFlowDecisionLedger::new());
        let request = request("production");
        let hash = request_hash(&request).unwrap();
        ledger
            .claim(
                &request.decision_id,
                &hash,
                "expired-owner",
                now_ms().saturating_sub(10),
                1,
            )
            .await
            .unwrap();
        let dispatcher = FlowDecisionDispatcher::with_ledger(
            "production",
            Arc::new(RecordingSink::default()),
            ledger,
        );
        assert!(dispatcher.dispatch(&request).await.unwrap());
        let health = dispatcher.health();
        assert_eq!(health.claimed, 1);
        assert_eq!(health.takeovers, 1);
        assert_eq!(health.completed, 1);
    }

    #[derive(Default)]
    struct LeaseLosingLedger;

    #[async_trait]
    impl FlowDecisionLedger for LeaseLosingLedger {
        async fn claim(
            &self,
            _decision_id: &str,
            _request_hash: &str,
            _owner_id: &str,
            _now_ms: u64,
            _lease_ms: u64,
        ) -> anyhow::Result<FlowDecisionClaimOutcome> {
            Ok(FlowDecisionClaimOutcome::Claimed { attempt: 1 })
        }

        async fn renew(
            &self,
            _decision_id: &str,
            _request_hash: &str,
            _owner_id: &str,
            _now_ms: u64,
            _lease_ms: u64,
        ) -> anyhow::Result<bool> {
            Ok(false)
        }

        async fn complete(
            &self,
            _decision_id: &str,
            _request_hash: &str,
            _owner_id: &str,
            _completed_at_ms: u64,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn release(
            &self,
            _decision_id: &str,
            _request_hash: &str,
            _owner_id: &str,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct ClaimFailingLedger;

    #[async_trait]
    impl FlowDecisionLedger for ClaimFailingLedger {
        async fn claim(
            &self,
            _decision_id: &str,
            _request_hash: &str,
            _owner_id: &str,
            _now_ms: u64,
            _lease_ms: u64,
        ) -> anyhow::Result<FlowDecisionClaimOutcome> {
            anyhow::bail!("ledger unavailable")
        }

        async fn renew(
            &self,
            _decision_id: &str,
            _request_hash: &str,
            _owner_id: &str,
            _now_ms: u64,
            _lease_ms: u64,
        ) -> anyhow::Result<bool> {
            Ok(false)
        }

        async fn complete(
            &self,
            _decision_id: &str,
            _request_hash: &str,
            _owner_id: &str,
            _completed_at_ms: u64,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn release(
            &self,
            _decision_id: &str,
            _request_hash: &str,
            _owner_id: &str,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn dispatcher_health_records_ledger_failure() {
        let dispatcher = FlowDecisionDispatcher::with_ledger(
            "production",
            Arc::new(RecordingSink::default()),
            Arc::new(ClaimFailingLedger),
        );
        assert!(matches!(
            dispatcher.dispatch(&request("production")).await,
            Err(FlowDecisionDispatchError::Ledger(_))
        ));
        let health = dispatcher.health();
        assert_eq!(health.status, FlowDecisionHealthStatus::Degraded);
        assert_eq!(health.ledger_failures, 1);
        assert_eq!(health.in_flight, 0);
    }

    struct CancellableSink {
        started: AtomicUsize,
        completed: AtomicUsize,
    }

    #[async_trait]
    impl FlowDecisionSink for CancellableSink {
        async fn submit(
            &self,
            _request: &FlowDecisionRequest,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.started.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            self.completed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn lost_lease_cancels_in_flight_sink_future() {
        let sink = Arc::new(CancellableSink {
            started: AtomicUsize::new(0),
            completed: AtomicUsize::new(0),
        });
        let dispatcher = FlowDecisionDispatcher::with_ledger(
            "production",
            sink.clone(),
            Arc::new(LeaseLosingLedger),
        )
        .with_lease_ms(9);
        assert!(matches!(
            dispatcher.dispatch(&request("production")).await,
            Err(FlowDecisionDispatchError::LeaseLost(_))
        ));
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        assert_eq!(sink.started.load(Ordering::SeqCst), 1);
        assert_eq!(sink.completed.load(Ordering::SeqCst), 0);
        let health = dispatcher.health();
        assert_eq!(health.status, FlowDecisionHealthStatus::Degraded);
        assert_eq!(health.lease_lost, 1);
        assert_eq!(health.in_flight, 0);
        assert!(health.last_failure_at_ms.is_some());
    }

    struct BlockingSink(Arc<tokio::sync::Notify>);

    #[async_trait]
    impl FlowDecisionSink for BlockingSink {
        async fn submit(
            &self,
            _request: &FlowDecisionRequest,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.0.notify_one();
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn cancelled_dispatch_does_not_leak_in_flight_health() {
        let started = Arc::new(tokio::sync::Notify::new());
        let dispatcher = Arc::new(FlowDecisionDispatcher::new(
            "production",
            Arc::new(BlockingSink(started.clone())),
        ));
        let task_dispatcher = dispatcher.clone();
        let pending =
            tokio::spawn(async move { task_dispatcher.dispatch(&request("production")).await });
        started.notified().await;
        assert_eq!(dispatcher.health().in_flight, 1);
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        let health = dispatcher.health();
        assert_eq!(health.status, FlowDecisionHealthStatus::Degraded);
        assert_eq!(health.attempted, 1);
        assert_eq!(health.cancellations, 1);
        assert_eq!(health.in_flight, 0);
    }
}
