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
        validate_text("location.anchor", &self.anchor, 512)
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
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, ResearchContractError> {
        #[derive(Serialize)]
        struct Identity<'a> {
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
        digest(
            RESEARCH_REVIEW_FINDING_DIGEST_DOMAIN,
            &Identity {
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
