use super::{digest, validate_digest_field, validate_id, ResearchContractError};
use crate::capability::RunCapabilityBindingV1;
use serde::{Deserialize, Serialize};

pub const RESEARCH_RUN_SCHEMA_V1: &str = "a3s.code.research-run.v1";
const RESEARCH_RUN_DIGEST_DOMAIN: &str = "a3s.code.research-run.identity.v1";

/// Reproducibility promise selected by a host for one research run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchReproducibilityV1 {
    Exploratory,
    Reproducible,
    Deterministic,
}

impl ResearchReproducibilityV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exploratory => "exploratory",
            Self::Reproducible => "reproducible",
            Self::Deterministic => "deterministic",
        }
    }
}

/// Durable lifecycle state for a research run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchRunStatusV1 {
    Planned,
    Admitted,
    Running,
    Checkpointed,
    Completed,
    Failed,
    Cancelled,
}

impl ResearchRunStatusV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Admitted => "admitted",
            Self::Running => "running",
            Self::Checkpointed => "checkpointed",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Planned, Self::Admitted | Self::Cancelled)
                | (Self::Admitted, Self::Running | Self::Cancelled)
                | (
                    Self::Running,
                    Self::Checkpointed | Self::Completed | Self::Failed | Self::Cancelled
                )
                | (
                    Self::Checkpointed,
                    Self::Running | Self::Completed | Self::Failed | Self::Cancelled
                )
        )
    }
}

/// Exact Code/Use identity and policy binding for one scientific run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResearchRunV1 {
    pub schema: String,
    pub run_id: String,
    pub project_id: String,
    pub project_revision: u64,
    pub source_snapshot_digest: String,
    pub evidence_snapshot_digest: String,
    pub capability_binding: RunCapabilityBindingV1,
    pub provider_id: String,
    pub model_id: String,
    pub reproducibility: ResearchReproducibilityV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub random_seed: Option<u64>,
    pub status: ResearchRunStatusV1,
    pub run_digest: String,
}

impl ResearchRunV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        run_id: impl Into<String>,
        project_id: impl Into<String>,
        project_revision: u64,
        source_snapshot_digest: impl Into<String>,
        evidence_snapshot_digest: impl Into<String>,
        capability_binding: RunCapabilityBindingV1,
        provider_id: impl Into<String>,
        model_id: impl Into<String>,
        reproducibility: ResearchReproducibilityV1,
        random_seed: Option<u64>,
    ) -> Result<Self, ResearchContractError> {
        let mut run = Self {
            schema: RESEARCH_RUN_SCHEMA_V1.to_owned(),
            run_id: run_id.into(),
            project_id: project_id.into(),
            project_revision,
            source_snapshot_digest: source_snapshot_digest.into(),
            evidence_snapshot_digest: evidence_snapshot_digest.into(),
            capability_binding,
            provider_id: provider_id.into(),
            model_id: model_id.into(),
            reproducibility,
            random_seed,
            status: ResearchRunStatusV1::Planned,
            run_digest: String::new(),
        };
        run.validate_without_digest()?;
        run.run_digest = run.expected_digest()?;
        Ok(run)
    }

    pub fn validate(&self) -> Result<(), ResearchContractError> {
        self.validate_without_digest()?;
        validate_digest_field("runDigest", &self.run_digest)?;
        if self.run_digest != self.expected_digest()? {
            return Err(ResearchContractError::DigestMismatch("runDigest"));
        }
        Ok(())
    }

    /// Verify that a research run is attached to the exact Code execution
    /// target that was admitted by the host.  The target's session identity is
    /// retained by Code's execution plane; this contract only owns the shared
    /// Run id and refuses a cross-Run projection.
    pub fn validate_execution_target(
        &self,
        target: &crate::evaluation::ExecutionTargetV1,
    ) -> Result<(), ResearchContractError> {
        self.validate()?;
        target
            .validate()
            .map_err(|_| ResearchContractError::InvalidField("executionTarget"))?;
        if target.run_id != self.run_id {
            return Err(ResearchContractError::InvalidField("executionTarget.runId"));
        }
        Ok(())
    }

    /// Validate that this Run has crossed the admission boundary before a
    /// host attaches reviewer evidence or findings to it.
    ///
    /// A planned Run has not yet frozen an executable identity, so accepting
    /// review output for it would allow an evaluator result to exist without
    /// a corresponding admitted research execution. Terminal and checkpoint
    /// states remain reviewable because hosts may inspect completed or failed
    /// artifacts after execution.
    pub(crate) fn validate_reviewable(&self) -> Result<(), ResearchContractError> {
        self.validate()?;
        if matches!(self.status, ResearchRunStatusV1::Planned) {
            return Err(ResearchContractError::InvalidField("researchRun.status"));
        }
        Ok(())
    }

    pub fn transition_to(
        &mut self,
        next: ResearchRunStatusV1,
    ) -> Result<(), ResearchContractError> {
        self.validate()?;
        if !self.status.can_transition_to(next) {
            return Err(ResearchContractError::InvalidTransition {
                from: self.status.as_str(),
                to: next.as_str(),
            });
        }
        self.status = next;
        self.run_digest = self.expected_digest()?;
        Ok(())
    }

    fn validate_without_digest(&self) -> Result<(), ResearchContractError> {
        if self.schema != RESEARCH_RUN_SCHEMA_V1 {
            return Err(ResearchContractError::UnsupportedSchema);
        }
        validate_id("runId", &self.run_id)?;
        validate_id("projectId", &self.project_id)?;
        if self.project_revision == 0 {
            return Err(ResearchContractError::InvalidField("projectRevision"));
        }
        validate_digest_field("sourceSnapshotDigest", &self.source_snapshot_digest)?;
        validate_digest_field("evidenceSnapshotDigest", &self.evidence_snapshot_digest)?;
        self.capability_binding
            .validate()
            .map_err(|_| ResearchContractError::InvalidField("capabilityBinding"))?;
        validate_id("providerId", &self.provider_id)?;
        validate_id("modelId", &self.model_id)?;
        if matches!(
            self.reproducibility,
            ResearchReproducibilityV1::Deterministic
        ) && self.random_seed.is_none()
        {
            return Err(ResearchContractError::InvalidField("randomSeed"));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, ResearchContractError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            run_id: &'a str,
            project_id: &'a str,
            project_revision: u64,
            source_snapshot_digest: &'a str,
            evidence_snapshot_digest: &'a str,
            capability_binding: &'a RunCapabilityBindingV1,
            provider_id: &'a str,
            model_id: &'a str,
            reproducibility: ResearchReproducibilityV1,
            random_seed: Option<u64>,
            status: ResearchRunStatusV1,
        }
        digest(
            RESEARCH_RUN_DIGEST_DOMAIN,
            &Identity {
                schema: &self.schema,
                run_id: &self.run_id,
                project_id: &self.project_id,
                project_revision: self.project_revision,
                source_snapshot_digest: &self.source_snapshot_digest,
                evidence_snapshot_digest: &self.evidence_snapshot_digest,
                capability_binding: &self.capability_binding,
                provider_id: &self.provider_id,
                model_id: &self.model_id,
                reproducibility: self.reproducibility,
                random_seed: self.random_seed,
                status: self.status,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{
        CapabilityCeiling, CapabilityContribution, CapabilityDescriptor,
        CapabilityExecutionCeiling, CapabilityKind, CapabilitySet, CapabilitySource,
        CodeCatalogGeneration, GovernanceCapabilityCeiling, Sha256Digest,
        WorkspaceCapabilityCeiling,
    };

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

    #[test]
    fn run_digest_changes_when_status_changes() {
        let mut run = ResearchRunV1::new(
            "run-1",
            "project-1",
            1,
            digest('a'),
            digest('b'),
            binding(),
            "local",
            "model",
            ResearchReproducibilityV1::Reproducible,
            None,
        )
        .unwrap();
        let before = run.run_digest.clone();
        run.transition_to(ResearchRunStatusV1::Admitted).unwrap();
        assert_ne!(before, run.run_digest);
        assert!(run.validate().is_ok());
    }

    #[test]
    fn deterministic_runs_require_a_seed_and_terminal_runs_cannot_resume() {
        assert!(matches!(
            ResearchRunV1::new(
                "run-1",
                "project-1",
                1,
                digest('a'),
                digest('b'),
                binding(),
                "local",
                "model",
                ResearchReproducibilityV1::Deterministic,
                None,
            ),
            Err(ResearchContractError::InvalidField("randomSeed"))
        ));
        let mut run = ResearchRunV1::new(
            "run-1",
            "project-1",
            1,
            digest('a'),
            digest('b'),
            binding(),
            "local",
            "model",
            ResearchReproducibilityV1::Reproducible,
            None,
        )
        .unwrap();
        run.transition_to(ResearchRunStatusV1::Admitted).unwrap();
        run.transition_to(ResearchRunStatusV1::Running).unwrap();
        run.transition_to(ResearchRunStatusV1::Completed).unwrap();
        assert!(matches!(
            run.transition_to(ResearchRunStatusV1::Running),
            Err(ResearchContractError::InvalidTransition {
                from: "completed",
                to: "running"
            })
        ));
    }

    #[test]
    fn transition_rejects_a_tampered_run_before_rebinding_identity() {
        let mut run = ResearchRunV1::new(
            "run-1",
            "project-1",
            1,
            digest('a'),
            digest('b'),
            binding(),
            "local",
            "model",
            ResearchReproducibilityV1::Reproducible,
            None,
        )
        .unwrap();
        run.project_id = "other-project".to_owned();
        assert_eq!(
            run.transition_to(ResearchRunStatusV1::Admitted),
            Err(ResearchContractError::DigestMismatch("runDigest"))
        );
    }

    #[test]
    fn model_id_uses_the_same_single_line_identity_bound_as_provider_id() {
        assert_eq!(
            ResearchRunV1::new(
                "run-1",
                "project-1",
                1,
                digest('a'),
                digest('b'),
                binding(),
                "local",
                "model\nwith-newline",
                ResearchReproducibilityV1::Exploratory,
                None,
            ),
            Err(ResearchContractError::InvalidField("modelId"))
        );
    }

    #[test]
    fn execution_target_binding_rejects_a_cross_run_projection() {
        let run = ResearchRunV1::new(
            "run-1",
            "project-1",
            1,
            digest('a'),
            digest('b'),
            binding(),
            "local",
            "model",
            ResearchReproducibilityV1::Reproducible,
            None,
        )
        .unwrap();
        assert!(run
            .validate_execution_target(&crate::evaluation::ExecutionTargetV1::new(
                "session-1",
                "run-1"
            ))
            .is_ok());
        assert_eq!(
            run.validate_execution_target(&crate::evaluation::ExecutionTargetV1::new(
                "session-1",
                "run-2"
            )),
            Err(ResearchContractError::InvalidField("executionTarget.runId"))
        );
    }
}
