use super::*;

const PARALLEL_TASK_TOOL_DESCRIPTION: &str = "REMOVED from the model-visible registry (`HARNESS-CONV4`). Prefer `task` with multiple `tasks[]` items. This type remains for focused unit tests that construct ParallelTaskTool directly.";

/// ParallelTaskTool allows the LLM to fan out multiple delegated tasks concurrently.
///
/// All tasks execute in parallel and the tool returns when all complete.
pub struct ParallelTaskTool {
    executor: Arc<TaskExecutor>,
}

impl ParallelTaskTool {
    /// Create a new ParallelTaskTool
    pub fn new(executor: Arc<TaskExecutor>) -> Self {
        Self { executor }
    }

    pub(super) async fn execute_params(
        &self,
        params: ParallelTaskParams,
        ctx: &ToolContext,
        tool_name: &str,
        min_tasks: usize,
    ) -> Result<ToolOutput> {
        let started_at = std::time::Instant::now();
        let parent_cancellation = ctx.cancellation_token();
        let executor = self.executor.scoped_for_invocation(ctx);

        if params.tasks.len() < min_tasks {
            return Ok(invalid_delegation_argument(format!(
                "{tool_name} requires at least {min_tasks} task{}",
                if min_tasks == 1 { "" } else { "s" }
            )));
        }
        if params.tasks.len() > MAX_PARALLEL_TASKS_PER_CALL {
            return Ok(invalid_delegation_argument(format!(
                "{tool_name} accepts at most {MAX_PARALLEL_TASKS_PER_CALL} tasks"
            )));
        }
        if let Some((index, _)) = params
            .tasks
            .iter()
            .enumerate()
            .find(|(_, task)| task.background)
        {
            return Ok(invalid_delegation_argument(format!(
                "{tool_name} task {} cannot set background=true when fan-out options are used or multiple tasks are submitted; every branch is collected by the parent call",
                index + 1
            )));
        }
        if params.timeout_ms == Some(0) {
            return Ok(invalid_delegation_argument(format!(
                "{tool_name} timeout_ms must be at least 1"
            )));
        }
        if let Some(min_success_count) = params.min_success_count {
            if !params.allow_partial_failure {
                return Ok(invalid_delegation_argument(format!(
                    "{tool_name} min_success_count requires allow_partial_failure=true"
                )));
            }
            if min_success_count == 0 || min_success_count > params.tasks.len() {
                return Ok(invalid_delegation_argument(format!(
                    "{tool_name} min_success_count must be between 1 and the task count ({})",
                    params.tasks.len()
                )));
            }
        }

        let task_count = params.tasks.len();
        let run = executor
            .execute_parallel_for_tool(
                params.tasks.clone(),
                ctx.agent_event_tx.clone(),
                parallel_execution::ParallelToolOptions {
                    parent_session_id: ctx.session_id.as_deref(),
                    timeout_ms: params.timeout_ms,
                    min_success_count: params.min_success_count,
                    allow_partial_failure: params.allow_partial_failure,
                    parent_cancellation: Some(&parent_cancellation),
                },
            )
            .await;
        let results = run.results;

        let mut output = format!("Executed {} tasks concurrently:\n\n", task_count);
        let mut metadata_results = Vec::new();
        let source_anchor_counts = parallel_source_anchor_counts(&results);
        for (i, result) in results.iter().enumerate() {
            let status = if result.success { "[OK]" } else { "[ERR]" };
            let (formatted, truncated) = format_task_result_for_context(result);
            let (output_excerpt, _) = compact_task_output(&result.output);
            let source_anchors = &result.source_anchors[..source_anchor_counts[i]];
            metadata_results.push(serde_json::json!({
                "task_id": result.task_id,
                "session_id": result.session_id,
                "agent": result.agent,
                "success": result.success,
                "error_message": (!result.success).then(|| {
                    crate::text::truncate_utf8(&result.output, 1024).to_string()
                }),
                "output_excerpt": output_excerpt,
                "structured": result.structured,
                "source_anchors": source_anchors,
                "output_bytes": result.output.len(),
                "truncated_for_context": truncated,
                "artifact_id": task_artifact_id(result),
                "artifact_uri": task_artifact_uri(result),
            }));
            output.push_str(&format!(
                "--- Task {} ({}) {} ---\n{}\n\n",
                i + 1,
                result.agent,
                status,
                formatted
            ));
        }

        let success_count = results.iter().filter(|result| result.success).count();
        let failed_count = results.len().saturating_sub(success_count);
        let all_success = failed_count == 0;
        let partial_failure = failed_count > 0 && success_count > 0;
        if params.allow_partial_failure && partial_failure {
            output.push_str(&format!(
                "Partial failure tolerated: {success_count} succeeded, {failed_count} failed.\n"
            ));
        }
        if run.timed_out {
            output.push_str(&format!(
                "Task fan-out timed out after {} ms; returned completed child results and marked unfinished children failed.\n",
                run.timeout_ms.unwrap_or_default()
            ));
        } else if run.returned_early {
            output.push_str(&format!(
                "Task fan-out returned after reaching min_success_count={}; unfinished children were marked failed.\n",
                run.min_success_count.unwrap_or_default()
            ));
        }

        let tool_success = all_success || (params.allow_partial_failure && success_count > 0);
        let mut output = if tool_success {
            ToolOutput::success(output)
        } else {
            ToolOutput::error(output)
        };
        if !tool_success && failed_count > 0 {
            output.error_kind = Some(crate::tools::ToolErrorKind::PartialFailure {
                failed: failed_count,
                total: results.len(),
            });
        }

        Ok(output.with_metadata(serde_json::json!({
            "task_count": task_count,
            "result_count": results.len(),
            "success_count": success_count,
            "failed_count": failed_count,
            "all_success": all_success,
            "partial_failure": partial_failure,
            "allow_partial_failure": params.allow_partial_failure,
            "timeout_ms": params.timeout_ms,
            "timed_out": run.timed_out,
            "min_success_count": params.min_success_count,
            "returned_early": run.returned_early,
            "duration_ms": started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            "results": metadata_results,
        })))
    }
}

#[async_trait]
impl Tool for ParallelTaskTool {
    fn name(&self) -> &str {
        "parallel_task"
    }

    fn description(&self) -> &str {
        PARALLEL_TASK_TOOL_DESCRIPTION
    }

    fn parameters(&self) -> serde_json::Value {
        parallel_params::parallel_task_params_schema_for_agents(&self.executor.visible_agents())
    }

    fn definition(&self) -> ToolDefinition {
        let agents = self.executor.visible_agents();
        ToolDefinition {
            name: self.name().to_string(),
            description: delegation_tool_description(self.description(), &agents),
            parameters: parallel_params::parallel_task_params_schema_for_agents(&agents),
        }
    }

    fn is_model_visible(&self) -> bool {
        false
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let params: ParallelTaskParams = match serde_json::from_value(args.clone()) {
            Ok(params) => params,
            Err(error) => {
                return Ok(invalid_delegation_argument(format!(
                    "Invalid parallel_task parameters: {error}"
                )));
            }
        };
        self.execute_params(params, ctx, "parallel_task", 2).await
    }
}
