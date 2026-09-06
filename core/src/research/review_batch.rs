//! Bounded, digest-bound batches of reviewer findings.

use super::{
    digest, validate_digest_field, validate_id, ResearchContractError, ResearchReviewFindingV1,
    ResearchReviewStatusV1,
};
use serde::{Deserialize, Serialize};

pub const RESEARCH_REVIEW_BATCH_SCHEMA_V1: &str = "a3s.code.review-batch.v1";
pub const RESEARCH_MAX_REVIEW_FINDINGS: usize = 512;
const RESEARCH_REVIEW_BATCH_DIGEST_DOMAIN: &str = "a3s.code.review-batch.identity.v1";

/// One immutable projection of an evaluator result into bounded findings.
///
/// A batch does not define a rubric or a business decision. It only prevents
/// a host from publishing a partially mixed reviewer response: every finding
/// must belong to the same project/run, evaluation record, and evidence
/// snapshot. A batch may contain zero findings to represent a clean reviewer
/// result; the evaluator record remains the authoritative result and evidence
/// identity. Human resolution remains an explicit operation on each finding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResearchReviewBatchV1 {
    pub schema: String,
    pub batch_id: String,
    pub project_id: String,
    pub run_id: String,
    pub evaluation_record_digest: String,
    pub evidence_digest: String,
    pub findings: Vec<ResearchReviewFindingV1>,
    pub batch_digest: String,
}

impl ResearchReviewBatchV1 {
    /// Construct a batch against the admitted Run and exact evaluator record.
    ///
    /// The identity-only [`new`](Self::new) constructor remains available for
    /// compatibility with callers that only have wire-level digests. New
    /// reviewer pipelines should use this constructor so the project/run
    /// namespace and evaluator evidence snapshot are closed at admission. A
    /// newly admitted batch must contain only open findings; resolved or
    /// waived findings must be restored from the already-published batch and
    /// changed through the explicit transition methods below.
    pub fn new_for_run(
        batch_id: impl Into<String>,
        run: &crate::research::ResearchRunV1,
        record: &crate::evaluation::EvaluationRecordV1,
        evidence_digest: impl Into<String>,
        findings: Vec<ResearchReviewFindingV1>,
    ) -> Result<Self, ResearchContractError> {
        let batch = Self::new(
            batch_id,
            run.project_id.clone(),
            run.run_id.clone(),
            record.record_digest.clone(),
            evidence_digest,
            findings,
        )?;
        batch.validate_for_run(run, record)?;
        if batch
            .findings
            .iter()
            .any(|finding| !matches!(finding.status, ResearchReviewStatusV1::Open))
        {
            return Err(ResearchContractError::InvalidField("finding.status"));
        }
        Ok(batch)
    }

    pub fn new(
        batch_id: impl Into<String>,
        project_id: impl Into<String>,
        run_id: impl Into<String>,
        evaluation_record_digest: impl Into<String>,
        evidence_digest: impl Into<String>,
        mut findings: Vec<ResearchReviewFindingV1>,
    ) -> Result<Self, ResearchContractError> {
        findings.sort_unstable_by(|left, right| left.finding_id.cmp(&right.finding_id));
        let mut batch = Self {
            schema: RESEARCH_REVIEW_BATCH_SCHEMA_V1.to_owned(),
            batch_id: batch_id.into(),
            project_id: project_id.into(),
            run_id: run_id.into(),
            evaluation_record_digest: evaluation_record_digest.into(),
            evidence_digest: evidence_digest.into(),
            findings,
            batch_digest: String::new(),
        };
        batch.validate_without_digest()?;
        batch.batch_digest = batch.expected_digest()?;
        Ok(batch)
    }

    pub fn validate(&self) -> Result<(), ResearchContractError> {
        self.validate_without_digest()?;
        validate_digest_field("batchDigest", &self.batch_digest)?;
        if self.batch_digest != self.expected_digest()? {
            return Err(ResearchContractError::DigestMismatch("batchDigest"));
        }
        Ok(())
    }

    /// Validate this batch against the admitted Run and exact evaluator
    /// record that the host intends to publish.
    pub fn validate_for_run(
        &self,
        run: &crate::research::ResearchRunV1,
        record: &crate::evaluation::EvaluationRecordV1,
    ) -> Result<(), ResearchContractError> {
        self.validate()?;
        run.validate_reviewable()?;
        record
            .validate()
            .map_err(|_| ResearchContractError::InvalidField("evaluationRecord"))?;
        if run.project_id != self.project_id {
            return Err(ResearchContractError::InvalidField("researchRun.projectId"));
        }
        if run.run_id != self.run_id {
            return Err(ResearchContractError::InvalidField("researchRun.runId"));
        }
        if record.record_digest != self.evaluation_record_digest {
            return Err(ResearchContractError::InvalidField(
                "evaluationRecord.recordDigest",
            ));
        }
        if record.result.target.run_id != run.run_id {
            return Err(ResearchContractError::InvalidField(
                "evaluationRecord.target.runId",
            ));
        }
        if record.result.evidence_digest != run.evidence_snapshot_digest {
            return Err(ResearchContractError::InvalidField(
                "evaluationRecord.evidenceDigest",
            ));
        }
        if record.result.evidence_digest != self.evidence_digest {
            return Err(ResearchContractError::InvalidField(
                "evaluationRecord.evidenceDigest",
            ));
        }
        for finding in &self.findings {
            if finding.evaluator_id != record.result.evaluator_id {
                return Err(ResearchContractError::InvalidField("finding.evaluatorId"));
            }
        }
        Ok(())
    }

    /// Resolve one finding and rebind the batch identity atomically.
    pub fn resolve_finding(
        &mut self,
        finding_id: &str,
        resolution_digest: impl Into<String>,
    ) -> Result<(), ResearchContractError> {
        self.validate()?;
        let finding = self
            .findings
            .iter_mut()
            .find(|finding| finding.finding_id == finding_id)
            .ok_or(ResearchContractError::InvalidField("findingId"))?;
        finding.resolve(resolution_digest)?;
        self.batch_digest = self.expected_digest()?;
        self.validate()
    }

    /// Waive one finding and rebind the batch identity atomically.
    pub fn waive_finding(
        &mut self,
        finding_id: &str,
        resolution_digest: impl Into<String>,
    ) -> Result<(), ResearchContractError> {
        self.validate()?;
        let finding = self
            .findings
            .iter_mut()
            .find(|finding| finding.finding_id == finding_id)
            .ok_or(ResearchContractError::InvalidField("findingId"))?;
        finding.waive(resolution_digest)?;
        self.batch_digest = self.expected_digest()?;
        self.validate()
    }

    fn validate_without_digest(&self) -> Result<(), ResearchContractError> {
        if self.schema != RESEARCH_REVIEW_BATCH_SCHEMA_V1 {
            return Err(ResearchContractError::UnsupportedSchema);
        }
        validate_id("batchId", &self.batch_id)?;
        validate_id("projectId", &self.project_id)?;
        validate_id("runId", &self.run_id)?;
        validate_digest_field("evaluationRecordDigest", &self.evaluation_record_digest)?;
        validate_digest_field("evidenceDigest", &self.evidence_digest)?;
        if self.findings.len() > RESEARCH_MAX_REVIEW_FINDINGS {
            return Err(ResearchContractError::InvalidField("findings"));
        }
        for pair in self.findings.windows(2) {
            if pair[0].finding_id >= pair[1].finding_id {
                return Err(ResearchContractError::InvalidField("findings"));
            }
        }
        for finding in &self.findings {
            finding.validate()?;
            if finding.project_id != self.project_id || finding.run_id != self.run_id {
                return Err(ResearchContractError::InvalidField("finding.identity"));
            }
            if finding.evaluation_record_digest.as_deref()
                != Some(self.evaluation_record_digest.as_str())
            {
                return Err(ResearchContractError::InvalidField(
                    "finding.evaluationRecordDigest",
                ));
            }
            if finding
                .evidence_digests
                .binary_search(&self.evidence_digest)
                .is_err()
            {
                return Err(ResearchContractError::InvalidField(
                    "finding.evidenceDigest",
                ));
            }
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, ResearchContractError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            batch_id: &'a str,
            project_id: &'a str,
            run_id: &'a str,
            evaluation_record_digest: &'a str,
            evidence_digest: &'a str,
            findings: &'a [ResearchReviewFindingV1],
        }
        digest(
            RESEARCH_REVIEW_BATCH_DIGEST_DOMAIN,
            &Identity {
                schema: &self.schema,
                batch_id: &self.batch_id,
                project_id: &self.project_id,
                run_id: &self.run_id,
                evaluation_record_digest: &self.evaluation_record_digest,
                evidence_digest: &self.evidence_digest,
                findings: &self.findings,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluation::{EvaluationRecordV1, EvaluationResultV1, ExecutionTargetV1};
    use crate::research::{
        ResearchReviewCategoryV1, ResearchReviewSeverityV1, ResearchReviewStatusV1,
    };

    fn digest(ch: char) -> String {
        format!("sha256:{}", ch.to_string().repeat(64))
    }

    fn finding(id: &str, record: &EvaluationRecordV1) -> ResearchReviewFindingV1 {
        ResearchReviewFindingV1::new(
            id,
            "project-1",
            "run-1",
            digest('a'),
            ResearchReviewCategoryV1::Citation,
            ResearchReviewSeverityV1::Warning,
            "citation needs review",
            None,
            vec![record.result.evidence_digest.clone()],
            record.result.evaluator_id.clone(),
            3,
        )
        .unwrap()
        .bind_evaluation_record(record)
        .unwrap()
    }

    fn record() -> EvaluationRecordV1 {
        EvaluationRecordV1::new(
            EvaluationResultV1::new(
                "reviewer",
                ExecutionTargetV1::new("session-1", "run-1"),
                "aux-1",
                "needs_review",
                serde_json::json!({"finding_count": 2}),
                digest('b'),
            )
            .unwrap(),
            2,
        )
        .unwrap()
    }

    #[test]
    fn batch_sorts_findings_and_keeps_human_decisions_explicit() {
        let record = record();
        let mut batch = ResearchReviewBatchV1::new(
            "batch-1",
            "project-1",
            "run-1",
            record.record_digest.clone(),
            record.result.evidence_digest.clone(),
            vec![finding("finding-2", &record), finding("finding-1", &record)],
        )
        .unwrap();
        assert_eq!(batch.findings[0].finding_id, "finding-1");
        assert!(batch
            .findings
            .iter()
            .all(|finding| finding.status == ResearchReviewStatusV1::Open));
        let before = batch.batch_digest.clone();
        batch.resolve_finding("finding-1", digest('c')).unwrap();
        assert_ne!(before, batch.batch_digest);
        assert!(batch.validate().is_ok());
    }

    #[test]
    fn batch_rejects_mixed_run_or_evidence_and_tampering() {
        let record = record();
        let other_run_record = EvaluationRecordV1::new(
            EvaluationResultV1::new(
                "reviewer",
                ExecutionTargetV1::new("session-2", "run-2"),
                "aux-2",
                "needs_review",
                serde_json::json!({"finding_count": 1}),
                digest('b'),
            )
            .unwrap(),
            2,
        )
        .unwrap();
        let mixed = ResearchReviewFindingV1::new(
            "finding-1",
            "project-1",
            "run-2",
            digest('a'),
            ResearchReviewCategoryV1::Citation,
            ResearchReviewSeverityV1::Warning,
            "citation needs review",
            None,
            vec![other_run_record.result.evidence_digest.clone()],
            "reviewer",
            3,
        )
        .unwrap()
        .bind_evaluation_record(&other_run_record)
        .unwrap();
        assert_eq!(
            ResearchReviewBatchV1::new(
                "batch-1",
                "project-1",
                "run-1",
                record.record_digest.clone(),
                record.result.evidence_digest.clone(),
                vec![mixed],
            ),
            Err(ResearchContractError::InvalidField("finding.identity"))
        );

        let mut other = finding("finding-1", &record);
        other.run_id = "run-2".to_owned();
        assert!(matches!(
            ResearchReviewBatchV1::new(
                "batch-1",
                "project-1",
                "run-1",
                record.record_digest.clone(),
                record.result.evidence_digest.clone(),
                vec![other],
            ),
            Err(ResearchContractError::DigestMismatch("findingDigest"))
        ));

        let mut batch = ResearchReviewBatchV1::new(
            "batch-1",
            "project-1",
            "run-1",
            record.record_digest.clone(),
            record.result.evidence_digest.clone(),
            vec![finding("finding-1", &record)],
        )
        .unwrap();
        batch.findings[0].message = "tampered".to_owned();
        assert_eq!(
            batch.validate(),
            Err(ResearchContractError::DigestMismatch("findingDigest"))
        );
    }

    #[test]
    fn empty_batch_represents_a_clean_review_result() {
        let record = record();
        let batch = ResearchReviewBatchV1::new(
            "clean-batch",
            "project-1",
            "run-1",
            record.record_digest.clone(),
            record.result.evidence_digest.clone(),
            Vec::new(),
        )
        .unwrap();

        assert!(batch.findings.is_empty());
        assert!(batch.validate().is_ok());
    }

    #[test]
    fn batch_round_trip_is_strict_and_tamper_evident() {
        let record = record();
        let batch = ResearchReviewBatchV1::new(
            "wire-batch",
            "project-1",
            "run-1",
            record.record_digest.clone(),
            record.result.evidence_digest.clone(),
            vec![finding("finding-1", &record)],
        )
        .unwrap();

        let encoded = serde_json::to_value(&batch).unwrap();
        let reopened: ResearchReviewBatchV1 = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(reopened, batch);
        assert!(reopened.validate().is_ok());

        let mut with_unknown_field = encoded;
        with_unknown_field["unexpected"] = serde_json::Value::Bool(true);
        assert!(serde_json::from_value::<ResearchReviewBatchV1>(with_unknown_field).is_err());

        let mut tampered = reopened;
        tampered.findings[0].message = "changed after publication".to_owned();
        assert_eq!(
            tampered.validate(),
            Err(ResearchContractError::DigestMismatch("findingDigest"))
        );
    }

    #[test]
    fn closed_batch_round_trip_preserves_terminal_finding_state() {
        let record = record();
        let mut resolved = ResearchReviewBatchV1::new(
            "resolved-wire-batch",
            "project-1",
            "run-1",
            record.record_digest.clone(),
            record.result.evidence_digest.clone(),
            vec![finding("finding-1", &record)],
        )
        .unwrap();
        resolved.resolve_finding("finding-1", digest('c')).unwrap();
        let reopened: ResearchReviewBatchV1 =
            serde_json::from_slice(&serde_json::to_vec(&resolved).unwrap()).unwrap();
        assert_eq!(reopened, resolved);
        assert_eq!(
            reopened.findings[0].status,
            ResearchReviewStatusV1::Resolved
        );
        assert!(reopened.validate().is_ok());
        assert_eq!(
            reopened.clone().resolve_finding("finding-1", digest('d')),
            Err(ResearchContractError::InvalidTransition {
                from: "resolved",
                to: "resolved"
            })
        );
        assert_eq!(
            reopened.clone().waive_finding("finding-1", digest('e')),
            Err(ResearchContractError::InvalidTransition {
                from: "resolved",
                to: "waived"
            })
        );

        let mut waived = ResearchReviewBatchV1::new(
            "waived-wire-batch",
            "project-1",
            "run-1",
            record.record_digest.clone(),
            record.result.evidence_digest.clone(),
            vec![finding("finding-2", &record)],
        )
        .unwrap();
        waived.waive_finding("finding-2", digest('f')).unwrap();
        let mut reopened: ResearchReviewBatchV1 =
            serde_json::from_slice(&serde_json::to_vec(&waived).unwrap()).unwrap();
        assert_eq!(reopened, waived);
        assert_eq!(reopened.findings[0].status, ResearchReviewStatusV1::Waived);
        assert!(reopened.validate().is_ok());
        assert_eq!(
            reopened.resolve_finding("finding-2", digest('1')),
            Err(ResearchContractError::InvalidTransition {
                from: "waived",
                to: "resolved"
            })
        );
    }
}
