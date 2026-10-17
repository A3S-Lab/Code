//! Tool results as facts, tool admission, and the model decision for one response.

use super::*;

pub(super) const FACT_TOOL_RESULT_KIND: &str = "a3s.fact_tool_result";

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct FactToolResultEnvelope {
    pub(super) kind: String,
    pub(super) text: String,
    pub(super) is_error: bool,
    #[serde(default)]
    pub(super) images: Vec<FactToolImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) outcome: Option<a3s_effect::ToolExecutionOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) workspace_child: Option<String>,
    #[serde(default)]
    pub(super) observation_incomplete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) metadata: Option<serde_json::Value>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct FactToolImage {
    pub(super) media_type: String,
    pub(super) data: String,
}

pub(crate) fn exposed_tool_specs(
    catalog: &[ToolDefinition],
    expose: impl Fn(&str) -> bool,
) -> Vec<ToolSpec> {
    catalog
        .iter()
        .filter(|tool| expose(&tool.name))
        .map(|tool| ToolSpec {
            name: tool.name.clone(),
            description: tool.description.clone(),
        })
        .collect()
}

pub(super) fn fact_tool_runner_value(
    output: &str,
    is_error: bool,
    images: &[crate::llm::Attachment],
) -> serde_json::Value {
    if !is_error && images.is_empty() {
        return serde_json::Value::String(output.to_string());
    }
    let envelope = FactToolResultEnvelope {
        kind: FACT_TOOL_RESULT_KIND.to_string(),
        text: output.to_string(),
        is_error,
        images: images
            .iter()
            .map(|image| FactToolImage {
                media_type: image.media_type.clone(),
                data: base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &image.data,
                ),
            })
            .collect(),
        exit_code: None,
        outcome: None,
        workspace_child: None,
        observation_incomplete: false,
        metadata: None,
    };
    match serde_json::to_string(&envelope) {
        Ok(encoded) => serde_json::Value::String(encoded),
        Err(_) => serde_json::Value::String(output.to_string()),
    }
}

pub(super) fn decode_fact_tool_result(body: &str) -> Option<FactToolResultEnvelope> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    if value.get("kind").and_then(|kind| kind.as_str()) != Some(FACT_TOOL_RESULT_KIND) {
        return None;
    }
    serde_json::from_value(value).ok()
}

pub(super) fn message_from_folded_tool(id: &str, body: &str, folded_error: bool) -> Message {
    let Some(envelope) = decode_fact_tool_result(body) else {
        return Message::tool_result(id, body, folded_error);
    };
    let is_error = folded_error || envelope.is_error;
    let images = envelope
        .images
        .iter()
        .filter_map(|image| {
            let bytes = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                image.data.as_bytes(),
            )
            .ok()?;
            if bytes.is_empty() {
                return None;
            }
            Some(crate::llm::Attachment::new(bytes, image.media_type.clone()))
        })
        .collect::<Vec<_>>();
    if images.is_empty() {
        Message::tool_result(id, &envelope.text, is_error)
    } else {
        Message::tool_result_with_images(id, &envelope.text, &images, is_error)
    }
}

pub(super) fn definitions_for(
    catalog: &[ToolDefinition],
    specs: &[ToolSpec],
) -> Vec<ToolDefinition> {
    specs
        .iter()
        .map(|spec| {
            catalog
                .iter()
                .find(|tool| tool.name == spec.name)
                .cloned()
                .unwrap_or(ToolDefinition {
                    name: spec.name.clone(),
                    description: spec.description.clone(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "additionalProperties": true
                    }),
                })
        })
        .collect()
}

/// Eight tool results since the latest `user\n` line, with an empty ledger,
/// end evidence gathering for that request. A later user line starts a new
/// count, so one finished read-only turn does not disarm the next command.
/// A recorded mutation or an open workspace child keeps tools available.
/// The fact log counts results, which is its tool-round unit. Name-blind:
/// every tool result counts, including `update_plan`.
#[cfg(test)]
pub(super) fn read_only_evidence_exhausted(
    ledger: &crate::harness_loop::MutationLedger,
    messages: &[String],
) -> bool {
    if !ledger.is_empty() || ledger.has_open_children() {
        return false;
    }
    // Only the `user\n` prefix is a new human request.
    let results = messages
        .iter()
        .rev()
        .take_while(|line| !line.starts_with("user\n"))
        .filter(|line| {
            let (role, _) = split_folded_message(line);
            role == "tool" || role == "tool-error"
        })
        .count();
    results >= usize::try_from(crate::agent::READ_ONLY_EVIDENCE_LIMIT).unwrap_or(usize::MAX)
}

/// Name-aware variant used by the fact-path completion. Plan management
/// (`update_plan`) is not evidence gathering: a long-horizon choreography
/// that plans first and reads many files before its first edit is
/// legitimate work, not an aimless loop, so its results do not count toward
/// the ceiling. The positional pairing mirrors the wire-to-message
/// conversion above.
pub(super) fn read_only_evidence_exhausted_with_calls(
    ledger: &crate::harness_loop::MutationLedger,
    messages: &[String],
    recorded: &mut std::vec::IntoIter<RecordedCall>,
) -> bool {
    if !ledger.is_empty() || ledger.has_open_children() {
        return false;
    }
    // Only the `user\n` prefix is a new human request.
    let tail_start = messages
        .iter()
        .rposition(|line| line.starts_with("user\n"))
        .map(|index| index + 1)
        .unwrap_or(0);
    let mut results = 0usize;
    let mut total = 0usize;
    let mut seen = std::collections::HashSet::new();
    for (index, line) in messages.iter().enumerate() {
        let (role, _) = split_folded_message(line);
        if role != "tool" && role != "tool-error" {
            continue;
        }
        // Consume in wire order so the positional pairing matches the
        // conversion; only results in the current request's tail count.
        let call = recorded.next();
        let name = call
            .as_ref()
            .map(|call| call.name.clone())
            .unwrap_or_default();
        if index < tail_start || name == "update_plan" {
            continue;
        }
        // "No further effect" is per DISTINCT evidence: a repeated call
        // (same tool, same arguments) re-reads bytes the log already
        // holds, so it must not consume the ceiling. Churn terminates on
        // the distinct count; genuine multi-hop exploration gets the
        // room its hop count needs.
        total += 1;
        let fingerprint = match &call {
            Some(call) => crate::content_digest::digest_bytes(
                "a3s.code.read-only-evidence.v1",
                format!("{}\n{}", call.name, call.args).as_bytes(),
            ),
            None => String::new(),
        };
        if !seen.insert(fingerprint) {
            continue;
        }
        results += 1;
    }
    let limit = usize::try_from(crate::agent::READ_ONLY_EVIDENCE_LIMIT).unwrap_or(usize::MAX);
    // Distinct evidence consumes the ceiling; total churn bounds the
    // pathological alternator (two alternating distinct calls would
    // otherwise never trip the distinct limit).
    results >= limit || total >= limit * 2
}

/// Once the latest folded message is a verified tool result, that completion
/// does not need another provider call. A later `user.message` still does.
/// A narrative allow still calls the model so the answer can use the tool output.
pub(super) fn verified_turn_text(surface: &SessionSurface, messages: &[String]) -> Option<String> {
    if surface.agent.requires_assistant_final_response() {
        // Verification authorizes completion of effects, not delivery of
        // the user's requested answer. The final model response still goes
        // through the same identity/effect/completion gates below.
        return None;
    }
    let last_is_tool = messages
        .last()
        .is_some_and(|line| split_folded_message(line).0 == "tool");
    if !last_is_tool {
        return None;
    }
    let ledger = surface
        .ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let reports = surface
        .reports
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    match surface.agent.fact_completion_gate(&ledger, &reports) {
        crate::harness_loop::CompletionGate::Allow(
            crate::harness_loop::CompletionTerminal::Verified { .. }
            | crate::harness_loop::CompletionTerminal::Waived { .. },
        ) => Some("completed".into()),
        _ => None,
    }
}

pub(super) fn latest_user_text(messages: &[String]) -> String {
    // Only `user\n` lines are the human turn. Assistant and tool lines share
    // the log, and treating them as user text would switch the reply language
    // to whatever language the model just wrote.
    messages
        .iter()
        .rev()
        .find_map(|text| text.strip_prefix("user\n").map(str::to_owned))
        .unwrap_or_default()
}

pub(super) fn split_folded_message(text: &str) -> (&str, String) {
    if let Some(body) = text.strip_prefix("user\n") {
        ("user", body.to_string())
    } else if let Some(body) = text.strip_prefix("tool-error\n") {
        ("tool-error", body.to_string())
    } else if let Some(body) = text.strip_prefix("tool\n") {
        ("tool", body.to_string())
    } else if let Some(body) = text.strip_prefix("assistant\n") {
        ("assistant", body.to_string())
    } else {
        ("user", text.to_string())
    }
}

pub(super) fn question_from_call(call: &crate::llm::ToolCall) -> ModelDecision {
    let question = call
        .args
        .get("question")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let options = call
        .args
        .get("options")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let allow_free_text = call
        .args
        .get("allow_free_text")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    ModelDecision::Question {
        question_id: if call.id.is_empty() {
            "ask".into()
        } else {
            call.id.clone()
        },
        question,
        allow_free_text,
        options,
    }
}

/// Project one tool through the admitted policy. Deny does not run `execute`.
///
/// Ask does not run it either: the fact log parks until a confirmation fact.
/// Allow is the only decision that calls `execute`.
pub fn admit_tool_call<T>(
    policy: &PermissionPolicy,
    tool_name: &str,
    args: &serde_json::Value,
    execute: impl FnOnce() -> T,
) -> std::result::Result<T, PermissionDecision> {
    let decision = execution_permission(policy, None, tool_name, args);
    if decision != PermissionDecision::Allow {
        return Err(decision);
    }
    Ok(execute())
}

/// Allow, deny, or ask for one tool call.
///
/// A policy deny is final. A policy allow, including a YOLO lane, runs the
/// tool. When the policy asks, the checker may still deny (a plan-mode
/// guardrail over a default policy) or quiet a read. It must not quiet a
/// command or file change: 逐次确认 and 智能授权 stay on Ask for those.
pub(super) fn execution_permission(
    policy: &PermissionPolicy,
    checker: Option<&dyn crate::permissions::PermissionChecker>,
    name: &str,
    args: &serde_json::Value,
) -> PermissionDecision {
    let policy_decision = policy.check(name, args);
    if policy_decision == PermissionDecision::Deny {
        return PermissionDecision::Deny;
    }
    let Some(checker) = checker else {
        return policy_decision;
    };
    if policy_decision == PermissionDecision::Allow {
        return PermissionDecision::Allow;
    }
    match checker.check(name, args) {
        PermissionDecision::Deny => PermissionDecision::Deny,
        PermissionDecision::Allow if !side_effect_tool(name) => PermissionDecision::Allow,
        PermissionDecision::Allow | PermissionDecision::Ask => PermissionDecision::Ask,
    }
}

/// Commands and file changes. A guardrail that allows "routine" calls must
/// not skip the confirmation the posture already asked for.
pub(super) fn side_effect_tool(name: &str) -> bool {
    matches!(
        name,
        "bash"
            | "shell"
            | "execute"
            | "write"
            | "edit"
            | "multiedit"
            | "delete"
            | "move"
            | "download"
            | "apply_patch"
            | "task"
            | "parallel_task"
    )
}

pub(super) fn skill_denial(
    agent: &crate::agent::AgentLoop,
    name: &str,
) -> Option<(String, String)> {
    agent.skill_restriction_denial(name)
}

pub(super) async fn confirmation_parked(
    policy: &PermissionPolicy,
    surface: Option<&SessionSurface>,
    name: &str,
    args: &serde_json::Value,
) -> bool {
    if let Some(surface) = surface {
        if skill_denial(&surface.agent, name).is_some() {
            return false;
        }
    }
    let checker = surface.and_then(|surface| surface.agent.permission_checker());
    let checker = checker.as_deref();
    if execution_permission(policy, checker, name, args) != PermissionDecision::Ask {
        return false;
    }
    // A command the sandbox will not start cannot be authorized by waiting.
    if surface
        .and_then(|surface| surface.agent.execution_unavailable(name, args))
        .is_some()
    {
        return false;
    }
    match surface.and_then(|surface| surface.confirmation.as_ref()) {
        Some(manager) => manager.requires_confirmation_for(name, args).await,
        // A bare fact run still parks on Ask. A session with no confirmation
        // manager has HITL disabled and runs the tool.
        None => surface.is_none(),
    }
}

/// Structured calls win. A text-only turn can still be a tool call when the
/// provider wrote the invocation as DSML or another leaked protocol, including
/// a reasoning channel with no normal content.
pub(super) fn leaked_model_calls(
    response: &LlmResponse,
) -> Vec<(String, String, serde_json::Value)> {
    let mut calls = crate::llm::recover_leaked_tool_calls(&response.text());
    if calls.is_empty() {
        if let Some(reasoning) = response
            .message
            .reasoning_content
            .as_deref()
            .filter(|text| !text.is_empty())
        {
            calls = crate::llm::recover_leaked_tool_calls(reasoning);
        }
    }
    calls
}

pub(super) async fn decision_from_response(
    response: &LlmResponse,
    policy: &PermissionPolicy,
    surface: Option<&SessionSurface>,
) -> ModelDecision {
    let structured: Vec<(String, String, serde_json::Value)> = response
        .tool_calls()
        .into_iter()
        .map(|call| (call.id, call.name, call.args))
        .collect();
    let calls = if structured.is_empty() {
        leaked_model_calls(response)
    } else {
        structured
    };
    if calls.iter().all(|call| call.1 == "ask_user") {
        if let Some((id, name, args)) = calls.first() {
            return question_from_call(&crate::llm::ToolCall {
                id: id.clone(),
                name: name.clone(),
                args: args.clone(),
            });
        }
    }
    let text = crate::llm::strip_leaked_tool_protocol(&response.text());
    let text = if text.trim().is_empty() {
        None
    } else {
        Some(text)
    };
    let reasoning = response
        .message
        .reasoning_content
        .as_ref()
        .map(|text| crate::llm::strip_leaked_tool_protocol(text))
        .filter(|text| !text.trim().is_empty());
    let mut tools = Vec::new();
    for (id, name, args) in calls {
        if name == "ask_user" {
            continue;
        }
        let needs_confirmation = confirmation_parked(policy, surface, &name, &args).await;
        tools.push(ToolCall {
            id: if id.is_empty() { "tool".into() } else { id },
            needs_confirmation,
            name,
            args,
            text: text.clone(),
            reasoning: reasoning.clone(),
        });
    }
    if let Some(call) = tools.first().cloned() {
        let also = tools.into_iter().skip(1).collect();
        return ModelDecision::Tool { call, also };
    }
    // A length-limited final response is a truncated answer, not a finished
    // one: flag it so the settled End names the cut. Exact members of the
    // closed wire set only ("length", "max_tokens") — no substring tuning.
    if let Some(surface) = surface.filter(|_| {
        matches!(
            response.stop_reason.as_deref(),
            Some("length") | Some("max_tokens")
        )
    }) {
        *surface
            .forced_stop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(crate::harness_loop::ForcedStop::ResponseTruncated);
    }
    ModelDecision::Text {
        text: response.text(),
    }
}
