//! Digest-bound research claims with explicit support, conflict, and gap states.

use super::{
    digest, validate_digest_field, validate_id, ResearchContractError, RESEARCH_MAX_DIGESTS,
};
use serde::{Deserialize, Serialize};

pub const RESEARCH_CLAIM_SCHEMA_V1: &str = "a3s.code.research-claim.v1";
const RESEARCH_CLAIM_DIGEST_DOMAIN: &str = "a3s.code.research-claim.identity.v1";

/// Explicit claim lifecycle. Missing evidence never becomes an implicit success.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchClaimStatusV1 {
    Proposed,
    Supported,
    Conflicted,
    Unsupported,
}

impl ResearchClaimStatusV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Supported => "supported",
            Self::Conflicted => "conflicted",
            Self::Unsupported => "unsupported",
        }
    }

    pub const fn is_publication_ready(self) -> bool {
        matches!(self, Self::Supported | Self::Conflicted | Self::Unsupported)
    }
}

/// One content-addressed claim in a research evidence fabric.
///
/// The statement itself is never stored as plaintext. Hosts retain claim text
/// behind `statement_digest` and attach support, conflict, or gap digests
/// before publication. Code validates identity and state shape only.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResearchClaimV1 {
    pub schema: String,
    pub claim_id: String,
    pub project_id: String,
    pub run_id: String,
    pub statement_digest: String,
    pub status: ResearchClaimStatusV1,
    pub support_digests: Vec<String>,
    pub conflict_digests: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap_digest: Option<String>,
    pub observed_at_ms: u64,
    pub claim_digest: String,
}

impl ResearchClaimV1 {
    pub fn new(
        claim_id: impl Into<String>,
        project_id: impl Into<String>,
        run_id: impl Into<String>,
        statement_digest: impl Into<String>,
        observed_at_ms: u64,
    ) -> Result<Self, ResearchContractError> {
        let mut claim = Self {
            schema: RESEARCH_CLAIM_SCHEMA_V1.to_owned(),
            claim_id: claim_id.into(),
            project_id: project_id.into(),
            run_id: run_id.into(),
            statement_digest: statement_digest.into(),
            status: ResearchClaimStatusV1::Proposed,
            support_digests: Vec::new(),
            conflict_digests: Vec::new(),
            gap_digest: None,
            observed_at_ms,
            claim_digest: String::new(),
        };
        claim.validate_without_digest()?;
        claim.claim_digest = claim.expected_digest()?;
        Ok(claim)
    }

    /// Bind this claim to the admitted research Run namespace.
    pub fn validate_for_run(
        &self,
        run: &crate::research::ResearchRunV1,
    ) -> Result<(), ResearchContractError> {
        self.validate()?;
        if self.project_id != run.project_id {
            return Err(ResearchContractError::InvalidField("projectId"));
        }
        if self.run_id != run.run_id {
            return Err(ResearchContractError::InvalidField("runId"));
        }
        Ok(())
    }

    pub fn mark_supported(
        mut self,
        mut support_digests: Vec<String>,
    ) -> Result<Self, ResearchContractError> {
        if !matches!(
            self.status,
            ResearchClaimStatusV1::Proposed | ResearchClaimStatusV1::Supported
        ) {
            return Err(ResearchContractError::InvalidTransition {
                from: self.status.as_str(),
                to: ResearchClaimStatusV1::Supported.as_str(),
            });
        }
        support_digests.sort();
        support_digests.dedup();
        if support_digests.is_empty() {
            return Err(ResearchContractError::InvalidField("supportDigests"));
        }
        self.status = ResearchClaimStatusV1::Supported;
        self.support_digests = support_digests;
        self.conflict_digests.clear();
        self.gap_digest = None;
        self.claim_digest = self.expected_digest()?;
        self.validate()?;
        Ok(self)
    }

    pub fn mark_conflicted(
        mut self,
        mut conflict_digests: Vec<String>,
    ) -> Result<Self, ResearchContractError> {
        if !matches!(
            self.status,
            ResearchClaimStatusV1::Proposed | ResearchClaimStatusV1::Conflicted
        ) {
            return Err(ResearchContractError::InvalidTransition {
                from: self.status.as_str(),
                to: ResearchClaimStatusV1::Conflicted.as_str(),
            });
        }
        conflict_digests.sort();
        conflict_digests.dedup();
        if conflict_digests.is_empty() {
            return Err(ResearchContractError::InvalidField("conflictDigests"));
        }
        self.status = ResearchClaimStatusV1::Conflicted;
        self.conflict_digests = conflict_digests;
        self.support_digests.clear();
        self.gap_digest = None;
        self.claim_digest = self.expected_digest()?;
        self.validate()?;
        Ok(self)
    }

    pub fn mark_unsupported(
        mut self,
        gap_digest: impl Into<String>,
    ) -> Result<Self, ResearchContractError> {
        if !matches!(
            self.status,
            ResearchClaimStatusV1::Proposed | ResearchClaimStatusV1::Unsupported
        ) {
            return Err(ResearchContractError::InvalidTransition {
                from: self.status.as_str(),
                to: ResearchClaimStatusV1::Unsupported.as_str(),
            });
        }
        let gap_digest = gap_digest.into();
        validate_digest_field("gapDigest", &gap_digest)?;
        self.status = ResearchClaimStatusV1::Unsupported;
        self.gap_digest = Some(gap_digest);
        self.support_digests.clear();
        self.conflict_digests.clear();
        self.claim_digest = self.expected_digest()?;
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), ResearchContractError> {
        self.validate_without_digest()?;
        validate_digest_field("claimDigest", &self.claim_digest)?;
        if self.claim_digest != self.expected_digest()? {
            return Err(ResearchContractError::DigestMismatch("claimDigest"));
        }
        Ok(())
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, ResearchContractError> {
        let claim: Self = super::decode_json_slice(bytes)?;
        claim.validate()?;
        Ok(claim)
    }

    pub fn to_vec(&self) -> Result<Vec<u8>, ResearchContractError> {
        self.validate()?;
        super::encode_json(self)
    }

    fn validate_without_digest(&self) -> Result<(), ResearchContractError> {
        if self.schema != RESEARCH_CLAIM_SCHEMA_V1 {
            return Err(ResearchContractError::UnsupportedSchema);
        }
        validate_id("claimId", &self.claim_id)?;
        validate_id("projectId", &self.project_id)?;
        validate_id("runId", &self.run_id)?;
        validate_digest_field("statementDigest", &self.statement_digest)?;
        if self.observed_at_ms == 0 {
            return Err(ResearchContractError::InvalidField("observedAtMs"));
        }
        if self.support_digests.len() > RESEARCH_MAX_DIGESTS
            || self.conflict_digests.len() > RESEARCH_MAX_DIGESTS
        {
            return Err(ResearchContractError::InvalidField("supportDigests"));
        }
        validate_sorted_unique_digests("supportDigests", &self.support_digests)?;
        validate_sorted_unique_digests("conflictDigests", &self.conflict_digests)?;
        match self.status {
            ResearchClaimStatusV1::Proposed => {
                if !self.support_digests.is_empty()
                    || !self.conflict_digests.is_empty()
                    || self.gap_digest.is_some()
                {
                    return Err(ResearchContractError::InvalidField("status"));
                }
            }
            ResearchClaimStatusV1::Supported => {
                if self.support_digests.is_empty()
                    || !self.conflict_digests.is_empty()
                    || self.gap_digest.is_some()
                {
                    return Err(ResearchContractError::InvalidField("supportDigests"));
                }
            }
            ResearchClaimStatusV1::Conflicted => {
                if self.conflict_digests.is_empty()
                    || !self.support_digests.is_empty()
                    || self.gap_digest.is_some()
                {
                    return Err(ResearchContractError::InvalidField("conflictDigests"));
                }
            }
            ResearchClaimStatusV1::Unsupported => {
                let Some(gap_digest) = &self.gap_digest else {
                    return Err(ResearchContractError::InvalidField("gapDigest"));
                };
                validate_digest_field("gapDigest", gap_digest)?;
                if !self.support_digests.is_empty() || !self.conflict_digests.is_empty() {
                    return Err(ResearchContractError::InvalidField("gapDigest"));
                }
            }
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, ResearchContractError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            claim_id: &'a str,
            project_id: &'a str,
            run_id: &'a str,
            statement_digest: &'a str,
            status: ResearchClaimStatusV1,
            support_digests: &'a [String],
            conflict_digests: &'a [String],
            gap_digest: Option<&'a str>,
            observed_at_ms: u64,
        }
        digest(
            RESEARCH_CLAIM_DIGEST_DOMAIN,
            &Identity {
                schema: &self.schema,
                claim_id: &self.claim_id,
                project_id: &self.project_id,
                run_id: &self.run_id,
                statement_digest: &self.statement_digest,
                status: self.status,
                support_digests: &self.support_digests,
                conflict_digests: &self.conflict_digests,
                gap_digest: self.gap_digest.as_deref(),
                observed_at_ms: self.observed_at_ms,
            },
        )
    }
}

fn validate_sorted_unique_digests(
    field: &'static str,
    digests: &[String],
) -> Result<(), ResearchContractError> {
    for pair in digests.windows(2) {
        if pair[0] >= pair[1] {
            return Err(ResearchContractError::InvalidField(field));
        }
    }
    for value in digests {
        validate_digest_field(field, value)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(ch: char) -> String {
        format!("sha256:{}", ch.to_string().repeat(64))
    }

    #[test]
    fn proposed_claim_is_digest_bound_and_wire_safe() {
        let claim = ResearchClaimV1::new("claim-1", "project-1", "run-1", digest('a'), 1).unwrap();
        assert_eq!(claim.status, ResearchClaimStatusV1::Proposed);
        let encoded = claim.to_vec().unwrap();
        assert_eq!(ResearchClaimV1::from_slice(&encoded).unwrap(), claim);
    }

    #[test]
    fn supported_claim_requires_support_digests_and_rejects_illegal_transitions() {
        let claim = ResearchClaimV1::new("claim-1", "project-1", "run-1", digest('a'), 1).unwrap();
        let supported = claim
            .clone()
            .mark_supported(vec![digest('c'), digest('b'), digest('c')])
            .unwrap();
        assert_eq!(supported.status, ResearchClaimStatusV1::Supported);
        assert_eq!(supported.support_digests, vec![digest('b'), digest('c')]);
        assert!(matches!(
            supported.clone().mark_conflicted(vec![digest('d')]),
            Err(ResearchContractError::InvalidTransition { .. })
        ));
        assert!(matches!(
            ResearchClaimV1::new("claim-1", "project-1", "run-1", digest('a'), 1)
                .unwrap()
                .mark_supported(Vec::new()),
            Err(ResearchContractError::InvalidField("supportDigests"))
        ));
    }

    #[test]
    fn unsupported_claim_requires_an_explicit_gap_digest() {
        let claim = ResearchClaimV1::new("claim-1", "project-1", "run-1", digest('a'), 1)
            .unwrap()
            .mark_unsupported(digest('e'))
            .unwrap();
        assert_eq!(claim.status, ResearchClaimStatusV1::Unsupported);
        assert_eq!(claim.gap_digest.as_deref(), Some(digest('e').as_str()));
        let mut tampered = claim.clone();
        tampered.gap_digest = Some(digest('f'));
        assert!(matches!(
            tampered.validate(),
            Err(ResearchContractError::DigestMismatch("claimDigest"))
        ));
    }
}
