//! The ACP permission ask for one tool call, with the reason it needs approval.

use super::{prepared_raw_input, tool_kind_for, tool_title};
use a3s_code_core::permissions::{
    InteractiveApprovalMode, InteractiveToolGuardrail, ToolRiskAssessment, ToolRiskLevel,
    ToolRiskReason,
};
use agent_client_protocol as acp;
use std::path::Path;

pub(super) fn tool_permission_ask(
    session_id: &acp::SessionId,
    tool_id: &str,
    tool_name: &str,
    args: &serde_json::Value,
    workspace: Option<&Path>,
) -> acp::RequestPermissionRequest {
    let raw_input = prepared_raw_input(tool_name, args);
    let why = approval_reason(&assess(tool_name, args, workspace));
    let mut fields = acp::ToolCallUpdateFields::new()
        .title(tool_title(tool_name))
        .kind(tool_kind_for(tool_name))
        .status(acp::ToolCallStatus::Pending)
        .content(vec![acp::ToolCallContent::from(acp::ContentBlock::Text(
            acp::TextContent::new(why),
        ))]);
    if let Some(raw_input) = raw_input {
        fields = fields.raw_input(raw_input);
    }
    let mut options = vec![acp::PermissionOption::new(
        acp::PermissionOptionId::new("allow-once"),
        "Yes, proceed",
        acp::PermissionOptionKind::AllowOnce,
    )];
    let learnable = (tool_name == "bash")
        .then(|| args.get("command").and_then(serde_json::Value::as_str))
        .flatten()
        .and_then(crate::approval_learning::learnable_rule);
    if let Some(rule) = learnable {
        options.push(acp::PermissionOption::new(
            acp::PermissionOptionId::new("allow-always"),
            format!("Yes, and don't ask again for `{rule}` in this workspace"),
            acp::PermissionOptionKind::AllowAlways,
        ));
    }
    options.push(acp::PermissionOption::new(
        acp::PermissionOptionId::new("reject-once"),
        "No",
        acp::PermissionOptionKind::RejectOnce,
    ));
    acp::RequestPermissionRequest::new(
        session_id.clone(),
        acp::ToolCallUpdate::new(acp::ToolCallId::new(tool_id.to_string()), fields),
        options,
    )
}

#[cfg(test)]
pub(super) fn tool_permission_request(
    session_id: &acp::SessionId,
    tool_id: &str,
    tool_name: &str,
    args: &serde_json::Value,
) -> acp::RequestPermissionRequest {
    tool_permission_ask(session_id, tool_id, tool_name, args, None)
}

fn assess(
    tool_name: &str,
    args: &serde_json::Value,
    workspace: Option<&Path>,
) -> ToolRiskAssessment {
    match workspace {
        Some(root) => InteractiveToolGuardrail::new(InteractiveApprovalMode::Default)
            .with_workspace(root)
            .assess(tool_name, args),
        None => InteractiveToolGuardrail::risk_assessment(tool_name, args),
    }
}

/// One sentence naming the risk level and every reason code behind it.
pub(super) fn approval_reason(assessment: &ToolRiskAssessment) -> String {
    let reasons = assessment
        .reasons
        .iter()
        .map(|reason| reason_text(*reason))
        .collect::<Vec<_>>();
    let because = if reasons.is_empty() {
        "the session's permission policy asks before this call".to_string()
    } else {
        reasons.join("; ")
    };
    format!(
        "Needs approval ({} risk): {because}.",
        level_text(assessment.level)
    )
}

fn level_text(level: ToolRiskLevel) -> &'static str {
    match level {
        ToolRiskLevel::Routine => "routine",
        ToolRiskLevel::Bounded => "bounded",
        ToolRiskLevel::High => "high",
        ToolRiskLevel::Critical => "critical",
    }
}

fn reason_text(reason: ToolRiskReason) -> &'static str {
    match reason {
        ToolRiskReason::KnownReadOnly => "it only reads",
        ToolRiskReason::BoundedWorkspaceMutation => "it changes files inside the workspace",
        ToolRiskReason::BoundedVersionControlMutation => "it changes version-control state",
        ToolRiskReason::UnboundedCommandExecution => {
            "it runs a command whose effects are not bounded to the workspace"
        }
        ToolRiskReason::DynamicOrDelegatedExecution => "it runs dynamic or delegated code",
        ToolRiskReason::ExternalOrUnknownIntegration => {
            "it calls an external or unknown integration"
        }
        ToolRiskReason::EnvironmentSensitiveOperation => {
            "it touches network, credential, or host state"
        }
        ToolRiskReason::ForceOrDestructiveOperation => "it forces or destroys data",
        ToolRiskReason::MalformedOrUnknownOperation => "the operation could not be classified",
        ToolRiskReason::WorkspaceBoundaryEscape => "it reaches outside the workspace",
        ToolRiskReason::SymlinkBoundaryEscape => "it follows a symlink outside the workspace",
        ToolRiskReason::CatastrophicOperation => "it matches the catastrophic-operation floor",
        ToolRiskReason::CompositeInvocation => "it combines several operations",
    }
}
