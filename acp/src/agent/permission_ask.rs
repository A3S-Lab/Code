//! The ACP permission ask for one tool call, with the reason it needs approval.

use super::*;
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

impl A3sCodeAgent {
    /// Ask the ACP client (pager) whether a tool may run. Fail closed when no client is wired.
    /// A pager allow is a confirmation fact. Core still projects the admitted policy, and a deny is not executed.
    pub(super) async fn request_tool_permission(
        &self,
        session_id: &acp::SessionId,
        tool_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
        workspace: &std::path::Path,
    ) -> bool {
        // The unattended launch is the operator's standing approval. It must
        // not learn a prefix into the graded workspace either.
        if self.unattended.is_some() {
            return true;
        }
        // A rule the user chose "don't ask again" for in this workspace
        // skips the round-trip; see approval_learning for what can match.
        if crate::approval_learning::is_previously_approved(workspace, tool_name, args) {
            tracing::debug!(tool = tool_name, "tool call approved by a learned rule");
            return true;
        }
        let request = permission_ask::tool_permission_ask(
            session_id,
            tool_id,
            tool_name,
            args,
            Some(workspace),
        );
        let client = self.client.borrow().clone();
        let Some(client) = client else {
            return false;
        };
        let ask = client.request_permission(request);
        let response = match tokio::time::timeout(ask_deadline(), ask).await {
            Ok(result) => result,
            Err(_elapsed) => {
                // Fail closed: an unanswered ask denies the call instead of
                // parking the turn forever. The bell rang; nobody came.
                tracing::warn!(
                    session = %session_id.0,
                    tool = tool_name,
                    "permission ask timed out unanswered; denying"
                );
                return false;
            }
        };
        match response {
            Ok(response) => {
                let approved = permission_response_approved(&response);
                if approved && permission_response_always(&response) && tool_name == "bash" {
                    let command = args
                        .get("command")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default();
                    match crate::approval_learning::record_always_allowed_bash(workspace, command) {
                        Ok(Some(rule)) => tracing::info!(
                            rule = %rule,
                            "learned a bash rule for this workspace"
                        ),
                        Ok(None) => {}
                        Err(error) => tracing::warn!(
                            %error,
                            "could not persist the allow-always rule; it applies to this call only"
                        ),
                    }
                }
                approved
            }
            Err(_) => false,
        }
    }

    /// Ask the same ACP permission channel for a structured answer.
    ///
    /// Options use `RejectOnce` so a YOLO `AllowOnce` drain cannot answer.
    /// Free text with no options stays unanswered; the run remains parked.
    pub(super) async fn request_question_answer(
        &self,
        session_id: &acp::SessionId,
        question_id: &str,
        question: &str,
        options: &[String],
        allow_free_text: bool,
    ) -> Option<String> {
        let request = question_permission_request(
            session_id,
            question_id,
            question,
            options,
            allow_free_text,
        )?;
        let client = self.client.borrow().clone();
        let client = client?;
        let ask = client.request_permission(request);
        let response = match tokio::time::timeout(ask_deadline(), ask).await {
            Ok(result) => result.ok()?,
            Err(_elapsed) => {
                // Unanswered: the run keeps its existing unanswered-question
                // behavior instead of parking on the wire forever.
                tracing::warn!(
                    session = %session_id.0,
                    question_id = %question_id,
                    "question ask timed out unanswered"
                );
                return None;
            }
        };
        // A typed free-text answer rides the followup meta and outranks the
        // choice-row mapping: the selected row only names how the dialog was
        // dismissed, not what the user answered.
        if let Some(text) = followup_message(&response) {
            return Some(text.to_string());
        }
        answer_from_permission_outcome(&response.outcome, options)
    }
}

/// Whether a `request_permission` response grants the tool.
pub(super) fn permission_response_approved(response: &acp::RequestPermissionResponse) -> bool {
    match &response.outcome {
        acp::RequestPermissionOutcome::Selected(selected) => {
            // YOLO picks the first AllowOnce (`allow-once`). RejectOnce / cancel stay false.
            matches!(selected.option_id.0.as_ref(), "allow-once" | "allow-always")
        }
        acp::RequestPermissionOutcome::Cancelled => false,
        _ => false,
    }
}

/// Whether the user answered "don't ask again". Only this answer learns.
pub(super) fn permission_response_always(response: &acp::RequestPermissionResponse) -> bool {
    matches!(
        &response.outcome,
        acp::RequestPermissionOutcome::Selected(selected)
            if selected.option_id.0.as_ref() == "allow-always"
    )
}

/// The free text a user typed into a permission dialog's followup input; the
/// pager attaches it as `followup_message` response meta (its
/// `dispatch_permission_followup` path). Trimmed; empty/absent yields `None`.
pub(super) fn followup_message(response: &acp::RequestPermissionResponse) -> Option<&str> {
    response
        .meta
        .as_ref()
        .and_then(|meta| meta.get("followup_message"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

/// Question choices are not permission grants. `RejectOnce` keeps YOLO from
/// selecting them via its `AllowOnce` drain.
pub(super) fn question_permission_options(options: &[String]) -> Vec<acp::PermissionOption> {
    options
        .iter()
        .enumerate()
        .filter_map(|(index, option)| {
            let text = option.trim();
            if text.is_empty() {
                return None;
            }
            Some(acp::PermissionOption::new(
                acp::PermissionOptionId::new(format!("question-{index}")),
                text.to_string(),
                acp::PermissionOptionKind::RejectOnce,
            ))
        })
        .collect()
}

pub(super) fn claim_question_prompt(
    seen: &mut std::collections::HashSet<String>,
    question_id: &str,
) -> bool {
    seen.insert(question_id.to_string())
}

pub(super) fn question_permission_request(
    session_id: &acp::SessionId,
    question_id: &str,
    question: &str,
    options: &[String],
    allow_free_text: bool,
) -> Option<acp::RequestPermissionRequest> {
    let mut choices = question_permission_options(options);
    if choices.is_empty() {
        // A free-text-only ask still needs a dialog to type into: one row
        // that carries no answer of its own (its index maps past the empty
        // options list in `answer_from_permission_outcome`, so a bare Enter
        // resolves the dialog without fabricating an answer).
        if !allow_free_text {
            return None;
        }
        choices = vec![acp::PermissionOption::new(
            acp::PermissionOptionId::new("question-0"),
            "Answer in text".to_owned(),
            acp::PermissionOptionKind::RejectOnce,
        )];
    }
    Some(
        acp::RequestPermissionRequest::new(
            session_id.clone(),
            acp::ToolCallUpdate::new(
                acp::ToolCallId::new(question_id.to_string()),
                acp::ToolCallUpdateFields::new()
                    .title(question.to_string())
                    .kind(acp::ToolKind::Other)
                    .status(acp::ToolCallStatus::Pending),
            ),
            choices,
        )
        // The marker lets a client render choices by name (they all ride the
        // RejectOnce kind) and offer the free-text row only when allowed.
        .meta(
            serde_json::json!({
                QUESTION_META_KEY: {
                    "allowFreeText": allow_free_text,
                    "hasChoices": question_has_choices(options),
                },
            })
            .as_object()
            .cloned(),
        ),
    )
}

/// A choice opens the permission dialog. Blank options do not.
pub(super) fn question_has_choices(options: &[String]) -> bool {
    options.iter().any(|option| !option.trim().is_empty())
}

/// How long an `request_permission` ask may park before the agent fails
/// closed. Default 30 minutes (the same budget `ask_user` gives an
/// unanswered questionnaire); `A3S_CODE_PERMISSION_ASK_TIMEOUT_SECS`
/// overrides, `0` disables the bound entirely.
pub(super) fn ask_deadline() -> std::time::Duration {
    const DEFAULT_SECS: u64 = 1800;
    let configured = std::env::var("A3S_CODE_PERMISSION_ASK_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok());
    let secs = match configured {
        // Explicit disable: no bound.
        Some(0) => u64::MAX,
        Some(secs) => secs,
        None => DEFAULT_SECS,
    };
    std::time::Duration::from_secs(secs)
}

pub(super) fn answer_from_permission_outcome(
    outcome: &acp::RequestPermissionOutcome,
    options: &[String],
) -> Option<String> {
    let acp::RequestPermissionOutcome::Selected(selected) = outcome else {
        return None;
    };
    let index = selected
        .option_id
        .0
        .as_ref()
        .strip_prefix("question-")?
        .parse::<usize>()
        .ok()?;
    let text = options.get(index)?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}
