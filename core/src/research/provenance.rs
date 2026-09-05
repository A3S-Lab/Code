use super::{
    digest, validate_digest_field, validate_id, ResearchContractError, RESEARCH_MAX_DIGESTS,
};
use serde::{Deserialize, Serialize};

pub const RESEARCH_PROVENANCE_RECEIPT_SCHEMA_V1: &str = "a3s.code.provenance-receipt.v1";
const RESEARCH_PROVENANCE_RECEIPT_DIGEST_DOMAIN: &str = "a3s.code.provenance-receipt.identity.v1";

/// Artifact families that can be bound to a research provenance receipt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchArtifactKindV1 {
    Figure,
    Table,
    Dataset,
    Notebook,
    Model,
    Report,
    Other,
}

pub const RESEARCH_ARTIFACT_KINDS: &[ResearchArtifactKindV1] = &[
    ResearchArtifactKindV1::Figure,
    ResearchArtifactKindV1::Table,
    ResearchArtifactKindV1::Dataset,
    ResearchArtifactKindV1::Notebook,
    ResearchArtifactKindV1::Model,
    ResearchArtifactKindV1::Report,
    ResearchArtifactKindV1::Other,
];

/// Reproducibility identity for one generated artifact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResearchProvenanceReceiptV1 {
    pub schema: String,
    pub project_id: String,
    pub project_revision: u64,
    pub run_id: String,
    pub artifact_id: String,
    pub artifact_kind: ResearchArtifactKindV1,
    pub artifact_digest: String,
    pub input_digests: Vec<String>,
    pub workflow_digest: String,
    pub code_digest: String,
    pub environment_digest: String,
    pub provider_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub random_seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_digest: Option<String>,
    pub receipt_digest: String,
}

impl ResearchProvenanceReceiptV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        project_id: impl Into<String>,
        project_revision: u64,
        run_id: impl Into<String>,
        artifact_id: impl Into<String>,
        artifact_kind: ResearchArtifactKindV1,
        artifact_digest: impl Into<String>,
        mut input_digests: Vec<String>,
        workflow_digest: impl Into<String>,
        code_digest: impl Into<String>,
        environment_digest: impl Into<String>,
        provider_id: impl Into<String>,
        model_digest: Option<String>,
        random_seed: Option<u64>,
        validation_digest: Option<String>,
    ) -> Result<Self, ResearchContractError> {
        input_digests.sort();
        input_digests.dedup();
        let mut receipt = Self {
            schema: RESEARCH_PROVENANCE_RECEIPT_SCHEMA_V1.to_owned(),
            project_id: project_id.into(),
            project_revision,
            run_id: run_id.into(),
            artifact_id: artifact_id.into(),
            artifact_kind,
            artifact_digest: artifact_digest.into(),
            input_digests,
            workflow_digest: workflow_digest.into(),
            code_digest: code_digest.into(),
            environment_digest: environment_digest.into(),
            provider_id: provider_id.into(),
            model_digest,
            random_seed,
            validation_digest,
            receipt_digest: String::new(),
        };
        receipt.validate_without_digest()?;
        receipt.receipt_digest = receipt.expected_digest()?;
        Ok(receipt)
    }

    pub fn validate(&self) -> Result<(), ResearchContractError> {
        self.validate_without_digest()?;
        validate_digest_field("receiptDigest", &self.receipt_digest)?;
        if self.receipt_digest != self.expected_digest()? {
            return Err(ResearchContractError::DigestMismatch("receiptDigest"));
        }
        Ok(())
    }

    fn validate_without_digest(&self) -> Result<(), ResearchContractError> {
        if self.schema != RESEARCH_PROVENANCE_RECEIPT_SCHEMA_V1 {
            return Err(ResearchContractError::UnsupportedSchema);
        }
        validate_id("projectId", &self.project_id)?;
        if self.project_revision == 0 {
            return Err(ResearchContractError::InvalidField("projectRevision"));
        }
        validate_id("runId", &self.run_id)?;
        validate_id("artifactId", &self.artifact_id)?;
        validate_digest_field("artifactDigest", &self.artifact_digest)?;
        if self.input_digests.is_empty() || self.input_digests.len() > RESEARCH_MAX_DIGESTS {
            return Err(ResearchContractError::InvalidField("inputDigests"));
        }
        for pair in self.input_digests.windows(2) {
            if pair[0] >= pair[1] {
                return Err(ResearchContractError::InvalidField("inputDigests"));
            }
        }
        for digest in &self.input_digests {
            validate_digest_field("inputDigests", digest)?;
        }
        validate_digest_field("workflowDigest", &self.workflow_digest)?;
        validate_digest_field("codeDigest", &self.code_digest)?;
        validate_digest_field("environmentDigest", &self.environment_digest)?;
        validate_id("providerId", &self.provider_id)?;
        if let Some(model_digest) = &self.model_digest {
            validate_digest_field("modelDigest", model_digest)?;
        }
        if let Some(validation_digest) = &self.validation_digest {
            validate_digest_field("validationDigest", validation_digest)?;
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, ResearchContractError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            project_id: &'a str,
            project_revision: u64,
            run_id: &'a str,
            artifact_id: &'a str,
            artifact_kind: ResearchArtifactKindV1,
            artifact_digest: &'a str,
            input_digests: &'a [String],
            workflow_digest: &'a str,
            code_digest: &'a str,
            environment_digest: &'a str,
            provider_id: &'a str,
            model_digest: Option<&'a str>,
            random_seed: Option<u64>,
            validation_digest: Option<&'a str>,
        }
        digest(
            RESEARCH_PROVENANCE_RECEIPT_DIGEST_DOMAIN,
            &Identity {
                schema: &self.schema,
                project_id: &self.project_id,
                project_revision: self.project_revision,
                run_id: &self.run_id,
                artifact_id: &self.artifact_id,
                artifact_kind: self.artifact_kind,
                artifact_digest: &self.artifact_digest,
                input_digests: &self.input_digests,
                workflow_digest: &self.workflow_digest,
                code_digest: &self.code_digest,
                environment_digest: &self.environment_digest,
                provider_id: &self.provider_id,
                model_digest: self.model_digest.as_deref(),
                random_seed: self.random_seed,
                validation_digest: self.validation_digest.as_deref(),
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
    fn receipt_sorts_inputs_and_binds_environment_and_workflow() {
        let receipt = ResearchProvenanceReceiptV1::new(
            "project-1",
            1,
            "run-1",
            "artifact-1",
            ResearchArtifactKindV1::Figure,
            digest('a'),
            vec![digest('c'), digest('b'), digest('c')],
            digest('d'),
            digest('e'),
            digest('f'),
            "local",
            Some(digest('f')),
            Some(7),
            None,
        )
        .unwrap();
        assert_eq!(receipt.input_digests, vec![digest('b'), digest('c')]);
        assert!(receipt.validate().is_ok());
    }

    #[test]
    fn receipt_rejects_duplicate_or_unsorted_mutation() {
        let mut receipt = ResearchProvenanceReceiptV1::new(
            "project-1",
            1,
            "run-1",
            "artifact-1",
            ResearchArtifactKindV1::Report,
            digest('a'),
            vec![digest('b'), digest('c')],
            digest('d'),
            digest('e'),
            digest('f'),
            "local",
            None,
            None,
            None,
        )
        .unwrap();
        receipt.input_digests.reverse();
        assert_eq!(
            receipt.validate(),
            Err(ResearchContractError::InvalidField("inputDigests"))
        );
    }
}
