use super::AgentLoop;
use crate::hooks::{
    ErrorType, HookEvent, HookResult, OnErrorEvent, PlanningStrategy, PostPlanningEvent,
    PostResponseEvent, PostToolUseEvent, PrePlanningEvent, PreToolUseEvent, TokenUsageInfo,
    ToolResultData,
};
use crate::llm::TokenUsage;
use crate::planning::ExecutionPlan;
use anyhow::{bail, Result};
use std::sync::Arc;

impl AgentLoop {
    /// Fire PreToolUse hook event before tool execution.
    /// Returns the HookResult which may block the tool call.
    pub(super) async fn fire_pre_tool_use(
        &self,
        session_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
        recent_tools: Vec<String>,
    ) -> Option<HookResult> {
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
            let result = he.fire(&event).await;
            if result.is_block() {
                return Some(result);
            }
        }
        None
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
            let he = Arc::clone(he);
            tokio::spawn(async move {
                let _ = he.fire(&event).await;
            });
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

        match he.fire(&event).await {
            HookResult::Block(reason) => bail!("Planning blocked by hook: {reason}"),
            HookResult::Retry(delay_ms) => {
                bail!("Planning hook requested retry after {delay_ms}ms")
            }
            HookResult::Escalate { reason, target } => {
                let target = target
                    .as_deref()
                    .map(|value| format!(" for {value}"))
                    .unwrap_or_default();
                bail!("Planning hook escalated{target}: {reason}")
            }
            HookResult::Continue(modified) => {
                Ok(apply_pre_planning_modifications(task_description, modified))
            }
            HookResult::Skip => Ok(task_description.to_string()),
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
            let he = Arc::clone(he);
            tokio::spawn(async move {
                let _ = he.fire(&event).await;
            });
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
            let he = Arc::clone(he);
            tokio::spawn(async move {
                let _ = he.fire(&event).await;
            });
        }
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
