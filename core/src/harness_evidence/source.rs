use super::digest::measure;
use super::input::{capture_model_input, ModelInputCapture};
use super::{
    HarnessEvidenceError, ModelInputKindV1, ModelInputSnapshotV1, RunCapabilitySnapshotV1,
    RunPolicyCeilingSnapshotV1, WorkspaceRetrievalCapabilitySnapshotV1, CONFIRMATION_POLICY_DOMAIN,
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
}

impl<'a> ModelCallObservation<'a> {
    pub(crate) fn new(
        kind: ModelInputKindV1,
        messages: &'a [Message],
        system: Option<&'a str>,
        tools: &'a [ToolDefinition],
        directive: Option<&'a StructuredDirective>,
        estimated_prompt_tokens: usize,
    ) -> Self {
        Self {
            kind,
            messages,
            system,
            tools,
            directive,
            estimated_prompt_tokens,
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
}

impl RunCapabilityEvidenceSource {
    pub(crate) fn from_agent(
        config: &AgentConfig,
        workspace_services: Arc<WorkspaceServices>,
        permission_checker_bound: bool,
        confirmation_manager_bound: bool,
    ) -> Self {
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
        }
    }

    pub(crate) fn capture(
        &self,
        call_sequence: u64,
        observation: ModelCallObservation<'_>,
    ) -> Result<(RunCapabilitySnapshotV1, ModelInputSnapshotV1), HarnessEvidenceError> {
        let tools_measurement = measure(MODEL_TOOLS_DOMAIN, observation.tools)?;
        let capability =
            self.capability_snapshot(observation.tools.len(), &tools_measurement.digest)?;
        let input = capture_model_input(ModelInputCapture {
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
        Ok((capability, input))
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
