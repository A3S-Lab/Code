use super::{
    digest, validate_digest_field, validate_id, validate_text, ResearchContractError,
    RESEARCH_MAX_DIGESTS, RESEARCH_MAX_TEXT_BYTES,
};
use serde::{Deserialize, Serialize};

pub const RESEARCH_REVIEW_FINDING_SCHEMA_V1: &str = "a3s.code.review-finding.v1";
const RESEARCH_REVIEW_FINDING_DIGEST_DOMAIN: &str = "a3s.code.review-finding.identity.v1";

/// Product-neutral classes that let a host render and route a finding.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchReviewCategoryV1 {
    Citation,
    Numeric,
    FigureCode,
    Method,
    Reproducibility,
    Source,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchReviewSeverityV1 {
    Info,
    Warning,
    Error,
    Blocker,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchReviewStatusV1 {
    Open,
    Resolved,
    Waived,
}

impl ResearchReviewStatusV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Resolved => "resolved",
            Self::Waived => "waived",
        }
    }
}

/// Bounded location of a reviewer observation in an artifact or source.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResearchReviewLocationV1 {
    pub anchor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

impl ResearchReviewLocationV1 {
    pub fn new(anchor: impl Into<String>) -> Result<Self, ResearchContractError> {
        let location = Self {
            anchor: anchor.into(),
            line: None,
            column: None,
        };
        validate_text("location.anchor", &location.anchor, 512)?;
        Ok(location)
    }

    pub fn with_line(mut self, line: u32, column: Option<u32>) -> Self {
        self.line = Some(line);
        self.column = column;
        self
    }

    fn validate(&self) -> Result<(), ResearchContractError> {
        validate_text("location.anchor", &self.anchor, 512)?;
        if self.line.is_some_and(|line| line == 0) {
            return Err(ResearchContractError::InvalidField("location.line"));
        }
        if self.column.is_some_and(|column| column == 0) {
            return Err(ResearchContractError::InvalidField("location.column"));
        }
        if self.column.is_some() && self.line.is_none() {
            return Err(ResearchContractError::InvalidField("location.column"));
        }
        Ok(())
    }
}

/// One host-produced scientific review observation bound to exact evidence.
///
/// Code validates identity and lifecycle shape only. A host or Use package
/// supplies the rubric, model, thresholds, and final approval decision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResearchReviewFindingV1 {
    pub schema: String,
    pub finding_id: String,
    pub project_id: String,
    pub run_id: String,
    pub artifact_digest: String,
    pub category: ResearchReviewCategoryV1,
    pub severity: ResearchReviewSeverityV1,
    pub status: ResearchReviewStatusV1,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<ResearchReviewLocationV1>,
    pub evidence_digests: Vec<String>,
    pub evaluator_id: String,
    /// Optional digest of the immutable generic evaluation record that
    /// produced this finding.  It is optional for compatibility with
    /// findings created before evaluator-result binding was available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluation_record_digest: Option<String>,
    /// Optional digest of the immutable provenance receipt for the reviewed
    /// artifact.  It is optional for compatibility with findings created
    /// before provenance binding was available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance_receipt_digest: Option<String>,
    pub observed_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_digest: Option<String>,
    pub finding_digest: String,
}

impl ResearchReviewFindingV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        finding_id: impl Into<String>,
        project_id: impl Into<String>,
        run_id: impl Into<String>,
        artifact_digest: impl Into<String>,
        category: ResearchReviewCategoryV1,
        severity: ResearchReviewSeverityV1,
        message: impl Into<String>,
        location: Option<ResearchReviewLocationV1>,
        mut evidence_digests: Vec<String>,
        evaluator_id: impl Into<String>,
        observed_at_ms: u64,
    ) -> Result<Self, ResearchContractError> {
        evidence_digests.sort();
        evidence_digests.dedup();
        let mut finding = Self {
            schema: RESEARCH_REVIEW_FINDING_SCHEMA_V1.to_owned(),
            finding_id: finding_id.into(),
            project_id: project_id.into(),
            run_id: run_id.into(),
            artifact_digest: artifact_digest.into(),
            category,
            severity,
            status: ResearchReviewStatusV1::Open,
            message: message.into(),
            location,
            evidence_digests,
            evaluator_id: evaluator_id.into(),
            evaluation_record_digest: None,
            provenance_receipt_digest: None,
            observed_at_ms,
            resolution_digest: None,
            finding_digest: String::new(),
        };
        finding.validate_without_digest()?;
        finding.finding_digest = finding.expected_digest()?;
        Ok(finding)
    }

    pub fn validate(&self) -> Result<(), ResearchContractError> {
        self.validate_without_digest()?;
        validate_digest_field("findingDigest", &self.finding_digest)?;
        if self.finding_digest != self.expected_digest()? {
            return Err(ResearchContractError::DigestMismatch("findingDigest"));
        }
        Ok(())
    }

    /// Bind this finding to the exact generic evaluation record that produced
    /// it.  The host still owns the rubric and finding projection, while Code
    /// verifies that the evaluator, Run, and evidence identity cannot drift.
    ///
    /// The method consumes and returns the finding so callers cannot observe a
    /// partially rebound value if validation fails.
    pub fn bind_evaluation_record(
        mut self,
        record: &crate::evaluation::EvaluationRecordV1,
    ) -> Result<Self, ResearchContractError> {
        self.validate()?;
        record
            .validate()
            .map_err(|_| ResearchContractError::InvalidField("evaluationRecord"))?;
        if record.result.target.run_id != self.run_id {
            return Err(ResearchContractError::InvalidField(
                "evaluationRecord.target",
            ));
        }
        if record.result.evaluator_id != self.evaluator_id {
            return Err(ResearchContractError::InvalidField(
                "evaluationRecord.evaluatorId",
            ));
        }
        if self
            .evidence_digests
            .binary_search(&record.result.evidence_digest)
            .is_err()
        {
            return Err(ResearchContractError::InvalidField(
                "evaluationRecord.evidenceDigest",
            ));
        }
        self.evaluation_record_digest = Some(record.record_digest.clone());
        self.finding_digest = self.expected_digest()?;
        self.validate()?;
        Ok(self)
    }

    /// Bind this finding to the exact provenance receipt for its artifact.
    ///
    /// A provenance receipt is host-produced, but Code can still reject an
    /// artifact/project/Run mismatch and require that the finding retain one
    /// of the receipt's input evidence digests.  This keeps reviewer policy
    /// outside Core while preventing a valid receipt from being attached to a
    /// different scientific object.
    pub fn bind_provenance_receipt(
        mut self,
        receipt: &crate::research::ResearchProvenanceReceiptV1,
    ) -> Result<Self, ResearchContractError> {
        self.validate()?;
        receipt
            .validate()
            .map_err(|_| ResearchContractError::InvalidField("provenanceReceipt"))?;
        if receipt.project_id != self.project_id {
            return Err(ResearchContractError::InvalidField(
                "provenanceReceipt.projectId",
            ));
        }
        if receipt.run_id != self.run_id {
            return Err(ResearchContractError::InvalidField(
                "provenanceReceipt.runId",
            ));
        }
        if receipt.artifact_digest != self.artifact_digest {
            return Err(ResearchContractError::InvalidField(
                "provenanceReceipt.artifactDigest",
            ));
        }
        if !receipt
            .input_digests
            .iter()
            .any(|digest| self.evidence_digests.binary_search(digest).is_ok())
        {
            return Err(ResearchContractError::InvalidField(
                "provenanceReceipt.inputDigests",
            ));
        }
        self.provenance_receipt_digest = Some(receipt.receipt_digest.clone());
        self.finding_digest = self.expected_digest()?;
        self.validate()?;
        Ok(self)
    }

    /// Bind this finding to a provenance receipt and the exact research Run
    /// admission that produced it.
    ///
    /// [`bind_provenance_receipt`](Self::bind_provenance_receipt) remains
    /// available for compatibility with callers that only have the finding
    /// and receipt.  New reviewer pipelines should pass the admitted Run as
    /// well so Code can reject a receipt from another project revision.
    pub fn bind_provenance_receipt_for_run(
        self,
        receipt: &crate::research::ResearchProvenanceReceiptV1,
        run: &crate::research::ResearchRunV1,
    ) -> Result<Self, ResearchContractError> {
        self.validate()?;
        run.validate()
            .map_err(|_| ResearchContractError::InvalidField("researchRun"))?;
        if run.project_id != self.project_id {
            return Err(ResearchContractError::InvalidField("researchRun.projectId"));
        }
        if run.run_id != self.run_id {
            return Err(ResearchContractError::InvalidField("researchRun.runId"));
        }
        receipt
            .validate()
            .map_err(|_| ResearchContractError::InvalidField("provenanceReceipt"))?;
        if receipt.project_revision != run.project_revision {
            return Err(ResearchContractError::InvalidField(
                "provenanceReceipt.projectRevision",
            ));
        }
        if receipt.provider_id != run.provider_id {
            return Err(ResearchContractError::InvalidField(
                "provenanceReceipt.providerId",
            ));
        }
        if receipt.random_seed != run.random_seed {
            return Err(ResearchContractError::InvalidField(
                "provenanceReceipt.randomSeed",
            ));
        }
        self.bind_provenance_receipt(receipt)
    }

    pub fn resolve(
        &mut self,
        resolution_digest: impl Into<String>,
    ) -> Result<(), ResearchContractError> {
        self.validate()?;
        if !matches!(self.status, ResearchReviewStatusV1::Open) {
            return Err(ResearchContractError::InvalidTransition {
                from: self.status.as_str(),
                to: ResearchReviewStatusV1::Resolved.as_str(),
            });
        }
        let resolution_digest = resolution_digest.into();
        validate_digest_field("resolutionDigest", &resolution_digest)?;
        self.status = ResearchReviewStatusV1::Resolved;
        self.resolution_digest = Some(resolution_digest);
        self.finding_digest = self.expected_digest()?;
        Ok(())
    }

    pub fn waive(
        &mut self,
        resolution_digest: impl Into<String>,
    ) -> Result<(), ResearchContractError> {
        self.validate()?;
        if !matches!(self.status, ResearchReviewStatusV1::Open) {
            return Err(ResearchContractError::InvalidTransition {
                from: self.status.as_str(),
                to: ResearchReviewStatusV1::Waived.as_str(),
            });
        }
        let resolution_digest = resolution_digest.into();
        validate_digest_field("resolutionDigest", &resolution_digest)?;
        self.status = ResearchReviewStatusV1::Waived;
        self.resolution_digest = Some(resolution_digest);
        self.finding_digest = self.expected_digest()?;
        Ok(())
    }

    fn validate_without_digest(&self) -> Result<(), ResearchContractError> {
        if self.schema != RESEARCH_REVIEW_FINDING_SCHEMA_V1 {
            return Err(ResearchContractError::UnsupportedSchema);
        }
        validate_id("findingId", &self.finding_id)?;
        validate_id("projectId", &self.project_id)?;
        validate_id("runId", &self.run_id)?;
        validate_digest_field("artifactDigest", &self.artifact_digest)?;
        validate_text("message", &self.message, RESEARCH_MAX_TEXT_BYTES)?;
        if let Some(location) = &self.location {
            location.validate()?;
        }
        if self.evidence_digests.is_empty() || self.evidence_digests.len() > RESEARCH_MAX_DIGESTS {
            return Err(ResearchContractError::InvalidField("evidenceDigests"));
        }
        for pair in self.evidence_digests.windows(2) {
            if pair[0] >= pair[1] {
                return Err(ResearchContractError::InvalidField("evidenceDigests"));
            }
        }
        for digest in &self.evidence_digests {
            validate_digest_field("evidenceDigests", digest)?;
        }
        validate_id("evaluatorId", &self.evaluator_id)?;
        if self.observed_at_ms == 0 {
            return Err(ResearchContractError::InvalidField("observedAtMs"));
        }
        if matches!(self.status, ResearchReviewStatusV1::Open) && self.resolution_digest.is_some() {
            return Err(ResearchContractError::InvalidField("resolutionDigest"));
        }
        if !matches!(self.status, ResearchReviewStatusV1::Open) && self.resolution_digest.is_none()
        {
            return Err(ResearchContractError::InvalidField("resolutionDigest"));
        }
        if let Some(resolution_digest) = &self.resolution_digest {
            validate_digest_field("resolutionDigest", resolution_digest)?;
        }
        if let Some(evaluation_record_digest) = &self.evaluation_record_digest {
            validate_digest_field("evaluationRecordDigest", evaluation_record_digest)?;
        }
        if let Some(provenance_receipt_digest) = &self.provenance_receipt_digest {
            validate_digest_field("provenanceReceiptDigest", provenance_receipt_digest)?;
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, ResearchContractError> {
        #[derive(Serialize)]
        struct LegacyIdentity<'a> {
            schema: &'a str,
            finding_id: &'a str,
            project_id: &'a str,
            run_id: &'a str,
            artifact_digest: &'a str,
            category: ResearchReviewCategoryV1,
            severity: ResearchReviewSeverityV1,
            status: ResearchReviewStatusV1,
            message: &'a str,
            location: &'a Option<ResearchReviewLocationV1>,
            evidence_digests: &'a [String],
            evaluator_id: &'a str,
            observed_at_ms: u64,
            resolution_digest: Option<&'a str>,
        }
        let Some(evaluation_record_digest) = self.evaluation_record_digest.as_deref() else {
            if let Some(provenance_receipt_digest) = self.provenance_receipt_digest.as_deref() {
                #[derive(Serialize)]
                struct ProvenanceBoundIdentity<'a> {
                    schema: &'a str,
                    finding_id: &'a str,
                    project_id: &'a str,
                    run_id: &'a str,
                    artifact_digest: &'a str,
                    category: ResearchReviewCategoryV1,
                    severity: ResearchReviewSeverityV1,
                    status: ResearchReviewStatusV1,
                    message: &'a str,
                    location: &'a Option<ResearchReviewLocationV1>,
                    evidence_digests: &'a [String],
                    evaluator_id: &'a str,
                    provenance_receipt_digest: &'a str,
                    observed_at_ms: u64,
                    resolution_digest: Option<&'a str>,
                }
                return digest(
                    RESEARCH_REVIEW_FINDING_DIGEST_DOMAIN,
                    &ProvenanceBoundIdentity {
                        schema: &self.schema,
                        finding_id: &self.finding_id,
                        project_id: &self.project_id,
                        run_id: &self.run_id,
                        artifact_digest: &self.artifact_digest,
                        category: self.category,
                        severity: self.severity,
                        status: self.status,
                        message: &self.message,
                        location: &self.location,
                        evidence_digests: &self.evidence_digests,
                        evaluator_id: &self.evaluator_id,
                        provenance_receipt_digest,
                        observed_at_ms: self.observed_at_ms,
                        resolution_digest: self.resolution_digest.as_deref(),
                    },
                );
            }
            return digest(
                RESEARCH_REVIEW_FINDING_DIGEST_DOMAIN,
                &LegacyIdentity {
                    schema: &self.schema,
                    finding_id: &self.finding_id,
                    project_id: &self.project_id,
                    run_id: &self.run_id,
                    artifact_digest: &self.artifact_digest,
                    category: self.category,
                    severity: self.severity,
                    status: self.status,
                    message: &self.message,
                    location: &self.location,
                    evidence_digests: &self.evidence_digests,
                    evaluator_id: &self.evaluator_id,
                    observed_at_ms: self.observed_at_ms,
                    resolution_digest: self.resolution_digest.as_deref(),
                },
            );
        };
        let Some(provenance_receipt_digest) = self.provenance_receipt_digest.as_deref() else {
            #[derive(Serialize)]
            struct BoundIdentity<'a> {
                schema: &'a str,
                finding_id: &'a str,
                project_id: &'a str,
                run_id: &'a str,
                artifact_digest: &'a str,
                category: ResearchReviewCategoryV1,
                severity: ResearchReviewSeverityV1,
                status: ResearchReviewStatusV1,
                message: &'a str,
                location: &'a Option<ResearchReviewLocationV1>,
                evidence_digests: &'a [String],
                evaluator_id: &'a str,
                evaluation_record_digest: &'a str,
                observed_at_ms: u64,
                resolution_digest: Option<&'a str>,
            }
            return digest(
                RESEARCH_REVIEW_FINDING_DIGEST_DOMAIN,
                &BoundIdentity {
                    schema: &self.schema,
                    finding_id: &self.finding_id,
                    project_id: &self.project_id,
                    run_id: &self.run_id,
                    artifact_digest: &self.artifact_digest,
                    category: self.category,
                    severity: self.severity,
                    status: self.status,
                    message: &self.message,
                    location: &self.location,
                    evidence_digests: &self.evidence_digests,
                    evaluator_id: &self.evaluator_id,
                    evaluation_record_digest,
                    observed_at_ms: self.observed_at_ms,
                    resolution_digest: self.resolution_digest.as_deref(),
                },
            );
        };

        #[derive(Serialize)]
        struct BoundIdentity<'a> {
            schema: &'a str,
            finding_id: &'a str,
            project_id: &'a str,
            run_id: &'a str,
            artifact_digest: &'a str,
            category: ResearchReviewCategoryV1,
            severity: ResearchReviewSeverityV1,
            status: ResearchReviewStatusV1,
            message: &'a str,
            location: &'a Option<ResearchReviewLocationV1>,
            evidence_digests: &'a [String],
            evaluator_id: &'a str,
            evaluation_record_digest: &'a str,
            provenance_receipt_digest: &'a str,
            observed_at_ms: u64,
            resolution_digest: Option<&'a str>,
        }
        digest(
            RESEARCH_REVIEW_FINDING_DIGEST_DOMAIN,
            &BoundIdentity {
                schema: &self.schema,
                finding_id: &self.finding_id,
                project_id: &self.project_id,
                run_id: &self.run_id,
                artifact_digest: &self.artifact_digest,
                category: self.category,
                severity: self.severity,
                status: self.status,
                message: &self.message,
                location: &self.location,
                evidence_digests: &self.evidence_digests,
                evaluator_id: &self.evaluator_id,
                evaluation_record_digest,
                provenance_receipt_digest,
                observed_at_ms: self.observed_at_ms,
                resolution_digest: self.resolution_digest.as_deref(),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(ch: char) -> String {
        format!("sha256:{}", ch.to_string().repeat(64))
    }

    #[test]
    fn finding_is_bound_to_evidence_and_resolution() {
        let mut finding = ResearchReviewFindingV1::new(
            "finding-1",
            "project-1",
            "run-1",
            digest('a'),
            ResearchReviewCategoryV1::Citation,
            ResearchReviewSeverityV1::Warning,
            "Citation does not support the claim.",
            Some(
                ResearchReviewLocationV1::new("report.md")
                    .unwrap()
                    .with_line(4, None),
            ),
            vec![digest('c'), digest('b')],
            "citation-reviewer",
            1,
        )
        .unwrap();
        assert_eq!(finding.evidence_digests, vec![digest('b'), digest('c')]);
        let open_digest = finding.finding_digest.clone();
        finding.resolve(digest('d')).unwrap();
        assert_ne!(open_digest, finding.finding_digest);
        assert!(finding.validate().is_ok());
    }

    #[test]
    fn finding_rejects_open_resolution_without_digest() {
        let mut finding = ResearchReviewFindingV1::new(
            "finding-1",
            "project-1",
            "run-1",
            digest('a'),
            ResearchReviewCategoryV1::Numeric,
            ResearchReviewSeverityV1::Error,
            "Numbers are not traceable.",
            None,
            vec![digest('b')],
            "numeric-reviewer",
            1,
        )
        .unwrap();
        finding.status = ResearchReviewStatusV1::Resolved;
        assert_eq!(
            finding.validate(),
            Err(ResearchContractError::InvalidField("resolutionDigest"))
        );
    }

    #[test]
    fn location_coordinates_are_one_based_and_line_bound() {
        let zero_line = ResearchReviewLocationV1::new("report.md")
            .unwrap()
            .with_line(0, None);
        assert_eq!(
            ResearchReviewFindingV1::new(
                "finding-zero-line",
                "project-1",
                "run-1",
                digest('a'),
                ResearchReviewCategoryV1::Citation,
                ResearchReviewSeverityV1::Warning,
                "invalid location",
                Some(zero_line),
                vec![digest('b')],
                "reviewer",
                1,
            ),
            Err(ResearchContractError::InvalidField("location.line"))
        );

        let orphaned_column = ResearchReviewLocationV1 {
            anchor: "report.md".to_owned(),
            line: None,
            column: Some(3),
        };
        assert_eq!(
            ResearchReviewFindingV1::new(
                "finding-orphaned-column",
                "project-1",
                "run-1",
                digest('a'),
                ResearchReviewCategoryV1::Citation,
                ResearchReviewSeverityV1::Warning,
                "invalid location",
                Some(orphaned_column),
                vec![digest('b')],
                "reviewer",
                1,
            ),
            Err(ResearchContractError::InvalidField("location.column"))
        );

        let zero_column = ResearchReviewLocationV1::new("report.md")
            .unwrap()
            .with_line(2, Some(0));
        assert_eq!(
            ResearchReviewFindingV1::new(
                "finding-zero-column",
                "project-1",
                "run-1",
                digest('a'),
                ResearchReviewCategoryV1::Citation,
                ResearchReviewSeverityV1::Warning,
                "invalid location",
                Some(zero_column),
                vec![digest('b')],
                "reviewer",
                1,
            ),
            Err(ResearchContractError::InvalidField("location.column"))
        );
    }

    #[test]
    fn finding_binds_the_exact_evaluation_record_without_importing_a_rubric() {
        let evidence_digest = digest('b');
        let result = crate::evaluation::EvaluationResultV1::new(
            "citation-reviewer",
            crate::evaluation::ExecutionTargetV1::new("session-1", "run-1"),
            "aux-1",
            "observed",
            serde_json::json!({"finding_count": 1}),
            evidence_digest.clone(),
        )
        .unwrap();
        let record = crate::evaluation::EvaluationRecordV1::new(result, 2).unwrap();
        let finding = ResearchReviewFindingV1::new(
            "finding-1",
            "project-1",
            "run-1",
            digest('a'),
            ResearchReviewCategoryV1::Citation,
            ResearchReviewSeverityV1::Warning,
            "Citation does not support the claim.",
            None,
            vec![evidence_digest],
            "citation-reviewer",
            3,
        )
        .unwrap()
        .bind_evaluation_record(&record)
        .unwrap();

        assert_eq!(
            finding.evaluation_record_digest.as_deref(),
            Some(record.record_digest.as_str())
        );
        assert!(finding.validate().is_ok());
        let mut tampered = finding;
        tampered.evaluation_record_digest = Some(digest('f'));
        assert_eq!(
            tampered.validate(),
            Err(ResearchContractError::DigestMismatch("findingDigest"))
        );
    }

    #[test]
    fn finding_binds_the_exact_artifact_provenance_without_importing_policy() {
        let evidence_digest = digest('b');
        let receipt = crate::research::ResearchProvenanceReceiptV1::new(
            "project-1",
            4,
            "run-1",
            "figure-1",
            crate::research::ResearchArtifactKindV1::Figure,
            digest('a'),
            vec![evidence_digest.clone(), digest('c')],
            digest('d'),
            digest('e'),
            digest('f'),
            "fixture-provider",
            Some(digest('1')),
            Some(7),
            Some(digest('2')),
        )
        .unwrap();
        let finding = ResearchReviewFindingV1::new(
            "finding-1",
            "project-1",
            "run-1",
            digest('a'),
            ResearchReviewCategoryV1::FigureCode,
            ResearchReviewSeverityV1::Warning,
            "Figure provenance must remain reproducible.",
            None,
            vec![evidence_digest],
            "reproducibility-reviewer",
            8,
        )
        .unwrap()
        .bind_provenance_receipt(&receipt)
        .unwrap();

        assert_eq!(
            finding.provenance_receipt_digest.as_deref(),
            Some(receipt.receipt_digest.as_str())
        );
        assert!(finding.validate().is_ok());
        let mut tampered = finding;
        tampered.provenance_receipt_digest = Some(digest('9'));
        assert_eq!(
            tampered.validate(),
            Err(ResearchContractError::DigestMismatch("findingDigest"))
        );
    }

    #[test]
    fn finding_rejects_provenance_from_another_artifact_or_evidence_window() {
        let receipt = crate::research::ResearchProvenanceReceiptV1::new(
            "project-1",
            4,
            "run-1",
            "figure-1",
            crate::research::ResearchArtifactKindV1::Figure,
            digest('a'),
            vec![digest('b')],
            digest('c'),
            digest('d'),
            digest('e'),
            "fixture-provider",
            None,
            None,
            None,
        )
        .unwrap();
        let other_artifact = ResearchReviewFindingV1::new(
            "finding-1",
            "project-1",
            "run-1",
            digest('f'),
            ResearchReviewCategoryV1::FigureCode,
            ResearchReviewSeverityV1::Warning,
            "wrong artifact",
            None,
            vec![digest('b')],
            "reviewer",
            8,
        )
        .unwrap();
        assert_eq!(
            other_artifact.bind_provenance_receipt(&receipt),
            Err(ResearchContractError::InvalidField(
                "provenanceReceipt.artifactDigest"
            ))
        );

        let other_evidence = ResearchReviewFindingV1::new(
            "finding-2",
            "project-1",
            "run-1",
            digest('a'),
            ResearchReviewCategoryV1::FigureCode,
            ResearchReviewSeverityV1::Warning,
            "wrong evidence",
            None,
            vec![digest('f')],
            "reviewer",
            8,
        )
        .unwrap();
        assert_eq!(
            other_evidence.bind_provenance_receipt(&receipt),
            Err(ResearchContractError::InvalidField(
                "provenanceReceipt.inputDigests"
            ))
        );
    }

    #[test]
    fn finding_rejects_a_record_from_another_run_or_evidence_window() {
        let record = crate::evaluation::EvaluationRecordV1::new(
            crate::evaluation::EvaluationResultV1::new(
                "numeric-reviewer",
                crate::evaluation::ExecutionTargetV1::new("session-1", "run-2"),
                "aux-2",
                "observed",
                serde_json::json!({"finding_count": 1}),
                digest('b'),
            )
            .unwrap(),
            2,
        )
        .unwrap();
        let finding = ResearchReviewFindingV1::new(
            "finding-1",
            "project-1",
            "run-1",
            digest('a'),
            ResearchReviewCategoryV1::Numeric,
            ResearchReviewSeverityV1::Error,
            "The value is not traceable.",
            None,
            vec![digest('b')],
            "numeric-reviewer",
            3,
        )
        .unwrap();
        assert_eq!(
            finding.clone().bind_evaluation_record(&record),
            Err(ResearchContractError::InvalidField(
                "evaluationRecord.target"
            ))
        );

        let same_run = crate::evaluation::EvaluationRecordV1::new(
            crate::evaluation::EvaluationResultV1::new(
                "numeric-reviewer",
                crate::evaluation::ExecutionTargetV1::new("session-1", "run-1"),
                "aux-3",
                "observed",
                serde_json::json!({"finding_count": 1}),
                digest('c'),
            )
            .unwrap(),
            2,
        )
        .unwrap();
        assert_eq!(
            finding.bind_evaluation_record(&same_run),
            Err(ResearchContractError::InvalidField(
                "evaluationRecord.evidenceDigest"
            ))
        );
    }

    #[test]
    fn resolution_rejects_a_tampered_finding_before_rebinding_identity() {
        let mut finding = ResearchReviewFindingV1::new(
            "finding-1",
            "project-1",
            "run-1",
            digest('a'),
            ResearchReviewCategoryV1::Method,
            ResearchReviewSeverityV1::Warning,
            "The method needs a bounded description.",
            None,
            vec![digest('b')],
            "method-reviewer",
            1,
        )
        .unwrap();
        finding.message = "tampered".to_owned();
        assert_eq!(
            finding.resolve(digest('c')),
            Err(ResearchContractError::DigestMismatch("findingDigest"))
        );
    }

    #[test]
    fn a_closed_finding_cannot_be_resolved_or_waived_again() {
        let mut finding = ResearchReviewFindingV1::new(
            "finding-1",
            "project-1",
            "run-1",
            digest('a'),
            ResearchReviewCategoryV1::Reproducibility,
            ResearchReviewSeverityV1::Blocker,
            "The environment receipt is missing.",
            None,
            vec![digest('b')],
            "reproducibility-reviewer",
            1,
        )
        .unwrap();
        finding.resolve(digest('c')).unwrap();
        assert_eq!(
            finding.resolve(digest('d')),
            Err(ResearchContractError::InvalidTransition {
                from: "resolved",
                to: "resolved"
            })
        );
        assert_eq!(
            finding.waive(digest('e')),
            Err(ResearchContractError::InvalidTransition {
                from: "resolved",
                to: "waived"
            })
        );
    }
}
