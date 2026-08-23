//! Bounded, content-addressed evidence for model-visible run capabilities and
//! provider-neutral model input.
//!
//! The snapshots in this module deliberately retain only counters, stable
//! capability facts, and domain-separated digests. They never retain prompt
//! text, Tool output, source text, vectors, credentials, or provider endpoints.

use crate::workspace::{WorkspaceCapabilities, WorkspaceRetrievalPhase};
use serde::{Deserialize, Serialize};
use thiserror::Error;

mod digest;
mod input;
mod source;
#[cfg(test)]
mod tests;
mod usage;

use digest::{measure, require_digest, require_optional_digest};
pub(crate) use source::{ModelCallObservation, RunCapabilityEvidenceSource};
pub(crate) use usage::ModelUsageBinding;
pub use usage::{ModelUsageSnapshotV1, ToolResultContextUsageV1, MODEL_USAGE_SNAPSHOT_V1_SCHEMA};

pub const RUN_CAPABILITY_SNAPSHOT_V1_SCHEMA: &str = "a3s.code.run-capability-snapshot.v1";
pub const MODEL_PRESENTATION_SNAPSHOT_V1_SCHEMA: &str = "a3s.code.model-presentation-snapshot.v1";
pub const MODEL_INPUT_SNAPSHOT_V1_SCHEMA: &str = "a3s.code.model-input-snapshot.v1";

const CAPABILITY_SNAPSHOT_DOMAIN: &str = "a3s.code.run-capability-snapshot.v1";
const MODEL_PRESENTATION_SNAPSHOT_DOMAIN: &str = "a3s.code.model-presentation-snapshot.v1";
const MODEL_INPUT_SNAPSHOT_DOMAIN: &str = "a3s.code.model-input-snapshot.v1";
const MODEL_INPUT_PAYLOAD_DOMAIN: &str = "a3s.code.model-input-payload.v1";
const MODEL_MESSAGES_DOMAIN: &str = "a3s.code.model-input-messages.v1";
const MODEL_SYSTEM_DOMAIN: &str = "a3s.code.model-input-system.v1";
const MODEL_TOOLS_DOMAIN: &str = "a3s.code.model-visible-tools.v1";
const MODEL_STRUCTURED_DOMAIN: &str = "a3s.code.model-input-structured.v1";
const RETRIEVAL_RESULTS_DOMAIN: &str = "a3s.code.model-input-retrieval-results.v1";
const TOOL_RESULT_CONTENT_DOMAIN: &str = "a3s.code.model-input-tool-result-content.v1";
const TOOL_RESULT_CONTENTS_DOMAIN: &str = "a3s.code.model-input-tool-result-contents.v1";
const REPEATED_TOOL_RESULT_CONTENTS_DOMAIN: &str =
    "a3s.code.model-input-repeated-tool-result-contents.v1";
const RETRIEVAL_MODEL_DOMAIN: &str = "a3s.code.workspace-retrieval-model.v1";
const PERMISSION_POLICY_DOMAIN: &str = "a3s.code.permission-policy.v1";
const CONFIRMATION_POLICY_DOMAIN: &str = "a3s.code.confirmation-policy.v1";

#[derive(Debug, Error)]
pub enum HarnessEvidenceError {
    #[error("Harness evidence could not be serialized: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Harness evidence uses an unsupported schema")]
    UnsupportedSchema,
    #[error("Harness evidence field `{0}` is not a canonical SHA-256 digest")]
    InvalidDigest(&'static str),
    #[error("Harness evidence violates invariant `{0}`")]
    InvalidContents(&'static str),
    #[error("Harness evidence field `{0}` does not match the snapshot contents")]
    DigestMismatch(&'static str),
    #[error("Harness model-call sequence is exhausted")]
    CallSequenceExhausted,
    #[error(transparent)]
    ToolPresentation(#[from] crate::tools::ToolPresentationError),
}

/// Model-call shape used by one provider-neutral [`crate::llm::LlmClient`]
/// invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelInputKindV1 {
    Completion,
    Streaming,
    Structured,
    StreamingStructured,
}

/// Whether a provider-neutral call used the Session's Tool-presentation
/// profile or a host-owned auxiliary protocol such as structured validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPresentationApplicationV1 {
    Profiled,
    Auxiliary,
}

/// Non-sensitive workspace service surface visible to one model call.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceCapabilitySnapshotV1 {
    pub read: bool,
    pub write: bool,
    pub exec: bool,
    pub search: bool,
    pub git: bool,
    pub code_intelligence: bool,
}

impl From<WorkspaceCapabilities> for WorkspaceCapabilitySnapshotV1 {
    fn from(value: WorkspaceCapabilities) -> Self {
        Self {
            read: value.read,
            write: value.write,
            exec: value.exec,
            search: value.search,
            git: value.git,
            code_intelligence: value.code_intelligence,
        }
    }
}

/// Run-owned governance bindings, serializable policy identities, and
/// execution ceilings observed for the same run as the model call.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunPolicyCeilingSnapshotV1 {
    pub permission_checker_bound: bool,
    pub permission_policy_digest: Option<String>,
    pub confirmation_manager_bound: bool,
    pub confirmation_policy_digest: Option<String>,
    pub budget_guard_bound: bool,
    pub active_skill_tool_restrictions: bool,
    pub max_tool_rounds: usize,
    pub max_parallel_tasks: usize,
    pub tool_timeout_ms: Option<u64>,
    pub llm_api_timeout_ms: Option<u64>,
    pub max_execution_time_ms: Option<u64>,
}

/// Exact semantic readiness and generation identities observed immediately
/// before a model call. Model identity is represented only by a digest of the
/// non-sensitive provider descriptor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceRetrievalCapabilitySnapshotV1 {
    pub enabled: bool,
    pub phase: WorkspaceRetrievalPhase,
    pub catalog_revision: u64,
    pub source_revision: u64,
    pub vector_revision: u64,
    pub coverage_bps: u16,
    pub model_digest: Option<String>,
}

/// Immutable description of the exact capability surface exposed for a model
/// call. The digest excludes itself and remains stable while the surface and
/// retrieval readiness generation remain unchanged.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunCapabilitySnapshotV1 {
    pub schema: String,
    pub model_visible_tool_count: usize,
    pub model_visible_tools_digest: String,
    pub workspace: WorkspaceCapabilitySnapshotV1,
    pub policy: RunPolicyCeilingSnapshotV1,
    pub retrieval: WorkspaceRetrievalCapabilitySnapshotV1,
    pub snapshot_digest: String,
}

impl RunCapabilitySnapshotV1 {
    fn new(
        model_visible_tool_count: usize,
        model_visible_tools_digest: String,
        workspace: WorkspaceCapabilitySnapshotV1,
        policy: RunPolicyCeilingSnapshotV1,
        retrieval: WorkspaceRetrievalCapabilitySnapshotV1,
    ) -> Result<Self, HarnessEvidenceError> {
        let mut snapshot = Self {
            schema: RUN_CAPABILITY_SNAPSHOT_V1_SCHEMA.to_string(),
            model_visible_tool_count,
            model_visible_tools_digest,
            workspace,
            policy,
            retrieval,
            snapshot_digest: String::new(),
        };
        snapshot.snapshot_digest = snapshot.expected_digest()?;
        Ok(snapshot)
    }

    pub fn validate(&self) -> Result<(), HarnessEvidenceError> {
        if self.schema != RUN_CAPABILITY_SNAPSHOT_V1_SCHEMA {
            return Err(HarnessEvidenceError::UnsupportedSchema);
        }
        let disabled_shape = self.retrieval.phase == WorkspaceRetrievalPhase::Disabled
            && self.retrieval.catalog_revision == 0
            && self.retrieval.source_revision == 0
            && self.retrieval.vector_revision == 0
            && self.retrieval.coverage_bps == 0
            && self.retrieval.model_digest.is_none();
        if self.retrieval.coverage_bps > 10_000 {
            return Err(HarnessEvidenceError::InvalidContents(
                "retrieval.coverage_bps <= 10_000",
            ));
        }
        if self.retrieval.enabled && self.retrieval.phase == WorkspaceRetrievalPhase::Disabled {
            return Err(HarnessEvidenceError::InvalidContents(
                "enabled retrieval has a live phase",
            ));
        }
        if self.retrieval.enabled && self.retrieval.model_digest.is_none() {
            return Err(HarnessEvidenceError::InvalidContents(
                "enabled retrieval has a model descriptor digest",
            ));
        }
        if !self.retrieval.enabled && !disabled_shape {
            return Err(HarnessEvidenceError::InvalidContents(
                "disabled retrieval has an empty generation",
            ));
        }
        require_digest(
            "model_visible_tools_digest",
            &self.model_visible_tools_digest,
        )?;
        require_optional_digest(
            "permission_policy_digest",
            self.policy.permission_policy_digest.as_deref(),
        )?;
        require_optional_digest(
            "confirmation_policy_digest",
            self.policy.confirmation_policy_digest.as_deref(),
        )?;
        require_optional_digest(
            "retrieval.model_digest",
            self.retrieval.model_digest.as_deref(),
        )?;
        require_digest("snapshot_digest", &self.snapshot_digest)?;
        if self.snapshot_digest != self.expected_digest()? {
            return Err(HarnessEvidenceError::DigestMismatch("snapshot_digest"));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, HarnessEvidenceError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            model_visible_tool_count: usize,
            model_visible_tools_digest: &'a str,
            workspace: &'a WorkspaceCapabilitySnapshotV1,
            policy: &'a RunPolicyCeilingSnapshotV1,
            retrieval: &'a WorkspaceRetrievalCapabilitySnapshotV1,
        }

        Ok(measure(
            CAPABILITY_SNAPSHOT_DOMAIN,
            &Identity {
                schema: &self.schema,
                model_visible_tool_count: self.model_visible_tool_count,
                model_visible_tools_digest: &self.model_visible_tools_digest,
                workspace: &self.workspace,
                policy: &self.policy,
                retrieval: &self.retrieval,
            },
        )?
        .digest)
    }
}

/// Per-call evidence binding a frozen presentation-profile identity to both
/// its canonical source definitions and the definitions actually submitted to
/// the provider-neutral LLM boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelPresentationSnapshotV1 {
    pub schema: String,
    pub call_sequence: u64,
    pub profile: crate::tools::ToolPresentationProfileV1,
    pub application: ModelPresentationApplicationV1,
    pub source_tool_count: usize,
    pub source_tool_definitions_digest: String,
    pub source_estimated_tokens: usize,
    pub presented_tool_count: usize,
    pub presented_tool_definitions_digest: String,
    pub presented_estimated_tokens: usize,
    pub snapshot_digest: String,
}

impl ModelPresentationSnapshotV1 {
    pub(crate) fn new(
        call_sequence: u64,
        profile: crate::tools::ToolPresentationProfileV1,
        application: ModelPresentationApplicationV1,
        source_tools: &[crate::llm::ToolDefinition],
        source_tool_definitions_digest: String,
        presented_tools: &[crate::llm::ToolDefinition],
        presented_tool_definitions_digest: String,
    ) -> Result<Self, HarnessEvidenceError> {
        let mut snapshot = Self {
            schema: MODEL_PRESENTATION_SNAPSHOT_V1_SCHEMA.to_owned(),
            call_sequence,
            profile,
            application,
            source_tool_count: source_tools.len(),
            source_tool_definitions_digest,
            source_estimated_tokens: crate::tools::estimated_definition_tokens(source_tools),
            presented_tool_count: presented_tools.len(),
            presented_tool_definitions_digest,
            presented_estimated_tokens: crate::tools::estimated_definition_tokens(presented_tools),
            snapshot_digest: String::new(),
        };
        snapshot.snapshot_digest = snapshot.expected_digest()?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn validate(&self) -> Result<(), HarnessEvidenceError> {
        if self.schema != MODEL_PRESENTATION_SNAPSHOT_V1_SCHEMA {
            return Err(HarnessEvidenceError::UnsupportedSchema);
        }
        if self.call_sequence == 0 {
            return Err(HarnessEvidenceError::InvalidContents(
                "presentation call_sequence is positive",
            ));
        }
        self.profile.validate()?;
        require_digest(
            "source_tool_definitions_digest",
            &self.source_tool_definitions_digest,
        )?;
        require_digest(
            "presented_tool_definitions_digest",
            &self.presented_tool_definitions_digest,
        )?;
        require_digest("snapshot_digest", &self.snapshot_digest)?;
        if self.application == ModelPresentationApplicationV1::Profiled
            && self.presented_tool_count > self.source_tool_count
        {
            return Err(HarnessEvidenceError::InvalidContents(
                "profiled presentation cannot add Tool definitions",
            ));
        }
        if self.application == ModelPresentationApplicationV1::Auxiliary
            && (self.source_tool_count != self.presented_tool_count
                || self.source_tool_definitions_digest != self.presented_tool_definitions_digest
                || self.source_estimated_tokens != self.presented_estimated_tokens)
        {
            return Err(HarnessEvidenceError::InvalidContents(
                "auxiliary presentation source and submitted definitions agree",
            ));
        }
        if self.snapshot_digest != self.expected_digest()? {
            return Err(HarnessEvidenceError::DigestMismatch("snapshot_digest"));
        }
        Ok(())
    }

    pub fn validate_against(
        &self,
        input: &ModelInputSnapshotV1,
    ) -> Result<(), HarnessEvidenceError> {
        self.validate()?;
        input.validate()?;
        if self.call_sequence != input.call_sequence {
            return Err(HarnessEvidenceError::InvalidContents(
                "presentation and input call sequences agree",
            ));
        }
        if self.presented_tool_count != input.tool_count {
            return Err(HarnessEvidenceError::InvalidContents(
                "presentation and input Tool counts agree",
            ));
        }
        if self.presented_tool_definitions_digest != input.tool_definitions_digest {
            return Err(HarnessEvidenceError::DigestMismatch(
                "presented_tool_definitions_digest",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, HarnessEvidenceError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            call_sequence: u64,
            profile: &'a crate::tools::ToolPresentationProfileV1,
            application: ModelPresentationApplicationV1,
            source_tool_count: usize,
            source_tool_definitions_digest: &'a str,
            source_estimated_tokens: usize,
            presented_tool_count: usize,
            presented_tool_definitions_digest: &'a str,
            presented_estimated_tokens: usize,
        }

        Ok(measure(
            MODEL_PRESENTATION_SNAPSHOT_DOMAIN,
            &Identity {
                schema: &self.schema,
                call_sequence: self.call_sequence,
                profile: &self.profile,
                application: self.application,
                source_tool_count: self.source_tool_count,
                source_tool_definitions_digest: &self.source_tool_definitions_digest,
                source_estimated_tokens: self.source_estimated_tokens,
                presented_tool_count: self.presented_tool_count,
                presented_tool_definitions_digest: &self.presented_tool_definitions_digest,
                presented_estimated_tokens: self.presented_estimated_tokens,
            },
        )?
        .digest)
    }
}

/// Immutable, bounded evidence for the arguments submitted to one
/// provider-neutral model call.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelInputSnapshotV1 {
    pub schema: String,
    pub call_sequence: u64,
    pub kind: ModelInputKindV1,
    pub message_count: usize,
    pub content_block_count: usize,
    pub image_block_count: usize,
    pub tool_result_count: usize,
    pub tool_count: usize,
    pub retrieval_result_count: usize,
    pub retrieval_result_bytes: u64,
    pub retrieval_results_digest: Option<String>,
    pub system_bytes: u64,
    pub message_payload_bytes: u64,
    pub tool_definition_bytes: u64,
    pub structured_output_bytes: u64,
    pub payload_bytes: u64,
    pub estimated_prompt_tokens: usize,
    pub messages_digest: String,
    pub system_digest: Option<String>,
    pub tool_definitions_digest: String,
    pub structured_output_digest: Option<String>,
    pub input_digest: String,
    pub capability_snapshot_digest: String,
    pub snapshot_digest: String,
}

impl ModelInputSnapshotV1 {
    pub fn validate(&self) -> Result<(), HarnessEvidenceError> {
        if self.schema != MODEL_INPUT_SNAPSHOT_V1_SCHEMA {
            return Err(HarnessEvidenceError::UnsupportedSchema);
        }
        if self.call_sequence == 0 {
            return Err(HarnessEvidenceError::InvalidContents(
                "call_sequence is positive",
            ));
        }
        if self.tool_result_count > self.content_block_count {
            return Err(HarnessEvidenceError::InvalidContents(
                "tool results are content blocks",
            ));
        }
        if self.retrieval_result_count > self.tool_result_count {
            return Err(HarnessEvidenceError::InvalidContents(
                "retrieval results are tool results",
            ));
        }
        if (self.retrieval_result_count == 0) != self.retrieval_results_digest.is_none()
            || (self.retrieval_result_count == 0) != (self.retrieval_result_bytes == 0)
        {
            return Err(HarnessEvidenceError::InvalidContents(
                "retrieval result count, bytes, and digest agree",
            ));
        }
        if (self.system_bytes == 0) != self.system_digest.is_none() {
            return Err(HarnessEvidenceError::InvalidContents(
                "system bytes and digest agree",
            ));
        }
        if self.structured_output_digest.is_none() && self.structured_output_bytes != 0 {
            return Err(HarnessEvidenceError::InvalidContents(
                "structured-output bytes require a directive digest",
            ));
        }
        if self.structured_output_digest.is_some() && self.structured_output_bytes == 0 {
            return Err(HarnessEvidenceError::InvalidContents(
                "a directive digest has serialized bytes",
            ));
        }
        if self.message_payload_bytes == 0
            || self.tool_definition_bytes == 0
            || self.payload_bytes == 0
        {
            return Err(HarnessEvidenceError::InvalidContents(
                "serialized input components are non-empty",
            ));
        }
        for (field, digest) in [
            ("messages_digest", self.messages_digest.as_str()),
            (
                "tool_definitions_digest",
                self.tool_definitions_digest.as_str(),
            ),
            ("input_digest", self.input_digest.as_str()),
            (
                "capability_snapshot_digest",
                self.capability_snapshot_digest.as_str(),
            ),
            ("snapshot_digest", self.snapshot_digest.as_str()),
        ] {
            require_digest(field, digest)?;
        }
        require_optional_digest("system_digest", self.system_digest.as_deref())?;
        require_optional_digest(
            "structured_output_digest",
            self.structured_output_digest.as_deref(),
        )?;
        require_optional_digest(
            "retrieval_results_digest",
            self.retrieval_results_digest.as_deref(),
        )?;
        if self.snapshot_digest != self.expected_digest()? {
            return Err(HarnessEvidenceError::DigestMismatch("snapshot_digest"));
        }
        Ok(())
    }

    /// Validate this input and the exact capability snapshot it references.
    pub fn validate_against(
        &self,
        capability: &RunCapabilitySnapshotV1,
    ) -> Result<(), HarnessEvidenceError> {
        self.validate()?;
        capability.validate()?;
        if self.capability_snapshot_digest != capability.snapshot_digest {
            return Err(HarnessEvidenceError::DigestMismatch(
                "capability_snapshot_digest",
            ));
        }
        if self.tool_count != capability.model_visible_tool_count {
            return Err(HarnessEvidenceError::InvalidContents(
                "input and capability tool counts agree",
            ));
        }
        if self.tool_definitions_digest != capability.model_visible_tools_digest {
            return Err(HarnessEvidenceError::DigestMismatch(
                "tool_definitions_digest",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, HarnessEvidenceError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            call_sequence: u64,
            kind: ModelInputKindV1,
            message_count: usize,
            content_block_count: usize,
            image_block_count: usize,
            tool_result_count: usize,
            tool_count: usize,
            retrieval_result_count: usize,
            retrieval_result_bytes: u64,
            retrieval_results_digest: &'a Option<String>,
            system_bytes: u64,
            message_payload_bytes: u64,
            tool_definition_bytes: u64,
            structured_output_bytes: u64,
            payload_bytes: u64,
            estimated_prompt_tokens: usize,
            messages_digest: &'a str,
            system_digest: &'a Option<String>,
            tool_definitions_digest: &'a str,
            structured_output_digest: &'a Option<String>,
            input_digest: &'a str,
            capability_snapshot_digest: &'a str,
        }

        Ok(measure(
            MODEL_INPUT_SNAPSHOT_DOMAIN,
            &Identity {
                schema: &self.schema,
                call_sequence: self.call_sequence,
                kind: self.kind,
                message_count: self.message_count,
                content_block_count: self.content_block_count,
                image_block_count: self.image_block_count,
                tool_result_count: self.tool_result_count,
                tool_count: self.tool_count,
                retrieval_result_count: self.retrieval_result_count,
                retrieval_result_bytes: self.retrieval_result_bytes,
                retrieval_results_digest: &self.retrieval_results_digest,
                system_bytes: self.system_bytes,
                message_payload_bytes: self.message_payload_bytes,
                tool_definition_bytes: self.tool_definition_bytes,
                structured_output_bytes: self.structured_output_bytes,
                payload_bytes: self.payload_bytes,
                estimated_prompt_tokens: self.estimated_prompt_tokens,
                messages_digest: &self.messages_digest,
                system_digest: &self.system_digest,
                tool_definitions_digest: &self.tool_definitions_digest,
                structured_output_digest: &self.structured_output_digest,
                input_digest: &self.input_digest,
                capability_snapshot_digest: &self.capability_snapshot_digest,
            },
        )?
        .digest)
    }
}
