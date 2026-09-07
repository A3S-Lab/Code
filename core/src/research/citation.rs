//! Digest-bound citations that link claims to source-span evidence.

use super::{
    digest, validate_digest_field, validate_id, ResearchContractError, RESEARCH_MAX_TEXT_BYTES,
};
use serde::{Deserialize, Serialize};

pub const RESEARCH_CITATION_SCHEMA_V1: &str = "a3s.code.research-citation.v1";
const RESEARCH_CITATION_DIGEST_DOMAIN: &str = "a3s.code.research-citation.identity.v1";

/// One bounded citation from a claim to a source span or evidence fact.
///
/// Citations carry digests only. Hosts retain source text and locators behind
/// those digests and optional bounded locator text for UI routing.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResearchCitationV1 {
    pub schema: String,
    pub citation_id: String,
    pub project_id: String,
    pub run_id: String,
    pub claim_id: String,
    pub source_digest: String,
    pub source_span_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
    pub observed_at_ms: u64,
    pub citation_digest: String,
}

impl ResearchCitationV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        citation_id: impl Into<String>,
        project_id: impl Into<String>,
        run_id: impl Into<String>,
        claim_id: impl Into<String>,
        source_digest: impl Into<String>,
        source_span_digest: impl Into<String>,
        locator: Option<String>,
        observed_at_ms: u64,
    ) -> Result<Self, ResearchContractError> {
        let mut citation = Self {
            schema: RESEARCH_CITATION_SCHEMA_V1.to_owned(),
            citation_id: citation_id.into(),
            project_id: project_id.into(),
            run_id: run_id.into(),
            claim_id: claim_id.into(),
            source_digest: source_digest.into(),
            source_span_digest: source_span_digest.into(),
            locator,
            observed_at_ms,
            citation_digest: String::new(),
        };
        citation.validate_without_digest()?;
        citation.citation_digest = citation.expected_digest()?;
        Ok(citation)
    }

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

    pub fn validate(&self) -> Result<(), ResearchContractError> {
        self.validate_without_digest()?;
        validate_digest_field("citationDigest", &self.citation_digest)?;
        if self.citation_digest != self.expected_digest()? {
            return Err(ResearchContractError::DigestMismatch("citationDigest"));
        }
        Ok(())
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, ResearchContractError> {
        let citation: Self = super::decode_json_slice(bytes)?;
        citation.validate()?;
        Ok(citation)
    }

    pub fn to_vec(&self) -> Result<Vec<u8>, ResearchContractError> {
        self.validate()?;
        super::encode_json(self)
    }

    fn validate_without_digest(&self) -> Result<(), ResearchContractError> {
        if self.schema != RESEARCH_CITATION_SCHEMA_V1 {
            return Err(ResearchContractError::UnsupportedSchema);
        }
        validate_id("citationId", &self.citation_id)?;
        validate_id("projectId", &self.project_id)?;
        validate_id("runId", &self.run_id)?;
        validate_id("claimId", &self.claim_id)?;
        validate_digest_field("sourceDigest", &self.source_digest)?;
        validate_digest_field("sourceSpanDigest", &self.source_span_digest)?;
        if let Some(locator) = &self.locator {
            super::validate_text("locator", locator, RESEARCH_MAX_TEXT_BYTES)?;
        }
        if self.observed_at_ms == 0 {
            return Err(ResearchContractError::InvalidField("observedAtMs"));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, ResearchContractError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            citation_id: &'a str,
            project_id: &'a str,
            run_id: &'a str,
            claim_id: &'a str,
            source_digest: &'a str,
            source_span_digest: &'a str,
            locator: Option<&'a str>,
            observed_at_ms: u64,
        }
        digest(
            RESEARCH_CITATION_DIGEST_DOMAIN,
            &Identity {
                schema: &self.schema,
                citation_id: &self.citation_id,
                project_id: &self.project_id,
                run_id: &self.run_id,
                claim_id: &self.claim_id,
                source_digest: &self.source_digest,
                source_span_digest: &self.source_span_digest,
                locator: self.locator.as_deref(),
                observed_at_ms: self.observed_at_ms,
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
    fn citation_is_digest_bound_and_rejects_multiline_locator() {
        let citation = ResearchCitationV1::new(
            "cite-1",
            "project-1",
            "run-1",
            "claim-1",
            digest('a'),
            digest('b'),
            Some("page-3".to_owned()),
            1,
        )
        .unwrap();
        let encoded = citation.to_vec().unwrap();
        assert_eq!(ResearchCitationV1::from_slice(&encoded).unwrap(), citation);
        assert!(matches!(
            ResearchCitationV1::new(
                "cite-1",
                "project-1",
                "run-1",
                "claim-1",
                digest('a'),
                digest('b'),
                Some("page-3\n".to_owned()),
                1,
            ),
            Err(ResearchContractError::InvalidField("locator"))
        ));
    }
}
