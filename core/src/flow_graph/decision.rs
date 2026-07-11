use a3s_flow::RetryPolicy;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
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
    async fn submit(
        &self,
        request: &FlowDecisionRequest,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

/// Enforces production-branch authority and idempotent successful submission.
pub struct FlowDecisionDispatcher {
    production_branch_id: String,
    sink: Arc<dyn FlowDecisionSink>,
    submitted: Mutex<BTreeSet<String>>,
}

impl FlowDecisionDispatcher {
    pub fn new(production_branch_id: impl Into<String>, sink: Arc<dyn FlowDecisionSink>) -> Self {
        Self {
            production_branch_id: production_branch_id.into(),
            sink,
            submitted: Mutex::new(BTreeSet::new()),
        }
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
        let mut submitted = self.submitted.lock().await;
        if submitted.contains(&request.decision_id) {
            return Ok(false);
        }
        self.sink
            .submit(request)
            .await
            .map_err(|error| FlowDecisionDispatchError::Sink(error.to_string()))?;
        submitted.insert(request.decision_id.clone());
        Ok(true)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FlowDecisionDispatchError {
    #[error("graph branch is not authorized for production Flow decisions: expected `{expected}`, got `{actual}`")]
    UnauthorizedBranch { expected: String, actual: String },
    #[error("decision_id and causation_event_id must be non-empty")]
    InvalidIdentity,
    #[error("Flow decision sink failed: {0}")]
    Sink(String),
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
