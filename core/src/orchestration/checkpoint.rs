//! Workflow-level checkpoints: journal completed steps so an interrupted
//! orchestration resumes from the longest completed prefix — on this node or,
//! because the checkpoint is serializable and the executor is pluggable, on
//! another one (host-driven migration).
//!
//! This is the step-boundary analogue of [`LoopCheckpoint`](crate::loop_checkpoint::LoopCheckpoint),
//! which checkpoints at tool-round boundaries one level down.

use super::executor::{AgentStepSpec, StepOutcome};
use crate::evaluation::{digest_bytes, digest_json};
use crate::execution_identity::{
    ExecutionIdentityV1, ExecutionResultOutcomeV1, ExecutionResultReceiptV1,
    WORKFLOW_STEP_EVIDENCE_DOMAIN_V1, WORKFLOW_STEP_IDENTITY_DOMAIN_V1,
    WORKFLOW_STEP_RESULT_DOMAIN_V1,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Schema version. Bumped on incompatible format changes; loads from a future
/// version are rejected (see [`WorkflowCheckpoint::ensure_loadable`]).
pub const WORKFLOW_CHECKPOINT_SCHEMA_VERSION: u32 = 1;

/// One completed step within a workflow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowStepRecord {
    /// Matches the [`AgentStepSpec::task_id`](super::AgentStepSpec) of the
    /// step that produced this outcome.
    pub task_id: String,
    /// The completed step's result.
    pub outcome: StepOutcome,
    /// Digest-only result metadata bound to this step's execution identity.
    /// Older checkpoints omit this field and remain loadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_receipt: Option<ExecutionResultReceiptV1>,
}

/// Snapshot of a workflow's completed steps at a step boundary.
///
/// (`StepOutcome` contains a `serde_json::Value`, which is not `Eq`, so this
/// derives `PartialEq` only.)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowCheckpoint {
    /// Schema version — see [`WORKFLOW_CHECKPOINT_SCHEMA_VERSION`].
    #[serde(default)]
    pub schema_version: u32,
    /// Logical workflow identifier the checkpoint is keyed by.
    pub workflow_id: String,
    /// The steps completed so far. A resuming run skips these and re-dispatches
    /// only the rest.
    pub steps: Vec<WorkflowStepRecord>,
    /// Wall-clock timestamp when the checkpoint was written (Unix epoch ms).
    pub checkpoint_ms: u64,
}

impl WorkflowCheckpoint {
    /// Build a checkpoint from a map of completed `task_id -> outcome`.
    pub fn from_completed(
        workflow_id: impl Into<String>,
        completed: &HashMap<String, StepOutcome>,
        checkpoint_ms: u64,
    ) -> Self {
        let steps = completed
            .iter()
            .map(|(task_id, outcome)| WorkflowStepRecord {
                task_id: task_id.clone(),
                outcome: outcome.clone(),
                result_receipt: None,
            })
            .collect();
        Self {
            schema_version: WORKFLOW_CHECKPOINT_SCHEMA_VERSION,
            workflow_id: workflow_id.into(),
            steps,
            checkpoint_ms,
        }
    }

    /// Build a checkpoint carrying result receipts produced by the same
    /// execution boundary as the completed outcomes.
    pub fn from_completed_with_receipts(
        workflow_id: impl Into<String>,
        completed: &HashMap<String, StepOutcome>,
        receipts: &HashMap<String, ExecutionResultReceiptV1>,
        checkpoint_ms: u64,
    ) -> Self {
        let mut checkpoint = Self::from_completed(workflow_id, completed, checkpoint_ms);
        for record in &mut checkpoint.steps {
            record.result_receipt = receipts.get(&record.task_id).cloned();
        }
        checkpoint
    }

    /// The completed steps as a `task_id -> outcome` map.
    pub fn completed(&self) -> HashMap<String, StepOutcome> {
        self.steps
            .iter()
            .map(|r| (r.task_id.clone(), r.outcome.clone()))
            .collect()
    }

    /// Reject a checkpoint written by a *newer*, incompatible schema version
    /// than this build understands — mirrors
    /// [`LoopCheckpoint::ensure_loadable`](crate::loop_checkpoint::LoopCheckpoint::ensure_loadable).
    /// Field additions are absorbed by `#[serde(default)]`, so older (incl.
    /// pre-v1 `0`) checkpoints always remain loadable.
    pub fn ensure_loadable(&self) -> anyhow::Result<()> {
        if self.schema_version > WORKFLOW_CHECKPOINT_SCHEMA_VERSION {
            anyhow::bail!(
                "workflow checkpoint {} has schema version {} but this build supports at \
                 most {}; refusing to resume from an incompatible future checkpoint",
                self.workflow_id,
                self.schema_version,
                WORKFLOW_CHECKPOINT_SCHEMA_VERSION
            );
        }
        for record in &self.steps {
            if record.task_id != record.outcome.task_id {
                anyhow::bail!(
                    "workflow checkpoint {} has task id mismatch for step {:?}",
                    self.workflow_id,
                    record.task_id
                );
            }
            if let Some(receipt) = &record.result_receipt {
                receipt.validate().map_err(|error| {
                    anyhow::anyhow!(
                        "workflow checkpoint {} has invalid result receipt for step {:?}: {error}",
                        self.workflow_id,
                        record.task_id
                    )
                })?;
                if receipt.identity.domain != WORKFLOW_STEP_IDENTITY_DOMAIN_V1 {
                    anyhow::bail!(
                        "workflow checkpoint {} has an unsupported result identity for step {:?}",
                        self.workflow_id,
                        record.task_id
                    );
                }
            }
        }
        Ok(())
    }

    /// Verify any identity-bearing cached steps against the current workflow
    /// specs. Legacy records without receipts remain compatible; new records
    /// fail closed when a task id is reused for a different invocation.
    pub fn validate_for_specs(
        &self,
        workflow_id: &str,
        specs: &[AgentStepSpec],
    ) -> anyhow::Result<()> {
        self.ensure_loadable()?;
        if self.workflow_id != workflow_id {
            anyhow::bail!(
                "workflow checkpoint is keyed to `{}`, not `{workflow_id}`",
                self.workflow_id
            );
        }
        for record in &self.steps {
            let Some(receipt) = &record.result_receipt else {
                continue;
            };
            let Some(spec) = specs.iter().find(|spec| spec.task_id == record.task_id) else {
                continue;
            };
            let expected =
                workflow_step_execution_identity(workflow_id, spec).map_err(|error| {
                    anyhow::anyhow!(
                        "cannot derive workflow step identity for {:?}: {error}",
                        record.task_id
                    )
                })?;
            if receipt.identity != expected {
                anyhow::bail!(
                    "workflow checkpoint {} has a stale result identity for step {:?}",
                    workflow_id,
                    record.task_id
                );
            }
            let expected_result =
                workflow_step_result_receipt(workflow_id, spec, &record.outcome, None).map_err(
                    |error| {
                        anyhow::anyhow!(
                            "cannot derive workflow result receipt for {:?}: {error}",
                            record.task_id
                        )
                    },
                )?;
            if receipt.outcome != expected_result.outcome
                || receipt.result_digest != expected_result.result_digest
                || receipt.result_bytes != expected_result.result_bytes
            {
                anyhow::bail!(
                    "workflow checkpoint {} has a stale result receipt for step {:?}",
                    workflow_id,
                    record.task_id
                );
            }
        }
        Ok(())
    }
}

/// Derive the canonical identity for one workflow step invocation.
pub fn workflow_step_execution_identity(
    workflow_id: &str,
    spec: &AgentStepSpec,
) -> Result<ExecutionIdentityV1, crate::execution_identity::ExecutionIdentityError> {
    ExecutionIdentityV1::derive(
        WORKFLOW_STEP_IDENTITY_DOMAIN_V1,
        &serde_json::json!({
            "workflow_id": workflow_id,
            "task_id": spec.task_id,
            "agent": spec.agent,
            "description": spec.description,
            "prompt": spec.prompt,
            "max_steps": spec.max_steps,
            "parent_session_id": spec.parent_session_id,
            "output_schema": spec.output_schema,
        }),
    )
}

/// Build a bounded, digest-only receipt for one completed workflow step.
///
/// A host may provide the digest of a richer immutable evidence snapshot. When
/// it does not, the fallback digest covers the normalized source-anchor
/// projection available in [`StepOutcome`]. The step output itself is never
/// copied into the receipt.
pub fn workflow_step_result_receipt(
    workflow_id: &str,
    spec: &AgentStepSpec,
    outcome: &StepOutcome,
    evidence_digest: Option<&str>,
) -> Result<ExecutionResultReceiptV1, crate::execution_identity::ExecutionIdentityError> {
    if spec.task_id != outcome.task_id {
        return Err(
            crate::execution_identity::ExecutionIdentityError::InvalidReceiptField("task_id"),
        );
    }
    if spec.agent != outcome.agent {
        return Err(
            crate::execution_identity::ExecutionIdentityError::InvalidReceiptField("agent"),
        );
    }
    let identity = workflow_step_execution_identity(workflow_id, spec)?;
    let evidence_digest = match evidence_digest {
        Some(digest) => digest.to_string(),
        None => digest_json(WORKFLOW_STEP_EVIDENCE_DOMAIN_V1, &outcome.source_anchors).map_err(
            |error| {
                crate::execution_identity::ExecutionIdentityError::Serialization(error.to_string())
            },
        )?,
    };
    let (result, result_bytes, result_digest) = if outcome.success {
        let encoded = serde_json::to_vec(&serde_json::json!({
            "output": &outcome.output,
            "structured": &outcome.structured,
        }))
        .map_err(|error| {
            crate::execution_identity::ExecutionIdentityError::Serialization(error.to_string())
        })?;
        (
            ExecutionResultOutcomeV1::Succeeded,
            u64::try_from(encoded.len())
                .map_err(|_| crate::execution_identity::ExecutionIdentityError::ReceiptSizeLimit)?,
            Some(digest_bytes(WORKFLOW_STEP_RESULT_DOMAIN_V1, &encoded)),
        )
    } else {
        (ExecutionResultOutcomeV1::Failed, 0, None)
    };
    ExecutionResultReceiptV1::new(
        identity,
        evidence_digest,
        result,
        result_digest,
        result_bytes,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(id: &str) -> StepOutcome {
        StepOutcome {
            task_id: id.to_string(),
            session_id: format!("task-run-{id}"),
            agent: "a".to_string(),
            output: "o".to_string(),
            success: true,
            structured: None,
            source_anchors: Vec::new(),
        }
    }

    fn spec(id: &str, prompt: &str) -> AgentStepSpec {
        AgentStepSpec::new(id, "a", "description", prompt)
    }

    #[test]
    fn round_trips_and_exposes_completed_map() {
        let mut completed = HashMap::new();
        completed.insert("t1".to_string(), outcome("t1"));
        let cp = WorkflowCheckpoint::from_completed("wf", &completed, 123);
        let back: WorkflowCheckpoint =
            serde_json::from_str(&serde_json::to_string(&cp).unwrap()).unwrap();
        assert_eq!(back, cp);
        assert_eq!(back.schema_version, WORKFLOW_CHECKPOINT_SCHEMA_VERSION);
        assert_eq!(back.checkpoint_ms, 123);
        assert_eq!(back.completed().get("t1").unwrap().task_id, "t1");
        assert!(back.steps[0].result_receipt.is_none());
    }

    #[test]
    fn ensure_loadable_rejects_only_future_versions() {
        let mut cp = WorkflowCheckpoint::from_completed("wf", &HashMap::new(), 0);
        cp.ensure_loadable().expect("current version loadable");
        cp.schema_version = 0;
        cp.ensure_loadable().expect("pre-v1 loadable");
        cp.schema_version = WORKFLOW_CHECKPOINT_SCHEMA_VERSION + 1;
        let err = cp.ensure_loadable().unwrap_err();
        assert!(err.to_string().contains("schema version"), "got: {err}");
    }

    #[test]
    fn pre_v1_payload_without_schema_version_loads() {
        let json = r#"{"workflow_id":"wf","steps":[],"checkpoint_ms":0}"#;
        let cp: WorkflowCheckpoint = serde_json::from_str(json).unwrap();
        assert_eq!(cp.schema_version, 0);
    }

    #[test]
    fn receipt_binds_step_identity_and_host_evidence_without_plaintext() {
        let step = spec("t1", "private prompt");
        let receipt = workflow_step_result_receipt(
            "wf",
            &step,
            &outcome("t1"),
            Some("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        )
        .unwrap();
        receipt.validate().unwrap();
        assert_eq!(receipt.identity.domain, WORKFLOW_STEP_IDENTITY_DOMAIN_V1);
        assert_eq!(
            receipt.evidence_digest,
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert!(receipt.result_digest.is_some());
        assert!(receipt.result_bytes > 0);
        assert!(!format!("{receipt:?}").contains("private prompt"));
    }

    #[test]
    fn result_receipt_tampering_is_rejected_for_the_current_step() {
        let step = spec("t1", "private prompt");
        let result = outcome("t1");
        let receipt = workflow_step_result_receipt("wf", &step, &result, None).unwrap();
        let mut completed = HashMap::new();
        completed.insert("t1".to_string(), result);
        let mut receipts = HashMap::new();
        receipts.insert("t1".to_string(), receipt);
        let mut checkpoint =
            WorkflowCheckpoint::from_completed_with_receipts("wf", &completed, &receipts, 1);
        checkpoint.steps[0]
            .result_receipt
            .as_mut()
            .unwrap()
            .result_bytes += 1;
        assert!(checkpoint
            .validate_for_specs("wf", &[step])
            .unwrap_err()
            .to_string()
            .contains("stale result receipt"));
    }

    #[test]
    fn legacy_step_record_without_receipt_remains_loadable() {
        let json = r#"{
            "workflow_id":"wf",
            "steps":[{
                "task_id":"t1",
                "outcome":{
                    "task_id":"t1",
                    "session_id":"task-run-t1",
                    "agent":"a",
                    "output":"legacy",
                    "success":true
                }
            }],
            "checkpoint_ms":1
        }"#;
        let cp: WorkflowCheckpoint = serde_json::from_str(json).unwrap();
        assert!(cp.steps[0].result_receipt.is_none());
        cp.ensure_loadable().unwrap();
    }
}
