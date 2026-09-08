//! Live checklist / plan updates for the host UI (Codex-compatible schema).
//!
//! Replaces the session task list and emits [`AgentEvent::TaskUpdated`] so
//! hosts that pin a plan panel (and session persistence) stay in sync.

use crate::agent::AgentEvent;
use crate::planning::{Task, TaskStatus};
use crate::tools::types::{Tool, ToolCapabilities, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;

/// Keep presentation and persistence bounded (aligned with CLI projection).
const MAX_PLAN_TASKS: usize = 256;
const MAX_STEP_CHARS: usize = 480;

pub struct UpdatePlanTool;

#[async_trait]
impl Tool for UpdatePlanTool {
    fn name(&self) -> &str {
        "update_plan"
    }

    fn description(&self) -> &str {
        "Replace the live task checklist shown to the user. Pass the full plan \
         array on every call. Use for multi-step work: create 2–8 concrete steps \
         early, keep exactly one in_progress, and mark steps completed as you finish. \
         Skip for trivial single-step requests."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "plan": {
                    "type": "array",
                    "description": "Required. Complete ordered checklist. Each call replaces the previous list.",
                    "minItems": 1,
                    "maxItems": MAX_PLAN_TASKS,
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "step": {
                                "type": "string",
                                "description": "Required. Short actionable step label."
                            },
                            "status": {
                                "type": "string",
                                "enum": [
                                    "pending",
                                    "in_progress",
                                    "completed",
                                    "failed",
                                    "skipped",
                                    "cancelled"
                                ],
                                "description": "Required. Step status. Prefer exactly one in_progress while working."
                            },
                            "id": {
                                "type": "string",
                                "description": "Optional stable id. Defaults to plan-{index}."
                            }
                        },
                        "required": ["step", "status"]
                    }
                }
            },
            "required": ["plan"],
            "examples": [
                {
                    "plan": [
                        {"step": "Inspect render helpers", "status": "completed", "id": "1"},
                        {"step": "Wire pagination into explore_detail", "status": "in_progress", "id": "2"},
                        {"step": "Add tests and docs", "status": "pending", "id": "3"}
                    ]
                }
            ]
        })
    }

    fn capabilities(&self, _args: &serde_json::Value) -> ToolCapabilities {
        ToolCapabilities {
            read_only: true,
            idempotent: true,
            resumable: false,
            cancellation_safe: true,
            supports_pagination: false,
            max_parallelism: 1,
            output_kind: crate::tools::ToolOutputKind::Text,
        }
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let tasks = match parse_plan_args(args) {
            Ok(tasks) => tasks,
            Err(message) => return Ok(ToolOutput::error(message)),
        };

        let session_id = ctx.session_id.clone().unwrap_or_default();
        if let Some(tx) = ctx.agent_event_tx.as_ref() {
            let _ = tx.send(AgentEvent::TaskUpdated {
                session_id,
                tasks: tasks.clone(),
            });
        }

        Ok(ToolOutput::success(summarize_plan(&tasks)))
    }
}

fn parse_plan_args(args: &serde_json::Value) -> Result<Vec<Task>, String> {
    let rows = args
        .get("plan")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "plan must be a non-empty array".to_string())?;
    if rows.is_empty() {
        return Err("plan must contain at least one step".to_string());
    }
    if rows.len() > MAX_PLAN_TASKS {
        return Err(format!(
            "plan has {} steps; maximum is {MAX_PLAN_TASKS}",
            rows.len()
        ));
    }

    let mut tasks = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let step = row
            .get("step")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|step| !step.is_empty())
            .ok_or_else(|| format!("plan[{index}].step must be a non-empty string"))?;
        let content = truncate_chars(step, MAX_STEP_CHARS);
        let status = row
            .get("status")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("plan[{index}].status is required"))
            .and_then(parse_status)?;
        let id = row
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("plan-{}", index + 1));
        let mut task = Task::new(id, content);
        task.status = status;
        tasks.push(task);
    }
    Ok(tasks)
}

fn parse_status(status: &str) -> Result<TaskStatus, String> {
    match status.trim().to_ascii_lowercase().as_str() {
        "pending" => Ok(TaskStatus::Pending),
        "in_progress" | "in-progress" | "active" => Ok(TaskStatus::InProgress),
        "completed" | "complete" | "done" => Ok(TaskStatus::Completed),
        "failed" | "error" => Ok(TaskStatus::Failed),
        "skipped" => Ok(TaskStatus::Skipped),
        "cancelled" | "canceled" => Ok(TaskStatus::Cancelled),
        other => Err(format!("unknown plan status: {other}")),
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (count, ch) in value.chars().enumerate() {
        if count >= max_chars {
            break;
        }
        out.push(ch);
    }
    out
}

fn summarize_plan(tasks: &[Task]) -> String {
    let total = tasks.len();
    let completed = tasks
        .iter()
        .filter(|task| task.status == TaskStatus::Completed)
        .count();
    let active = tasks
        .iter()
        .find(|task| task.status == TaskStatus::InProgress)
        .map(|task| task.content.as_str());
    match active {
        Some(step) => format!("Updated plan: {completed}/{total} done · in progress: {step}"),
        None => format!("Updated plan: {completed}/{total} done"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;

    #[tokio::test]
    async fn update_plan_emits_task_updated_and_summarizes() {
        let (tx, mut rx) = broadcast::channel(8);
        let ctx = ToolContext::new(std::path::PathBuf::from("/tmp"))
            .with_session_id("session-1")
            .with_agent_event_tx(tx);

        let args = serde_json::json!({
            "plan": [
                {"step": "First", "status": "completed", "id": "a"},
                {"step": "Second", "status": "in_progress"},
                {"step": "Third", "status": "pending"}
            ]
        });
        let output = UpdatePlanTool.execute(&args, &ctx).await.unwrap();
        assert!(output.success);
        assert!(output.content.contains("1/3 done"));
        assert!(output.content.contains("Second"));

        let event = rx.try_recv().expect("TaskUpdated event");
        match event {
            AgentEvent::TaskUpdated { session_id, tasks } => {
                assert_eq!(session_id, "session-1");
                assert_eq!(tasks.len(), 3);
                assert_eq!(tasks[0].id, "a");
                assert_eq!(tasks[0].status, TaskStatus::Completed);
                assert_eq!(tasks[1].id, "plan-2");
                assert_eq!(tasks[1].status, TaskStatus::InProgress);
                assert_eq!(tasks[2].status, TaskStatus::Pending);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn update_plan_rejects_empty_step_and_unknown_status() {
        let ctx = ToolContext::new(std::path::PathBuf::from("/tmp"));
        let empty = UpdatePlanTool
            .execute(
                &serde_json::json!({"plan": [{"step": "  ", "status": "pending"}]}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!empty.success);

        let bad_status = UpdatePlanTool
            .execute(
                &serde_json::json!({"plan": [{"step": "x", "status": "doing"}]}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!bad_status.success);
        assert!(bad_status.content.contains("unknown plan status"));
    }

    #[test]
    fn parse_plan_args_bounds_length() {
        let rows: Vec<_> = (0..MAX_PLAN_TASKS + 1)
            .map(|i| serde_json::json!({"step": format!("s{i}"), "status": "pending"}))
            .collect();
        let err = parse_plan_args(&serde_json::json!({ "plan": rows })).unwrap_err();
        assert!(err.contains("maximum"));
    }
}
