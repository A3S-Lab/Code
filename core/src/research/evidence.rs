use super::{digest, validate_digest_field, validate_id, validate_text, ResearchContractError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const RESEARCH_EVIDENCE_FACT_SCHEMA_V1: &str = "a3s.code.evidence-fact.v1";
const RESEARCH_EVIDENCE_FACT_DIGEST_DOMAIN: &str = "a3s.code.evidence-fact.identity.v1";
pub const RESEARCH_MAX_FACT_METADATA: usize = 32;
pub const RESEARCH_MAX_METADATA_VALUE_BYTES: usize = 1024;

/// Stable categories for digest-only research observations.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchEvidenceFactKindV1 {
    Source,
    SourceSpan,
    Claim,
    Citation,
    Measurement,
    Derivation,
    Artifact,
    Validation,
    Review,
}

/// One append-only, bounded observation in a research evidence ledger.
///
/// The value contains references and metadata, never the source text or raw
/// model/tool payload. A host may resolve the digests through its own artifact
/// store after checking the associated authority and project revision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResearchEvidenceFactV1 {
    pub schema: String,
    pub run_id: String,
    pub sequence: u64,
    pub kind: ResearchEvidenceFactKindV1,
    pub subject_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<String>,
    pub content_digest: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    pub observed_at_ms: u64,
    pub fact_digest: String,
}

impl ResearchEvidenceFactV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        run_id: impl Into<String>,
        sequence: u64,
        kind: ResearchEvidenceFactKindV1,
        subject_id: impl Into<String>,
        source_digest: Option<String>,
        content_digest: impl Into<String>,
        metadata: BTreeMap<String, String>,
        observed_at_ms: u64,
    ) -> Result<Self, ResearchContractError> {
        let mut fact = Self {
            schema: RESEARCH_EVIDENCE_FACT_SCHEMA_V1.to_owned(),
            run_id: run_id.into(),
            sequence,
            kind,
            subject_id: subject_id.into(),
            source_digest,
            content_digest: content_digest.into(),
            metadata,
            observed_at_ms,
            fact_digest: String::new(),
        };
        fact.validate_without_digest()?;
        fact.fact_digest = fact.expected_digest()?;
        Ok(fact)
    }

    pub fn validate(&self) -> Result<(), ResearchContractError> {
        self.validate_without_digest()?;
        validate_digest_field("factDigest", &self.fact_digest)?;
        if self.fact_digest != self.expected_digest()? {
            return Err(ResearchContractError::DigestMismatch("factDigest"));
        }
        Ok(())
    }

    /// Decode a bounded JSON evidence fact and validate its identity before
    /// returning it to a caller at a process boundary.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, ResearchContractError> {
        let fact: Self = super::decode_json_slice(bytes)?;
        fact.validate()?;
        Ok(fact)
    }

    /// Encode a validated evidence fact for a process boundary.
    pub fn to_vec(&self) -> Result<Vec<u8>, ResearchContractError> {
        self.validate()?;
        super::encode_json(self)
    }

    fn validate_without_digest(&self) -> Result<(), ResearchContractError> {
        if self.schema != RESEARCH_EVIDENCE_FACT_SCHEMA_V1 {
            return Err(ResearchContractError::UnsupportedSchema);
        }
        validate_id("runId", &self.run_id)?;
        if self.sequence == 0 {
            return Err(ResearchContractError::InvalidField("sequence"));
        }
        validate_id("subjectId", &self.subject_id)?;
        if let Some(source_digest) = &self.source_digest {
            validate_digest_field("sourceDigest", source_digest)?;
        }
        validate_digest_field("contentDigest", &self.content_digest)?;
        if self.metadata.len() > RESEARCH_MAX_FACT_METADATA {
            return Err(ResearchContractError::InvalidField("metadata"));
        }
        for (key, value) in &self.metadata {
            validate_id("metadata.key", key)?;
            validate_text("metadata.value", value, RESEARCH_MAX_METADATA_VALUE_BYTES)?;
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
            run_id: &'a str,
            sequence: u64,
            kind: ResearchEvidenceFactKindV1,
            subject_id: &'a str,
            source_digest: Option<&'a str>,
            content_digest: &'a str,
            metadata: &'a BTreeMap<String, String>,
            observed_at_ms: u64,
        }
        digest(
            RESEARCH_EVIDENCE_FACT_DIGEST_DOMAIN,
            &Identity {
                schema: &self.schema,
                run_id: &self.run_id,
                sequence: self.sequence,
                kind: self.kind,
                subject_id: &self.subject_id,
                source_digest: self.source_digest.as_deref(),
                content_digest: &self.content_digest,
                metadata: &self.metadata,
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
    fn fact_is_digest_bound_and_metadata_is_canonical() {
        let mut metadata = BTreeMap::new();
        metadata.insert("locator".to_owned(), "page-3".to_owned());
        let fact = ResearchEvidenceFactV1::new(
            "run-1",
            1,
            ResearchEvidenceFactKindV1::Claim,
            "claim-1",
            Some(digest('a')),
            digest('b'),
            metadata,
            1,
        )
        .unwrap();
        assert!(fact.validate().is_ok());
        let encoded = fact.to_vec().unwrap();
        assert_eq!(ResearchEvidenceFactV1::from_slice(&encoded).unwrap(), fact);
        let mut tampered = fact.clone();
        tampered.metadata.insert("extra".to_owned(), "x".to_owned());
        assert!(matches!(
            tampered.validate(),
            Err(ResearchContractError::DigestMismatch("factDigest"))
        ));
    }

    #[test]
    fn fact_rejects_zero_sequence_and_raw_multiline_metadata() {
        let error = ResearchEvidenceFactV1::new(
            "run-1",
            0,
            ResearchEvidenceFactKindV1::Source,
            "source-1",
            None,
            digest('a'),
            BTreeMap::new(),
            1,
        )
        .unwrap_err();
        assert_eq!(error, ResearchContractError::InvalidField("sequence"));

        let mut metadata = BTreeMap::new();
        metadata.insert("note".to_owned(), "line one\nline two".to_owned());
        let error = ResearchEvidenceFactV1::new(
            "run-1",
            1,
            ResearchEvidenceFactKindV1::Source,
            "source-1",
            None,
            digest('a'),
            metadata,
            1,
        )
        .unwrap_err();
        assert_eq!(error, ResearchContractError::InvalidField("metadata.value"));
    }
}
