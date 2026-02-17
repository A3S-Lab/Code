//! Cron tool - Manage cron jobs via a3s-cron

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use a3s_cron::CronManager;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct CronTool {
    /// Lazy-initialized CronManager per workspace
    manager: Arc<Mutex<Option<Arc<CronManager>>>>,
}

impl CronTool {
    pub fn new() -> Self {
        Self {
            manager: Arc::new(Mutex::new(None)),
        }
    }

    async fn get_manager(&self, ctx: &ToolContext) -> Result<Arc<CronManager>> {
        let mut guard = self.manager.lock().await;
        if let Some(ref manager) = *guard {
            return Ok(Arc::clone(manager));
        }
        let manager = Arc::new(CronManager::new(&ctx.workspace).await?);
        *guard = Some(Arc::clone(&manager));
        Ok(manager)
    }
}

#[async_trait]
impl Tool for CronTool {
    fn name(&self) -> &str {
        "cron"
    }

    fn description(&self) -> &str {
        "Manage cron jobs for scheduled task execution. Standard 5-field cron syntax (minute hour day month weekday). Natural language schedule support (English & Chinese). Task persistence and monitoring. CRUD operations: create, pause, update, terminate jobs. Execution history tracking."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "add", "get", "update", "pause", "resume", "remove", "history", "run", "parse"],
                    "description": "Action to perform"
                },
                "id": {
                    "type": "string",
                    "description": "Job ID (required for get, update, pause, resume, remove, history, run)"
                },
                "name": {
                    "type": "string",
                    "description": "Job name (required for add, optional for get)"
                },
                "schedule": {
                    "type": "string",
                    "description": "Schedule expression - supports cron syntax OR natural language like 'every 5 minutes', 'daily at 2am'"
                },
                "command": {
                    "type": "string",
                    "description": "Command to execute (required for add, optional for update)"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Execution timeout in milliseconds (default: 60000)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Number of history records to return (default: 10, for history action)"
                },
                "input": {
                    "type": "string",
                    "description": "Natural language input to parse (for parse action)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return Ok(ToolOutput::error("action parameter is required")),
        };

        // Handle parse action without needing CronManager
        if action == "parse" {
            let input = args
                .get("input")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if input.trim().is_empty() {
                return Ok(ToolOutput::error(
                    "input parameter is required for parse action",
                ));
            }

            return match a3s_cron::parse_natural(input) {
                Ok(expr) => Ok(ToolOutput::success(format!(
                    "Parsed \"{}\" -> cron expression: {}",
                    input, expr
                ))),
                Err(e) => Ok(ToolOutput::error(format!(
                    "Failed to parse schedule: {}",
                    e
                ))),
            };
        }

        let manager = match self.get_manager(ctx).await {
            Ok(m) => m,
            Err(e) => {
                return Ok(ToolOutput::error(format!(
                    "Failed to initialize cron manager: {}",
                    e
                )))
            }
        };

        match action {
            "list" => match manager.list_jobs().await {
                Ok(jobs) => {
                    if jobs.is_empty() {
                        Ok(ToolOutput::success("No cron jobs configured."))
                    } else {
                        let mut output = format!("{} cron job(s):\n\n", jobs.len());
                        for job in &jobs {
                            output.push_str(&format!(
                                "  ID: {}\n  Name: {}\n  Schedule: {}\n  Command: {}\n  Status: {:?}\n  Next run: {}\n\n",
                                job.id,
                                job.name,
                                job.schedule,
                                job.command,
                                job.status,
                                job.next_run.map(|t| t.to_string()).unwrap_or_else(|| "N/A".to_string()),
                            ));
                        }
                        Ok(ToolOutput::success(output))
                    }
                }
                Err(e) => Ok(ToolOutput::error(format!("Failed to list jobs: {}", e))),
            },

            "add" => {
                let name = match args.get("name").and_then(|v| v.as_str()) {
                    Some(n) => n,
                    None => {
                        return Ok(ToolOutput::error(
                            "name parameter is required for add action",
                        ))
                    }
                };

                let schedule = match args.get("schedule").and_then(|v| v.as_str()) {
                    Some(s) => s,
                    None => {
                        return Ok(ToolOutput::error(
                            "schedule parameter is required for add action",
                        ))
                    }
                };

                let command = match args.get("command").and_then(|v| v.as_str()) {
                    Some(c) => c,
                    None => {
                        return Ok(ToolOutput::error(
                            "command parameter is required for add action",
                        ))
                    }
                };

                match manager.add_job(name, schedule, command).await {
                    Ok(job) => Ok(ToolOutput::success(format!(
                        "Created cron job:\n  ID: {}\n  Name: {}\n  Schedule: {}\n  Command: {}\n  Next run: {}",
                        job.id,
                        job.name,
                        job.schedule,
                        job.command,
                        job.next_run.map(|t| t.to_string()).unwrap_or_else(|| "N/A".to_string()),
                    ))),
                    Err(e) => Ok(ToolOutput::error(format!("Failed to add job: {}", e))),
                }
            }

            "get" => {
                let id = args.get("id").and_then(|v| v.as_str());
                let name = args.get("name").and_then(|v| v.as_str());

                let job = if let Some(id) = id {
                    manager.get_job(id).await
                } else if let Some(name) = name {
                    manager.get_job_by_name(name).await
                } else {
                    return Ok(ToolOutput::error(
                        "id or name parameter is required for get action",
                    ));
                };

                match job {
                    Ok(Some(job)) => Ok(ToolOutput::success(format!(
                        "Cron job:\n  ID: {}\n  Name: {}\n  Schedule: {}\n  Command: {}\n  Status: {:?}\n  Created: {}\n  Last run: {}\n  Next run: {}\n  Run count: {}\n  Fail count: {}",
                        job.id,
                        job.name,
                        job.schedule,
                        job.command,
                        job.status,
                        job.created_at,
                        job.last_run.map(|t| t.to_string()).unwrap_or_else(|| "never".to_string()),
                        job.next_run.map(|t| t.to_string()).unwrap_or_else(|| "N/A".to_string()),
                        job.run_count,
                        job.fail_count,
                    ))),
                    Ok(None) => Ok(ToolOutput::error("Job not found")),
                    Err(e) => Ok(ToolOutput::error(format!("Failed to get job: {}", e))),
                }
            }

            "update" => {
                let id = match args.get("id").and_then(|v| v.as_str()) {
                    Some(i) => i,
                    None => {
                        return Ok(ToolOutput::error(
                            "id parameter is required for update action",
                        ))
                    }
                };

                let schedule = args.get("schedule").and_then(|v| v.as_str());
                let command = args.get("command").and_then(|v| v.as_str());
                let timeout = args.get("timeout").and_then(|v| v.as_u64());

                match manager.update_job(id, schedule, command, timeout).await {
                    Ok(job) => Ok(ToolOutput::success(format!(
                        "Updated cron job:\n  ID: {}\n  Name: {}\n  Schedule: {}\n  Command: {}",
                        job.id, job.name, job.schedule, job.command,
                    ))),
                    Err(e) => Ok(ToolOutput::error(format!("Failed to update job: {}", e))),
                }
            }

            "pause" => {
                let id = match args.get("id").and_then(|v| v.as_str()) {
                    Some(i) => i,
                    None => {
                        return Ok(ToolOutput::error(
                            "id parameter is required for pause action",
                        ))
                    }
                };

                match manager.pause_job(id).await {
                    Ok(job) => Ok(ToolOutput::success(format!(
                        "Paused job: {} ({})",
                        job.name, job.id
                    ))),
                    Err(e) => Ok(ToolOutput::error(format!("Failed to pause job: {}", e))),
                }
            }

            "resume" => {
                let id = match args.get("id").and_then(|v| v.as_str()) {
                    Some(i) => i,
                    None => {
                        return Ok(ToolOutput::error(
                            "id parameter is required for resume action",
                        ))
                    }
                };

                match manager.resume_job(id).await {
                    Ok(job) => Ok(ToolOutput::success(format!(
                        "Resumed job: {} ({})",
                        job.name, job.id
                    ))),
                    Err(e) => Ok(ToolOutput::error(format!("Failed to resume job: {}", e))),
                }
            }

            "remove" => {
                let id = match args.get("id").and_then(|v| v.as_str()) {
                    Some(i) => i,
                    None => {
                        return Ok(ToolOutput::error(
                            "id parameter is required for remove action",
                        ))
                    }
                };

                match manager.remove_job(id).await {
                    Ok(()) => Ok(ToolOutput::success(format!("Removed job: {}", id))),
                    Err(e) => Ok(ToolOutput::error(format!("Failed to remove job: {}", e))),
                }
            }

            "history" => {
                let id = match args.get("id").and_then(|v| v.as_str()) {
                    Some(i) => i,
                    None => {
                        return Ok(ToolOutput::error(
                            "id parameter is required for history action",
                        ))
                    }
                };

                let limit = args
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(10) as usize;

                match manager.get_history(id, limit).await {
                    Ok(history) => {
                        if history.is_empty() {
                            Ok(ToolOutput::success("No execution history."))
                        } else {
                            let mut output = format!(
                                "{} execution(s) for job {}:\n\n",
                                history.len(),
                                id
                            );
                            for exec in &history {
                                output.push_str(&format!(
                                    "  Execution: {}\n  Status: {:?}\n  Started: {}\n  Duration: {}ms\n  Exit code: {}\n\n",
                                    exec.id,
                                    exec.status,
                                    exec.started_at,
                                    exec.duration_ms.unwrap_or(0),
                                    exec.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "N/A".to_string()),
                                ));
                            }
                            Ok(ToolOutput::success(output))
                        }
                    }
                    Err(e) => Ok(ToolOutput::error(format!(
                        "Failed to get history: {}",
                        e
                    ))),
                }
            }

            "run" => {
                let id = match args.get("id").and_then(|v| v.as_str()) {
                    Some(i) => i,
                    None => {
                        return Ok(ToolOutput::error(
                            "id parameter is required for run action",
                        ))
                    }
                };

                match manager.run_job(id).await {
                    Ok(exec) => Ok(ToolOutput::success(format!(
                        "Job executed:\n  Status: {:?}\n  Duration: {}ms\n  Exit code: {}\n  Output: {}",
                        exec.status,
                        exec.duration_ms.unwrap_or(0),
                        exec.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "N/A".to_string()),
                        if exec.stdout.is_empty() { &exec.stderr } else { &exec.stdout },
                    ))),
                    Err(e) => Ok(ToolOutput::error(format!("Failed to run job: {}", e))),
                }
            }

            _ => Ok(ToolOutput::error(format!("Unknown action: {}", action))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_cron_tool_name() {
        let tool = CronTool::new();
        assert_eq!(tool.name(), "cron");
    }

    #[test]
    fn test_cron_tool_parameters() {
        let tool = CronTool::new();
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["action"].is_object());
    }

    #[tokio::test]
    async fn test_cron_missing_action() {
        let tool = CronTool::new();
        let ctx = ToolContext::new(PathBuf::from("/tmp"));

        let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();
        assert!(!result.success);
        assert!(result.content.contains("action"));
    }

    #[tokio::test]
    async fn test_cron_parse_natural_language() {
        let tool = CronTool::new();
        let ctx = ToolContext::new(PathBuf::from("/tmp"));

        let result = tool
            .execute(
                &serde_json::json!({"action": "parse", "input": "every 5 minutes"}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("*/5"));
    }

    #[tokio::test]
    async fn test_cron_parse_missing_input() {
        let tool = CronTool::new();
        let ctx = ToolContext::new(PathBuf::from("/tmp"));

        let result = tool
            .execute(&serde_json::json!({"action": "parse"}), &ctx)
            .await
            .unwrap();

        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_cron_list_empty() {
        let temp = tempfile::tempdir().unwrap();
        let tool = CronTool::new();
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(&serde_json::json!({"action": "list"}), &ctx)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("No cron jobs"));
    }

    #[tokio::test]
    async fn test_cron_add_missing_params() {
        let temp = tempfile::tempdir().unwrap();
        let tool = CronTool::new();
        let ctx = ToolContext::new(temp.path().to_path_buf());

        // Missing name
        let result = tool
            .execute(
                &serde_json::json!({"action": "add", "schedule": "* * * * *", "command": "echo"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!result.success);

        // Missing schedule
        let result = tool
            .execute(
                &serde_json::json!({"action": "add", "name": "test", "command": "echo"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!result.success);

        // Missing command
        let result = tool
            .execute(
                &serde_json::json!({"action": "add", "name": "test", "schedule": "* * * * *"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!result.success);
    }
}
