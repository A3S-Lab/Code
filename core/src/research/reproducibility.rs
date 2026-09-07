//! Typed reproducibility manifest for one research Run.

use super::{
    digest, validate_digest_field, validate_id, ResearchContractError, ResearchProvenanceReceiptV1,
    ResearchReproducibilityV1, ResearchRunV1, RESEARCH_MAX_DIGESTS,
};
use serde::{Deserialize, Serialize};

pub const RESEARCH_REPRODUCIBILITY_MANIFEST_SCHEMA_V1: &str =
    "a3s.code.reproducibility-manifest.v1";
const RESEARCH_REPRODUCIBILITY_MANIFEST_DIGEST_DOMAIN: &str =
    "a3s.code.reproducibility-manifest.identity.v1";

/// Digests and non-secret parameters that make one research Run reproducible.
///
/// The manifest never stores prompts, credentials, or raw provider payloads.
/// Hosts retain those behind digests. Code only fences identity drift against
/// the admitted Run and optional provenance receipts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResearchReproducibilityManifestV1 {
    pub schema: String,
    pub manifest_id: String,
    pub project_id: String,
    pub run_id: String,
    pub provider_id: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_revision_digest: Option<String>,
    pub environment_lock_digest: String,
    pub code_digest: String,
    pub workflow_digest: String,
    pub parameter_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub random_seed: Option<u64>,
    pub tolerance_digests: Vec<String>,
    pub output_artifact_digests: Vec<String>,
    pub manifest_digest: String,
}

impl ResearchReproducibilityManifestV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new_for_run(
        manifest_id: impl Into<String>,
        run: &ResearchRunV1,
        model_revision_digest: Option<String>,
        environment_lock_digest: impl Into<String>,
        code_digest: impl Into<String>,
        workflow_digest: impl Into<String>,
        parameter_digest: impl Into<String>,
        mut tolerance_digests: Vec<String>,
        mut output_artifact_digests: Vec<String>,
    ) -> Result<Self, ResearchContractError> {
        tolerance_digests.sort();
        tolerance_digests.dedup();
        output_artifact_digests.sort();
        output_artifact_digests.dedup();
        let mut manifest = Self {
            schema: RESEARCH_REPRODUCIBILITY_MANIFEST_SCHEMA_V1.to_owned(),
            manifest_id: manifest_id.into(),
            project_id: run.project_id.clone(),
            run_id: run.run_id.clone(),
            provider_id: run.provider_id.clone(),
            model_id: run.model_id.clone(),
            model_revision_digest,
            environment_lock_digest: environment_lock_digest.into(),
            code_digest: code_digest.into(),
            workflow_digest: workflow_digest.into(),
            parameter_digest: parameter_digest.into(),
            random_seed: run.random_seed,
            tolerance_digests,
            output_artifact_digests,
            manifest_digest: String::new(),
        };
        manifest.validate_without_digest()?;
        manifest.validate_against_run(run)?;
        manifest.manifest_digest = manifest.expected_digest()?;
        Ok(manifest)
    }

    pub fn validate_for_run(&self, run: &ResearchRunV1) -> Result<(), ResearchContractError> {
        self.validate()?;
        self.validate_against_run(run)
    }

    /// Require every output artifact in this manifest to appear in the supplied
    /// provenance receipts for the same project/Run, with matching environment
    /// and workflow digests.
    pub fn validate_against_provenance(
        &self,
        receipts: &[ResearchProvenanceReceiptV1],
    ) -> Result<(), ResearchContractError> {
        self.validate()?;
        if self.output_artifact_digests.is_empty() {
            return Err(ResearchContractError::InvalidField("outputArtifactDigests"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for receipt in receipts {
            receipt.validate()?;
            if receipt.project_id != self.project_id {
                return Err(ResearchContractError::InvalidField("receipt.projectId"));
            }
            if receipt.run_id != self.run_id {
                return Err(ResearchContractError::InvalidField("receipt.runId"));
            }
            if receipt.environment_digest != self.environment_lock_digest {
                return Err(ResearchContractError::InvalidField("environmentLockDigest"));
            }
            if receipt.workflow_digest != self.workflow_digest {
                return Err(ResearchContractError::InvalidField("workflowDigest"));
            }
            if receipt.code_digest != self.code_digest {
                return Err(ResearchContractError::InvalidField("codeDigest"));
            }
            if receipt.provider_id != self.provider_id {
                return Err(ResearchContractError::InvalidField("providerId"));
            }
            if receipt.random_seed != self.random_seed {
                return Err(ResearchContractError::InvalidField("randomSeed"));
            }
            if !self
                .output_artifact_digests
                .iter()
                .any(|digest| digest == &receipt.artifact_digest)
            {
                return Err(ResearchContractError::InvalidField("artifactDigest"));
            }
            if !seen.insert(receipt.artifact_digest.as_str()) {
                return Err(ResearchContractError::InvalidField("artifactDigest"));
            }
        }
        for output in &self.output_artifact_digests {
            if !seen.contains(output.as_str()) {
                return Err(ResearchContractError::InvalidField("outputArtifactDigests"));
            }
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), ResearchContractError> {
        self.validate_without_digest()?;
        validate_digest_field("manifestDigest", &self.manifest_digest)?;
        if self.manifest_digest != self.expected_digest()? {
            return Err(ResearchContractError::DigestMismatch("manifestDigest"));
        }
        Ok(())
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, ResearchContractError> {
        let manifest: Self = super::decode_json_slice(bytes)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn to_vec(&self) -> Result<Vec<u8>, ResearchContractError> {
        self.validate()?;
        super::encode_json(self)
    }

    fn validate_against_run(&self, run: &ResearchRunV1) -> Result<(), ResearchContractError> {
        if self.project_id != run.project_id {
            return Err(ResearchContractError::InvalidField("projectId"));
        }
        if self.run_id != run.run_id {
            return Err(ResearchContractError::InvalidField("runId"));
        }
        if self.provider_id != run.provider_id {
            return Err(ResearchContractError::InvalidField("providerId"));
        }
        if self.model_id != run.model_id {
            return Err(ResearchContractError::InvalidField("modelId"));
        }
        if self.random_seed != run.random_seed {
            return Err(ResearchContractError::InvalidField("randomSeed"));
        }
        if matches!(
            run.reproducibility,
            ResearchReproducibilityV1::Deterministic
        ) && self.random_seed.is_none()
        {
            return Err(ResearchContractError::InvalidField("randomSeed"));
        }
        Ok(())
    }

    fn validate_without_digest(&self) -> Result<(), ResearchContractError> {
        if self.schema != RESEARCH_REPRODUCIBILITY_MANIFEST_SCHEMA_V1 {
            return Err(ResearchContractError::UnsupportedSchema);
        }
        validate_id("manifestId", &self.manifest_id)?;
        validate_id("projectId", &self.project_id)?;
        validate_id("runId", &self.run_id)?;
        validate_id("providerId", &self.provider_id)?;
        validate_id("modelId", &self.model_id)?;
        if let Some(model_revision_digest) = &self.model_revision_digest {
            validate_digest_field("modelRevisionDigest", model_revision_digest)?;
        }
        validate_digest_field("environmentLockDigest", &self.environment_lock_digest)?;
        validate_digest_field("codeDigest", &self.code_digest)?;
        validate_digest_field("workflowDigest", &self.workflow_digest)?;
        validate_digest_field("parameterDigest", &self.parameter_digest)?;
        if self.tolerance_digests.len() > RESEARCH_MAX_DIGESTS {
            return Err(ResearchContractError::InvalidField("toleranceDigests"));
        }
        if self.output_artifact_digests.len() > RESEARCH_MAX_DIGESTS {
            return Err(ResearchContractError::InvalidField("outputArtifactDigests"));
        }
        validate_sorted_unique_digests("toleranceDigests", &self.tolerance_digests)?;
        validate_sorted_unique_digests("outputArtifactDigests", &self.output_artifact_digests)?;
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, ResearchContractError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            manifest_id: &'a str,
            project_id: &'a str,
            run_id: &'a str,
            provider_id: &'a str,
            model_id: &'a str,
            model_revision_digest: Option<&'a str>,
            environment_lock_digest: &'a str,
            code_digest: &'a str,
            workflow_digest: &'a str,
            parameter_digest: &'a str,
            random_seed: Option<u64>,
            tolerance_digests: &'a [String],
            output_artifact_digests: &'a [String],
        }
        digest(
            RESEARCH_REPRODUCIBILITY_MANIFEST_DIGEST_DOMAIN,
            &Identity {
                schema: &self.schema,
                manifest_id: &self.manifest_id,
                project_id: &self.project_id,
                run_id: &self.run_id,
                provider_id: &self.provider_id,
                model_id: &self.model_id,
                model_revision_digest: self.model_revision_digest.as_deref(),
                environment_lock_digest: &self.environment_lock_digest,
                code_digest: &self.code_digest,
                workflow_digest: &self.workflow_digest,
                parameter_digest: &self.parameter_digest,
                random_seed: self.random_seed,
                tolerance_digests: &self.tolerance_digests,
                output_artifact_digests: &self.output_artifact_digests,
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
    use crate::capability::{
        CapabilityCeiling, CapabilityContribution, CapabilityDescriptor,
        CapabilityExecutionCeiling, CapabilityKind, CapabilitySet, CapabilitySource,
        CodeCatalogGeneration, GovernanceCapabilityCeiling, RunCapabilityBindingV1, Sha256Digest,
        WorkspaceCapabilityCeiling,
    };
    use crate::research::{ResearchArtifactKindV1, ResearchRunStatusV1};

    fn digest(ch: char) -> String {
        format!("sha256:{}", ch.to_string().repeat(64))
    }

    fn binding() -> RunCapabilityBindingV1 {
        let source =
            CapabilitySource::builtin("test", Sha256Digest::new(digest('c')).unwrap()).unwrap();
        let descriptor = CapabilityDescriptor::new(
            &source,
            CapabilityKind::Tool,
            "tool",
            "tool",
            Sha256Digest::new(digest('d')).unwrap(),
            [],
        )
        .unwrap();
        let contribution = CapabilityContribution::new(source, [descriptor]).unwrap();
        let set = CapabilitySet::from_contributions(CodeCatalogGeneration::new(1), [contribution])
            .unwrap();
        let ceiling = CapabilityCeiling::all(
            &set,
            WorkspaceCapabilityCeiling::default(),
            GovernanceCapabilityCeiling::default(),
            CapabilityExecutionCeiling::new(1, 1, None, None, None).unwrap(),
        )
        .unwrap();
        RunCapabilityBindingV1::from_set_and_ceiling(&set, &ceiling).unwrap()
    }

    fn admitted_run() -> ResearchRunV1 {
        let mut run = ResearchRunV1::new(
            "run-1",
            "project-1",
            1,
            digest('1'),
            digest('2'),
            binding(),
            "provider-1",
            "model-1",
            ResearchReproducibilityV1::Deterministic,
            Some(7),
        )
        .unwrap();
        run.transition_to(ResearchRunStatusV1::Admitted).unwrap();
        run
    }

    #[test]
    fn manifest_binds_run_and_provenance_outputs() {
        let run = admitted_run();
        let manifest = ResearchReproducibilityManifestV1::new_for_run(
            "manifest-1",
            &run,
            Some(digest('3')),
            digest('4'),
            digest('5'),
            digest('6'),
            digest('7'),
            vec![digest('9'), digest('8')],
            vec![digest('a'), digest('b')],
        )
        .unwrap();
        assert_eq!(manifest.random_seed, Some(7));
        assert_eq!(manifest.tolerance_digests, vec![digest('8'), digest('9')]);
        let receipt_a = ResearchProvenanceReceiptV1::new(
            "project-1",
            1,
            "run-1",
            "artifact-a",
            ResearchArtifactKindV1::Figure,
            digest('a'),
            vec![digest('2')],
            digest('6'),
            digest('5'),
            digest('4'),
            "provider-1",
            Some(digest('3')),
            Some(7),
            None,
        )
        .unwrap();
        let receipt_b = ResearchProvenanceReceiptV1::new(
            "project-1",
            1,
            "run-1",
            "artifact-b",
            ResearchArtifactKindV1::Report,
            digest('b'),
            vec![digest('2')],
            digest('6'),
            digest('5'),
            digest('4'),
            "provider-1",
            Some(digest('3')),
            Some(7),
            None,
        )
        .unwrap();
        manifest
            .validate_against_provenance(&[receipt_a, receipt_b])
            .unwrap();
        let encoded = manifest.to_vec().unwrap();
        assert_eq!(
            ResearchReproducibilityManifestV1::from_slice(&encoded).unwrap(),
            manifest
        );
    }

    #[test]
    fn seed_and_output_drift_fail_closed() {
        let run = admitted_run();
        let manifest = ResearchReproducibilityManifestV1::new_for_run(
            "manifest-1",
            &run,
            None,
            digest('4'),
            digest('5'),
            digest('6'),
            digest('7'),
            Vec::new(),
            vec![digest('a')],
        )
        .unwrap();
        let mut drifted = run.clone();
        drifted.random_seed = Some(8);
        assert!(matches!(
            manifest.validate_for_run(&drifted),
            Err(ResearchContractError::InvalidField("randomSeed"))
        ));
        let foreign = ResearchProvenanceReceiptV1::new(
            "project-1",
            1,
            "run-1",
            "artifact-x",
            ResearchArtifactKindV1::Figure,
            digest('c'),
            vec![digest('2')],
            digest('6'),
            digest('5'),
            digest('4'),
            "provider-1",
            None,
            Some(7),
            None,
        )
        .unwrap();
        assert!(matches!(
            manifest.validate_against_provenance(&[foreign]),
            Err(ResearchContractError::InvalidField("artifactDigest"))
        ));
    }
}
