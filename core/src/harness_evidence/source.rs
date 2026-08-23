use super::digest::measure;
use super::input::{capture_model_input, ModelInputCapture};
use super::{
    HarnessEvidenceError, ModelInputKindV1, ModelInputSnapshotV1, ModelPresentationApplicationV1,
    ModelPresentationSnapshotV1, RunCapabilitySnapshotV1, RunPolicyCeilingSnapshotV1,
    ToolResultContextUsageV1, WorkspaceRetrievalCapabilitySnapshotV1, CONFIRMATION_POLICY_DOMAIN,
    MODEL_TOOLS_DOMAIN, PERMISSION_POLICY_DOMAIN, RETRIEVAL_MODEL_DOMAIN,
};
use crate::agent::AgentConfig;
use crate::llm::structured::StructuredDirective;
use crate::llm::{Message, ToolDefinition};
use crate::workspace::{WorkspaceRetrievalStatus, WorkspaceServices};
use serde::Serialize;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub(crate) struct ModelCallObservation<'a> {
    pub(crate) kind: ModelInputKindV1,
    pub(crate) messages: &'a [Message],
    pub(crate) system: Option<&'a str>,
    pub(crate) tools: &'a [ToolDefinition],
    pub(crate) directive: Option<&'a StructuredDirective>,
    pub(crate) estimated_prompt_tokens: usize,
    pub(crate) presentation_application: ModelPresentationApplicationV1,
}

impl<'a> ModelCallObservation<'a> {
    #[cfg(test)]
    pub(crate) fn new(
        kind: ModelInputKindV1,
        messages: &'a [Message],
        system: Option<&'a str>,
        tools: &'a [ToolDefinition],
        directive: Option<&'a StructuredDirective>,
        estimated_prompt_tokens: usize,
    ) -> Self {
        Self::with_presentation_application(
            kind,
            messages,
            system,
            tools,
            directive,
            estimated_prompt_tokens,
            ModelPresentationApplicationV1::Auxiliary,
        )
    }

    pub(crate) fn with_presentation_application(
        kind: ModelInputKindV1,
        messages: &'a [Message],
        system: Option<&'a str>,
        tools: &'a [ToolDefinition],
        directive: Option<&'a StructuredDirective>,
        estimated_prompt_tokens: usize,
        presentation_application: ModelPresentationApplicationV1,
    ) -> Self {
        Self {
            kind,
            messages,
            system,
            tools,
            directive,
            estimated_prompt_tokens,
            presentation_application,
        }
    }
}

#[derive(Clone)]
pub(crate) struct RunCapabilityEvidenceSource {
    workspace_services: Arc<WorkspaceServices>,
    permission_checker_bound: bool,
    permission_policy: Option<crate::permissions::PermissionPolicy>,
    confirmation_manager_bound: bool,
    confirmation_policy: Option<crate::hitl::ConfirmationPolicy>,
    budget_guard_bound: bool,
    active_skill_tool_restrictions: bool,
    max_tool_rounds: usize,
    max_parallel_tasks: usize,
    tool_timeout_ms: Option<u64>,
    llm_api_timeout_ms: Option<u64>,
    max_execution_time_ms: Option<u64>,
    tool_presentation_profile: crate::tools::ToolPresentationProfileV1,
    presentation_source_tools: Vec<ToolDefinition>,
}

impl RunCapabilityEvidenceSource {
    #[cfg(test)]
    pub(crate) fn from_agent(
        config: &AgentConfig,
        workspace_services: Arc<WorkspaceServices>,
        permission_checker_bound: bool,
        confirmation_manager_bound: bool,
    ) -> Self {
        Self::from_agent_inner(
            config,
            workspace_services,
            None,
            permission_checker_bound,
            confirmation_manager_bound,
        )
    }

    pub(crate) fn from_agent_with_permission_checker(
        config: &AgentConfig,
        workspace_services: Arc<WorkspaceServices>,
        permission_checker: Option<&Arc<dyn crate::permissions::PermissionChecker>>,
        confirmation_manager_bound: bool,
    ) -> Self {
        Self::from_agent_inner(
            config,
            workspace_services,
            permission_checker,
            permission_checker.is_some(),
            confirmation_manager_bound,
        )
    }

    fn from_agent_inner(
        config: &AgentConfig,
        workspace_services: Arc<WorkspaceServices>,
        permission_checker: Option<&Arc<dyn crate::permissions::PermissionChecker>>,
        permission_checker_bound: bool,
        confirmation_manager_bound: bool,
    ) -> Self {
        let mut presentation_source_tools = config.tools.clone();
        if let Some(permission_checker) = permission_checker {
            presentation_source_tools.retain(|tool| permission_checker.expose_to_model(&tool.name));
        }
        Self {
            workspace_services,
            permission_checker_bound,
            permission_policy: config.permission_policy.clone(),
            confirmation_manager_bound,
            confirmation_policy: config.confirmation_policy.clone(),
            budget_guard_bound: config.budget_guard.is_some(),
            active_skill_tool_restrictions: config.enforce_active_skill_tool_restrictions,
            max_tool_rounds: config.max_tool_rounds,
            max_parallel_tasks: config.max_parallel_tasks,
            tool_timeout_ms: config.tool_timeout_ms,
            llm_api_timeout_ms: config.llm_api_timeout_ms,
            max_execution_time_ms: config.max_execution_time_ms,
            tool_presentation_profile: config.tool_presentation_profile.clone(),
            presentation_source_tools,
        }
    }

    #[cfg(test)]
    pub(crate) fn capture(
        &self,
        call_sequence: u64,
        observation: ModelCallObservation<'_>,
    ) -> Result<
        (
            RunCapabilitySnapshotV1,
            ModelInputSnapshotV1,
            ToolResultContextUsageV1,
        ),
        HarnessEvidenceError,
    > {
        let (capability, _presentation, input, usage) =
            self.capture_with_presentation(call_sequence, observation)?;
        Ok((capability, input, usage))
    }

    pub(crate) fn capture_with_presentation(
        &self,
        call_sequence: u64,
        observation: ModelCallObservation<'_>,
    ) -> Result<
        (
            RunCapabilitySnapshotV1,
            ModelPresentationSnapshotV1,
            ModelInputSnapshotV1,
            ToolResultContextUsageV1,
        ),
        HarnessEvidenceError,
    > {
        let tools_measurement = measure(MODEL_TOOLS_DOMAIN, observation.tools)?;
        let (presentation_source, source_measurement) = match observation.presentation_application {
            ModelPresentationApplicationV1::Profiled => {
                let source =
                    crate::tools::canonical_presentation_source(&self.presentation_source_tools)?;
                let expected = self
                    .tool_presentation_profile
                    .present_for_messages(&source, observation.messages)?;
                crate::tools::is_definition_subset(&expected, observation.tools)?;
                let measurement = measure(MODEL_TOOLS_DOMAIN, &source)?;
                (source, measurement)
            }
            ModelPresentationApplicationV1::Auxiliary => {
                let source = observation.tools.to_vec();
                let measurement = measure(MODEL_TOOLS_DOMAIN, &source)?;
                (source, measurement)
            }
        };
        let presentation = ModelPresentationSnapshotV1::new(
            call_sequence,
            self.tool_presentation_profile.clone(),
            observation.presentation_application,
            &presentation_source,
            source_measurement.digest,
            observation.tools,
            tools_measurement.digest.clone(),
        )?;
        let capability =
            self.capability_snapshot(observation.tools.len(), &tools_measurement.digest)?;
        let (input, tool_result_context) = capture_model_input(ModelInputCapture {
            call_sequence,
            kind: observation.kind,
            messages: observation.messages,
            system: observation.system,
            tools: observation.tools,
            directive: observation.directive,
            estimated_prompt_tokens: observation.estimated_prompt_tokens,
            tools_measurement,
            capability_snapshot_digest: &capability.snapshot_digest,
        })?;
        input.validate_against(&capability)?;
        presentation.validate_against(&input)?;
        Ok((capability, presentation, input, tool_result_context))
    }

    fn capability_snapshot(
        &self,
        tool_count: usize,
        tools_digest: &str,
    ) -> Result<RunCapabilitySnapshotV1, HarnessEvidenceError> {
        let permission_policy_digest = self
            .permission_policy
            .as_ref()
            .map(|policy| measure(PERMISSION_POLICY_DOMAIN, policy).map(|value| value.digest))
            .transpose()?;
        let confirmation_policy_digest = self
            .confirmation_policy
            .as_ref()
            .map(confirmation_policy_digest)
            .transpose()?;
        let policy = RunPolicyCeilingSnapshotV1 {
            permission_checker_bound: self.permission_checker_bound,
            permission_policy_digest,
            confirmation_manager_bound: self.confirmation_manager_bound,
            confirmation_policy_digest,
            budget_guard_bound: self.budget_guard_bound,
            active_skill_tool_restrictions: self.active_skill_tool_restrictions,
            max_tool_rounds: self.max_tool_rounds,
            max_parallel_tasks: self.max_parallel_tasks,
            tool_timeout_ms: self.tool_timeout_ms,
            llm_api_timeout_ms: self.llm_api_timeout_ms,
            max_execution_time_ms: self.max_execution_time_ms,
        };
        let runtime = self.workspace_services.workspace_retrieval();
        let status = runtime
            .as_ref()
            .map(|runtime| runtime.status())
            .unwrap_or_else(WorkspaceRetrievalStatus::disabled);
        let model_digest = status
            .model
            .as_ref()
            .map(|model| measure(RETRIEVAL_MODEL_DOMAIN, model).map(|value| value.digest))
            .transpose()?;
        let retrieval = WorkspaceRetrievalCapabilitySnapshotV1 {
            enabled: runtime.is_some(),
            phase: status.phase,
            catalog_revision: status.catalog_revision,
            source_revision: status.source_revision,
            vector_revision: status.vector_revision,
            coverage_bps: status.coverage_bps,
            model_digest,
        };
        RunCapabilitySnapshotV1::new(
            tool_count,
            tools_digest.to_string(),
            self.workspace_services.capabilities().into(),
            policy,
            retrieval,
        )
    }
}

#[derive(Serialize)]
struct ConfirmationPolicyEvidence<'a> {
    enabled: bool,
    default_timeout_ms: u64,
    timeout_action: crate::hitl::TimeoutAction,
    yolo_lanes: Vec<&'a crate::queue::SessionLane>,
}

fn confirmation_policy_digest(
    policy: &crate::hitl::ConfirmationPolicy,
) -> Result<String, HarnessEvidenceError> {
    let mut lanes = policy.yolo_lanes.iter().collect::<Vec<_>>();
    lanes.sort_by_key(|lane| lane.priority());
    Ok(measure(
        CONFIRMATION_POLICY_DOMAIN,
        &ConfirmationPolicyEvidence {
            enabled: policy.enabled,
            default_timeout_ms: policy.default_timeout_ms,
            timeout_action: policy.timeout_action,
            yolo_lanes: lanes,
        },
    )?
    .digest)
}
