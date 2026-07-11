use a3s_flow::RetryPolicy;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
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

/// Enforces production-branch authority and idempotent successful submission.
pub struct FlowDecisionDispatcher {
    production_branch_id: String,
    sink: Arc<dyn FlowDecisionSink>,
    ledger: Arc<dyn FlowDecisionLedger>,
    owner_id: String,
    lease_ms: u64,
    dispatch_lock: Mutex<()>,
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
        }
    }

    pub fn with_lease_ms(mut self, lease_ms: u64) -> Self {
        self.lease_ms = lease_ms.max(1);
        self
    }

    /// Returns `true` only when the sink accepted a new decision.
    pub async fn dispatch(
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
            FlowDecisionClaimOutcome::Claimed { .. } => {}
        }
        if let Err(error) = self.sink.submit(request).await {
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
}
use super::decision_ledger::{
    FlowDecisionClaimOutcome, FlowDecisionLedger, MemoryFlowDecisionLedger,
};
