use super::AgentLoop;
use crate::hooks::{
    ErrorType, HookEvent, HookOutcome, OnErrorEvent, PermissionRequestEvent, PlanningStrategy,
    PostCompactEvent, PostPlanningEvent, PostResponseEvent, PostToolUseEvent, PreCompactEvent,
    PrePlanningEvent, PreToolUseEvent, TokenUsageInfo, ToolResultData,
};
use crate::llm::TokenUsage;
use crate::planning::ExecutionPlan;
use crate::safety_gate::HookDenialFeedback;
use anyhow::{bail, Result};
use std::sync::Arc;

pub(super) struct PreToolHookDecision {
    pub(super) denial: Option<HookDenialFeedback>,
    pub(super) updated_args: Option<serde_json::Value>,
}

pub(super) enum PermissionHookDecision {
    Continue,
    Allow,
    Deny(String),
}

impl AgentLoop {
    /// Fire PreToolUse hook event before tool execution.
    /// Returns structured denial feedback when a hook gates the tool call.
    pub(super) async fn fire_pre_tool_use(
        &self,
        session_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
        recent_tools: Vec<String>,
    ) -> PreToolHookDecision {
        if let Some(he) = &self.config.hook_engine {
            // Convert null args to empty object so JS callbacks don't get null.input errors
            let safe_args = if args.is_null() {
                serde_json::Value::Object(Default::default())
            } else {
                args.clone()
            };
            let event = HookEvent::PreToolUse(PreToolUseEvent {
                session_id: session_id.to_string(),
                tool: tool_name.to_string(),
                args: safe_args,
                working_directory: self.tool_context.workspace.to_string_lossy().to_string(),
                recent_tools,
            });
            return normalize_pre_tool_gate_outcome(he.fire_outcome(&event).await);
        }
        PreToolHookDecision {
            denial: None,
            updated_args: None,
        }
    }

    /// Fire PermissionRequest immediately before host confirmation is shown.
    pub(super) async fn fire_permission_request(
        &self,
        session_id: &str,
        tool_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> PermissionHookDecision {
        let Some(he) = &self.config.hook_engine else {
            return PermissionHookDecision::Continue;
        };
        let event = HookEvent::PermissionRequest(PermissionRequestEvent {
            session_id: session_id.to_string(),
            tool_id: tool_id.to_string(),
            tool: tool_name.to_string(),
            args: args.clone(),
        });
        normalize_permission_outcome(he.fire_outcome(&event).await)
    }

    pub(super) async fn fire_pre_compact(
        &self,
        session_id: &str,
        message_count: usize,
        used_tokens: usize,
        max_tokens: usize,
    ) -> Result<()> {
        let Some(he) = &self.config.hook_engine else {
            return Ok(());
        };
        let event = HookEvent::PreCompact(PreCompactEvent {
            session_id: session_id.to_string(),
            message_count,
            used_tokens,
            max_tokens,
        });
        match he.fire_outcome(&event).await {
            HookOutcome::Block { reason } => bail!("Context compaction blocked by hook: {reason}"),
            HookOutcome::Retry { reason, .. } => {
                bail!("Context compaction deferred by hook: {reason}")
            }
            HookOutcome::Escalate { reason, .. } => {
                bail!("Context compaction requires review: {reason}")
            }
            HookOutcome::Continue(_) | HookOutcome::Skip => Ok(()),
        }
    }

    pub(super) async fn fire_post_compact(
        &self,
        session_id: &str,
        message_count_before: usize,
        message_count_after: usize,
        summary_generated: bool,
    ) {
        let Some(he) = &self.config.hook_engine else {
            return;
        };
        let event = HookEvent::PostCompact(PostCompactEvent {
            session_id: session_id.to_string(),
            message_count_before,
            message_count_after,
            summary_generated,
        });
        let _ = he.fire(&event).await;
    }

    /// Fire PostToolUse hook event after tool execution (fire-and-forget).
    pub(super) async fn fire_post_tool_use(
        &self,
        session_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
        output: &str,
        success: bool,
        duration_ms: u64,
    ) {
        if let Some(he) = &self.config.hook_engine {
            // Convert null args to empty object so JS callbacks don't get null.input errors
            let safe_args = if args.is_null() {
                serde_json::Value::Object(Default::default())
            } else {
                args.clone()
            };
            let event = HookEvent::PostToolUse(PostToolUseEvent {
                session_id: session_id.to_string(),
                tool: tool_name.to_string(),
                args: safe_args,
                result: ToolResultData {
                    success,
                    output: output.to_string(),
                    exit_code: if success { Some(0) } else { Some(1) },
                    duration_ms,
                },
            });
            Arc::clone(he).dispatch_observational(event);
        }
    }

    /// Fire PrePlanning hook before plan generation.
    ///
    /// This is a blocking hook point: policy engines can stop planning before
    /// PlanningStart is emitted and before the planning phase chooses a plan.
    pub(super) async fn fire_pre_planning(
        &self,
        session_id: &str,
        task_description: &str,
    ) -> Result<String> {
        let Some(he) = &self.config.hook_engine else {
            return Ok(task_description.to_string());
        };

        let event = HookEvent::PrePlanning(PrePlanningEvent {
            session_id: session_id.to_string(),
            task_description: task_description.to_string(),
            available_strategies: vec![
                PlanningStrategy::StepByStep,
                PlanningStrategy::GraphPlanning,
            ],
            constraints: Some(serde_json::json!({
                "goal_tracking": self.config.goal_tracking,
                "max_parallel_tasks": self.config.max_parallel_tasks,
                "available_tools": self
                    .tool_executor
                    .definitions()
                    .iter()
                    .map(|tool| tool.name.clone())
                    .collect::<Vec<_>>(),
            })),
        });

        match he.fire_outcome(&event).await {
            HookOutcome::Block { reason } => bail!("Planning blocked by hook: {reason}"),
            HookOutcome::Retry {
                reason,
                retry_after_ms,
            } => {
                bail!("Planning hook requested retry after {retry_after_ms}ms: {reason}")
            }
            HookOutcome::Escalate { reason, target } => {
                let target = target
                    .as_deref()
                    .map(|value| format!(" for {value}"))
                    .unwrap_or_default();
                bail!("Planning hook escalated{target}: {reason}")
            }
            HookOutcome::Continue(modified) => {
                Ok(apply_pre_planning_modifications(task_description, modified))
            }
            HookOutcome::Skip => Ok(task_description.to_string()),
        }
    }

    /// Fire PostPlanning hook after a plan has been generated or failed.
    pub(super) async fn fire_post_planning(
        &self,
        session_id: &str,
        task_description: &str,
        plan: Option<&ExecutionPlan>,
        error: Option<&str>,
    ) {
        if let Some(he) = &self.config.hook_engine {
            let event = HookEvent::PostPlanning(PostPlanningEvent {
                session_id: session_id.to_string(),
                task_description: task_description.to_string(),
                strategy_used: plan
                    .map(Self::planning_strategy_for_plan)
                    .unwrap_or(PlanningStrategy::None),
                subtasks: plan
                    .map(|plan| {
                        plan.steps
                            .iter()
                            .map(|step| step.content.clone())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                success: error.is_none(),
                error: error.map(str::to_string),
            });
            let _ = he.fire(&event).await;
        }
    }

    fn planning_strategy_for_plan(plan: &ExecutionPlan) -> PlanningStrategy {
        if plan.steps.iter().any(|step| !step.dependencies.is_empty()) {
            PlanningStrategy::GraphPlanning
        } else {
            PlanningStrategy::StepByStep
        }
    }

    /// Fire PostResponse hook event after the agent loop completes.
    pub(super) async fn fire_post_response(
        &self,
        session_id: &str,
        response_text: &str,
        tool_calls_count: usize,
        usage: &TokenUsage,
        duration_ms: u64,
    ) {
        if let Some(he) = &self.config.hook_engine {
            let event = HookEvent::PostResponse(PostResponseEvent {
                session_id: session_id.to_string(),
                response_text: response_text.to_string(),
                tool_calls_count,
                usage: TokenUsageInfo {
                    prompt_tokens: usage.prompt_tokens as i32,
                    completion_tokens: usage.completion_tokens as i32,
                    total_tokens: usage.total_tokens as i32,
                },
                duration_ms,
            });
            Arc::clone(he).dispatch_observational(event);
        }
    }

    /// Fire OnError hook event when an error occurs.
    pub(super) async fn fire_on_error(
        &self,
        session_id: &str,
        error_type: ErrorType,
        error_message: &str,
        context: serde_json::Value,
    ) {
        if let Some(he) = &self.config.hook_engine {
            let event = HookEvent::OnError(OnErrorEvent {
                session_id: session_id.to_string(),
                error_type,
                error_message: error_message.to_string(),
                context,
            });
            Arc::clone(he).dispatch_observational(event);
        }
    }
}

/// Normalize hook outcomes to the decisions supported by the tool gateway.
///
/// The gateway has no retry scheduler or human-escalation wait state. Allowing
/// either outcome to fall through would execute the protected tool without the
/// requested policy action, so both fail closed until such a path exists.
fn normalize_pre_tool_gate_outcome(outcome: HookOutcome) -> PreToolHookDecision {
    match outcome {
        HookOutcome::Continue(modified) => PreToolHookDecision {
            denial: None,
            updated_args: modified.and_then(extract_updated_tool_input),
        },
        HookOutcome::Skip => PreToolHookDecision {
            denial: None,
            updated_args: None,
        },
        HookOutcome::Block { reason } => PreToolHookDecision {
            denial: Some(HookDenialFeedback::blocked(reason)),
            updated_args: None,
        },
        HookOutcome::Retry {
            reason,
            retry_after_ms,
        } => PreToolHookDecision {
            denial: Some(HookDenialFeedback::retry(reason, retry_after_ms)),
            updated_args: None,
        },
        HookOutcome::Escalate { reason, target } => {
            let target = target
                .as_deref()
                .map(|value| format!(" to {value}"))
                .unwrap_or_default();
            PreToolHookDecision {
                denial: Some(HookDenialFeedback::blocked(format!(
                    "Pre-tool hook escalated{target}: {reason}"
                ))),
                updated_args: None,
            }
        }
    }
}

fn extract_updated_tool_input(modified: serde_json::Value) -> Option<serde_json::Value> {
    let output = modified.get("hookSpecificOutput").unwrap_or(&modified);
    if let Some(value) = output
        .get("updatedInput")
        .or_else(|| output.get("updated_input"))
        .or_else(|| output.get("args"))
    {
        return Some(value.clone());
    }
    if output.get("additionalContext").is_some()
        || output.get("additional_context").is_some()
        || output.get("decision").is_some()
        || output.get("permissionDecision").is_some()
    {
        return None;
    }
    Some(output.clone())
}

fn normalize_permission_outcome(outcome: HookOutcome) -> PermissionHookDecision {
    match outcome {
        HookOutcome::Continue(Some(value)) => {
            let output = value.get("hookSpecificOutput").unwrap_or(&value);
            let decision = output
                .get("decision")
                .or_else(|| output.get("permissionDecision"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let reason = output
                .get("reason")
                .or_else(|| output.get("permissionDecisionReason"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Permission hook denied the tool")
                .to_string();
            match decision.to_ascii_lowercase().as_str() {
                "allow" | "approve" => PermissionHookDecision::Allow,
                "deny" | "reject" | "block" => PermissionHookDecision::Deny(reason),
                _ => PermissionHookDecision::Continue,
            }
        }
        HookOutcome::Block { reason }
        | HookOutcome::Retry { reason, .. }
        | HookOutcome::Escalate { reason, .. } => PermissionHookDecision::Deny(reason),
        HookOutcome::Continue(None) | HookOutcome::Skip => PermissionHookDecision::Continue,
    }
}

fn apply_pre_planning_modifications(
    task_description: &str,
    modified: Option<serde_json::Value>,
) -> String {
    let Some(modified) = modified else {
        return task_description.to_string();
    };
    let Some(object) = modified.as_object() else {
        return task_description.to_string();
    };

    let task = first_string(object, &["modified_task", "task_description", "prompt"])
        .unwrap_or_else(|| task_description.to_string());
    let task = task.trim();
    let original = task_description.trim();

    let mut sections = Vec::new();
    if !task.is_empty() && task != original {
        sections.push(format!("Original user request:\n{original}"));
        sections.push(format!("Hook-modified planning task:\n{task}"));
    } else {
        sections.push(task_description.to_string());
    }

    let mut guidance = Vec::new();
    if let Some(strategy) = first_string(object, &["selected_strategy"]) {
        guidance.push(format!("Selected strategy: {strategy}"));
    }
    if let Some(template) = first_string(object, &["planning_template"]) {
        guidance.push(format!("Planning template:\n{template}"));
    }
    if let Some(hints) = planning_hints(object.get("hints")) {
        guidance.push(format!("Hints:\n{hints}"));
    }

    if !guidance.is_empty() {
        sections.push(format!(
            "Planning hook guidance:\n{}",
            guidance.join("\n\n")
        ));
    }

    sections.join("\n\n")
}

fn first_string(
    object: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<String> {
    keys.iter().find_map(|key| {
        object
            .get(*key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn planning_hints(value: Option<&serde_json::Value>) -> Option<String> {
    match value? {
        serde_json::Value::String(value) if !value.trim().is_empty() => {
            Some(format!("- {}", value.trim()))
        }
        serde_json::Value::Array(values) => {
            let hints = values
                .iter()
                .filter_map(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| format!("- {value}"))
                .collect::<Vec<_>>();
            (!hints.is_empty()).then(|| hints.join("\n"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_pre_tool_gate_outcome;
    use crate::hooks::HookOutcome;

    #[test]
    fn pre_tool_retry_preserves_reason_and_retryability() {
        let result = normalize_pre_tool_gate_outcome(HookOutcome::Retry {
            reason: "temporary policy outage".to_string(),
            retry_after_ms: 250,
        })
        .denial
        .unwrap();

        assert_eq!(result.reason, "temporary policy outage");
        assert!(result.retryable);
        assert_eq!(result.retry_after_ms, Some(250));
    }

    #[test]
    fn pre_tool_escalation_fails_closed_without_escalation_runtime() {
        let result = normalize_pre_tool_gate_outcome(HookOutcome::Escalate {
            reason: "approval required".to_string(),
            target: Some("security-team".to_string()),
        })
        .denial
        .unwrap();

        assert!(result.reason.contains("security-team"));
        assert!(result.reason.contains("approval required"));
        assert!(!result.retryable);
        assert_eq!(result.retry_after_ms, None);
    }

    #[test]
    fn explicit_pre_tool_continue_and_skip_remain_non_blocking() {
        assert!(normalize_pre_tool_gate_outcome(HookOutcome::Continue(None))
            .denial
            .is_none());
        assert!(normalize_pre_tool_gate_outcome(HookOutcome::Skip)
            .denial
            .is_none());
    }

    #[test]
    fn pre_tool_rewrite_accepts_codex_hook_specific_output() {
        let decision =
            normalize_pre_tool_gate_outcome(HookOutcome::Continue(Some(serde_json::json!({
                "hookSpecificOutput": {
                    "updatedInput": {"path": "safe.txt"}
                }
            }))));

        assert!(decision.denial.is_none());
        assert_eq!(
            decision.updated_args,
            Some(serde_json::json!({"path": "safe.txt"}))
        );
    }
}
