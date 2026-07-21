//! Explainable four-level assessments built from the interactive lexical rules.

use super::{atomic_tool_is_bounded, classify_atomic_tool};
use crate::permissions::{
    EnvironmentSensitivity, ImpactScope, OperationTarget, PermissionDecision, Reversibility,
    ToolRiskAssessment, ToolRiskDimensions, ToolRiskLevel, ToolRiskReason, ToolRiskType,
};

const MAX_BATCH_RISK_DEPTH: usize = 16;

pub(super) fn assess_tool(tool_name: &str, args: &serde_json::Value) -> ToolRiskAssessment {
    assess_tool_at_depth(tool_name, args, 0)
}

fn assess_tool_at_depth(
    tool_name: &str,
    args: &serde_json::Value,
    depth: usize,
) -> ToolRiskAssessment {
    if tool_name.eq_ignore_ascii_case("batch") {
        return assess_batch(args, depth);
    }
    if !args.is_object() {
        return high_assessment(tool_name, ToolRiskReason::MalformedOrUnknownOperation);
    }

    match classify_atomic_tool(tool_name, args) {
        PermissionDecision::Allow => routine_assessment(tool_name),
        PermissionDecision::Ask if atomic_tool_is_bounded(tool_name, args) => {
            bounded_assessment(tool_name)
        }
        PermissionDecision::Ask => {
            let reason = match tool_risk_type(tool_name) {
                ToolRiskType::CommandExecution => ToolRiskReason::UnboundedCommandExecution,
                ToolRiskType::Delegation | ToolRiskType::DynamicExecution => {
                    ToolRiskReason::DynamicOrDelegatedExecution
                }
                ToolRiskType::VersionControl
                    if args.get("force").is_some_and(|value| value != false) =>
                {
                    ToolRiskReason::ForceOrDestructiveOperation
                }
                ToolRiskType::ExternalIntegration | ToolRiskType::Unknown => {
                    ToolRiskReason::ExternalOrUnknownIntegration
                }
                _ => ToolRiskReason::MalformedOrUnknownOperation,
            };
            high_assessment(tool_name, reason)
        }
        PermissionDecision::Deny => {
            let tool_type = tool_risk_type(tool_name);
            if tool_type == ToolRiskType::CommandExecution {
                critical_assessment(
                    tool_type,
                    OperationTarget::PrivilegedSystem,
                    ImpactScope::SystemWide,
                    Reversibility::Irreversible,
                    EnvironmentSensitivity::Privileged,
                    ToolRiskReason::CatastrophicOperation,
                )
            } else {
                critical_assessment(
                    tool_type,
                    OperationTarget::OutsideWorkspace,
                    ImpactScope::Host,
                    Reversibility::Unknown,
                    EnvironmentSensitivity::Host,
                    ToolRiskReason::WorkspaceBoundaryEscape,
                )
            }
        }
    }
}

fn assess_batch(args: &serde_json::Value, depth: usize) -> ToolRiskAssessment {
    let Some(invocations) = args
        .get("invocations")
        .and_then(serde_json::Value::as_array)
        .filter(|invocations| !invocations.is_empty())
    else {
        return malformed_batch_assessment();
    };
    if depth >= MAX_BATCH_RISK_DEPTH {
        return malformed_batch_assessment();
    }

    let mut highest: Option<ToolRiskAssessment> = None;
    let mut reasons = vec![ToolRiskReason::CompositeInvocation];
    for invocation in invocations {
        let Some(tool) = invocation.get("tool").and_then(serde_json::Value::as_str) else {
            return malformed_batch_assessment();
        };
        let Some(tool_args) = invocation.get("args").filter(|args| args.is_object()) else {
            return malformed_batch_assessment();
        };
        let child = assess_tool_at_depth(tool, tool_args, depth + 1);
        reasons.extend(child.reasons.iter().copied());
        if highest
            .as_ref()
            .is_none_or(|current| child.level > current.level)
        {
            highest = Some(child);
        }
    }

    let Some(highest) = highest else {
        return malformed_batch_assessment();
    };
    ToolRiskAssessment::new(
        highest.level,
        ToolRiskDimensions::new(
            ToolRiskType::Composite,
            OperationTarget::Multiple,
            highest.dimensions.impact_scope,
            highest.dimensions.reversibility,
            highest.dimensions.environment_sensitivity,
        ),
        reasons,
    )
}

fn malformed_batch_assessment() -> ToolRiskAssessment {
    ToolRiskAssessment::new(
        ToolRiskLevel::High,
        ToolRiskDimensions::new(
            ToolRiskType::Composite,
            OperationTarget::Multiple,
            ImpactScope::Unknown,
            Reversibility::Unknown,
            EnvironmentSensitivity::Unknown,
        ),
        [
            ToolRiskReason::CompositeInvocation,
            ToolRiskReason::MalformedOrUnknownOperation,
        ],
    )
}

pub(super) fn assessment_permission(assessment: &ToolRiskAssessment) -> PermissionDecision {
    match assessment.level {
        ToolRiskLevel::Routine => PermissionDecision::Allow,
        ToolRiskLevel::Bounded | ToolRiskLevel::High => PermissionDecision::Ask,
        ToolRiskLevel::Critical => PermissionDecision::Deny,
    }
}

fn routine_assessment(tool_name: &str) -> ToolRiskAssessment {
    let tool_type = tool_risk_type(tool_name);
    let (target, sensitivity) = match tool_type {
        ToolRiskType::NetworkRead => (
            OperationTarget::PublicNetwork,
            EnvironmentSensitivity::Network,
        ),
        ToolRiskType::CommandExecution => (
            OperationTarget::HostEnvironment,
            EnvironmentSensitivity::Host,
        ),
        _ => (
            OperationTarget::Workspace,
            EnvironmentSensitivity::Workspace,
        ),
    };
    ToolRiskAssessment::new(
        ToolRiskLevel::Routine,
        ToolRiskDimensions::new(
            tool_type,
            target,
            ImpactScope::Observation,
            Reversibility::NotApplicable,
            sensitivity,
        ),
        [ToolRiskReason::KnownReadOnly],
    )
}

fn bounded_assessment(tool_name: &str) -> ToolRiskAssessment {
    let tool_type = tool_risk_type(tool_name);
    if tool_name.eq_ignore_ascii_case("download") {
        return ToolRiskAssessment::new(
            ToolRiskLevel::Bounded,
            ToolRiskDimensions::new(
                tool_type,
                OperationTarget::Multiple,
                ImpactScope::Workspace,
                Reversibility::Easy,
                EnvironmentSensitivity::Mixed,
            ),
            [ToolRiskReason::BoundedWorkspaceMutation],
        );
    }
    let (reversibility, reason) = if tool_type == ToolRiskType::VersionControl {
        (
            Reversibility::Conditional,
            ToolRiskReason::BoundedVersionControlMutation,
        )
    } else {
        (
            Reversibility::Easy,
            ToolRiskReason::BoundedWorkspaceMutation,
        )
    };
    ToolRiskAssessment::new(
        ToolRiskLevel::Bounded,
        ToolRiskDimensions::new(
            tool_type,
            OperationTarget::Workspace,
            ImpactScope::Workspace,
            reversibility,
            EnvironmentSensitivity::Workspace,
        ),
        [reason],
    )
}

fn high_assessment(tool_name: &str, reason: ToolRiskReason) -> ToolRiskAssessment {
    let tool_type = tool_risk_type(tool_name);
    let (target, scope, sensitivity) = match tool_type {
        ToolRiskType::ExternalIntegration | ToolRiskType::Unknown => (
            OperationTarget::ExternalService,
            ImpactScope::ExternalService,
            EnvironmentSensitivity::Mixed,
        ),
        _ => (
            OperationTarget::HostEnvironment,
            ImpactScope::Host,
            EnvironmentSensitivity::Host,
        ),
    };
    ToolRiskAssessment::new(
        ToolRiskLevel::High,
        ToolRiskDimensions::new(
            tool_type,
            target,
            scope,
            Reversibility::Unknown,
            sensitivity,
        ),
        [reason],
    )
}

pub(super) fn critical_assessment(
    tool_type: ToolRiskType,
    operation_target: OperationTarget,
    impact_scope: ImpactScope,
    reversibility: Reversibility,
    environment_sensitivity: EnvironmentSensitivity,
    reason: ToolRiskReason,
) -> ToolRiskAssessment {
    ToolRiskAssessment::new(
        ToolRiskLevel::Critical,
        ToolRiskDimensions::new(
            tool_type,
            operation_target,
            impact_scope,
            reversibility,
            environment_sensitivity,
        ),
        [reason],
    )
}

pub(super) fn tool_risk_type(tool_name: &str) -> ToolRiskType {
    match tool_name.to_ascii_lowercase().as_str() {
        "read" | "grep" | "glob" | "ls" | "code_symbols" | "code_navigation"
        | "code_diagnostics" | "search_skills" | "generate_object" => ToolRiskType::ReadOnly,
        "write" | "edit" | "patch" | "download" => ToolRiskType::WorkspaceMutation,
        "bash" => ToolRiskType::CommandExecution,
        "git" => ToolRiskType::VersionControl,
        "web_search" | "web_fetch" => ToolRiskType::NetworkRead,
        "task" | "parallel_task" => ToolRiskType::Delegation,
        "program" | "dynamic_workflow" | "skill" => ToolRiskType::DynamicExecution,
        "runtime" => ToolRiskType::ExternalIntegration,
        "batch" => ToolRiskType::Composite,
        _ => ToolRiskType::Unknown,
    }
}
