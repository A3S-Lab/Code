use super::*;
use std::collections::HashMap;

pub(super) struct ParallelToolOptions<'a> {
    pub(super) parent_session_id: Option<&'a str>,
    pub(super) timeout_ms: Option<u64>,
    pub(super) min_success_count: Option<usize>,
    pub(super) allow_partial_failure: bool,
    pub(super) parent_cancellation: Option<&'a CancellationToken>,
}

impl TaskExecutor {
    /// Execute multiple tasks in parallel.
    ///
    /// Spawns all tasks concurrently and waits for all to complete.
    /// Returns results in the same order as the input tasks. Routed through
    /// the [`AgentExecutor`] seam so the
    /// same fan-out works whether steps run locally (default) or are placed
    /// on remote nodes by a host.
    pub async fn execute_parallel(
        self: &Arc<Self>,
        tasks: Vec<TaskParams>,
        event_tx: Option<broadcast::Sender<AgentEvent>>,
        parent_session_id: Option<&str>,
    ) -> Vec<TaskResult> {
        self.execute_parallel_with_parent_cancellation(
            tasks,
            event_tx,
            parent_session_id,
            self.parent_cancellation.as_ref(),
        )
        .await
    }

    async fn execute_parallel_with_parent_cancellation(
        self: &Arc<Self>,
        tasks: Vec<TaskParams>,
        event_tx: Option<broadcast::Sender<AgentEvent>>,
        parent_session_id: Option<&str>,
        parent_cancellation: Option<&CancellationToken>,
    ) -> Vec<TaskResult> {
        let parent = parent_session_id.map(|s| s.to_string());
        let specs = tasks
            .into_iter()
            .map(|params| AgentStepSpec {
                task_id: format!("task-{}", uuid::Uuid::new_v4()),
                agent: params.agent,
                description: params.description,
                prompt: params.prompt,
                max_steps: params.max_steps,
                parent_session_id: parent.clone(),
                output_schema: params.output_schema,
            })
            .collect();

        let executor: Arc<dyn AgentExecutor> = match parent_cancellation {
            Some(cancellation) => Arc::new(ScopedTaskExecutor {
                executor: Arc::clone(self),
                parent_cancellation: cancellation.clone(),
            }),
            None => Arc::<Self>::clone(self),
        };
        crate::orchestration::execute_steps_parallel(executor, specs, event_tx)
            .await
            .into_iter()
            .map(TaskResult::from)
            .collect()
    }

    pub(super) async fn execute_parallel_for_tool(
        self: &Arc<Self>,
        tasks: Vec<TaskParams>,
        event_tx: Option<broadcast::Sender<AgentEvent>>,
        options: ParallelToolOptions<'_>,
    ) -> ParallelTaskRun {
        let ParallelToolOptions {
            parent_session_id,
            timeout_ms,
            min_success_count,
            allow_partial_failure,
            parent_cancellation,
        } = options;
        let parallel_cancellation = parent_cancellation
            .map(CancellationToken::child_token)
            .unwrap_or_default();
        let should_return_early = allow_partial_failure && min_success_count.is_some();
        if timeout_ms.is_none() && !should_return_early {
            return ParallelTaskRun {
                results: self
                    .execute_parallel_with_parent_cancellation(
                        tasks,
                        event_tx,
                        parent_session_id,
                        Some(&parallel_cancellation),
                    )
                    .await,
                timed_out: false,
                returned_early: false,
                timeout_ms: None,
                min_success_count: None,
            };
        }

        let task_count = tasks.len();
        let parent = parent_session_id.map(ToString::to_string);
        let specs = tasks
            .into_iter()
            .map(|params| AgentStepSpec {
                task_id: format!("task-{}", uuid::Uuid::new_v4()),
                agent: params.agent,
                description: params.description,
                prompt: params.prompt,
                max_steps: params.max_steps,
                parent_session_id: parent.clone(),
                output_schema: params.output_schema,
            })
            .collect::<Vec<_>>();
        let labels = specs
            .iter()
            .map(|spec| (spec.task_id.clone(), spec.agent.clone()))
            .collect::<Vec<_>>();
        let target_successes = min_success_count
            .unwrap_or(task_count)
            .clamp(1, task_count.max(1));

        let max_concurrency = self.max_parallel_tasks.max(1);
        let scoped_executor: Arc<dyn AgentExecutor> = Arc::new(ScopedTaskExecutor {
            executor: Arc::clone(self),
            parent_cancellation: parallel_cancellation.clone(),
        });
        let mut pending = specs.into_iter().enumerate();
        let mut join_set = JoinSet::new();
        let mut active_indexes = HashMap::new();
        let mut active_count = 0usize;
        while active_count < max_concurrency {
            let Some((index, spec)) = pending.next() else {
                break;
            };
            let task_id = spawn_parallel_task_step(
                &mut join_set,
                Arc::clone(&scoped_executor),
                event_tx.clone(),
                index,
                spec,
            );
            active_indexes.insert(task_id, index);
            active_count += 1;
        }

        let mut results: Vec<Option<TaskResult>> = vec![None; task_count];
        let mut completed_count = 0usize;
        let mut success_count = 0usize;
        let mut timed_out = false;
        let mut returned_early = false;
        let deadline = timeout_ms.map(|timeout| {
            tokio::time::Instant::now() + std::time::Duration::from_millis(timeout.max(1))
        });

        while completed_count < task_count {
            if should_return_early && success_count >= target_successes {
                returned_early = true;
                break;
            }

            let next = match deadline {
                Some(deadline) => {
                    tokio::select! {
                        result = join_set.join_next_with_id() => result,
                        _ = tokio::time::sleep_until(deadline) => {
                            timed_out = true;
                            break;
                        }
                    }
                }
                None => join_set.join_next_with_id().await,
            };

            let Some(joined) = next else {
                break;
            };
            active_count = active_count.saturating_sub(1);
            let (index, outcome) = match joined {
                Ok((task_id, (reported_index, Ok(outcome)))) => {
                    let index = take_parallel_task_index(&mut active_indexes, task_id)
                        .unwrap_or(reported_index);
                    if index != reported_index {
                        tracing::error!(
                            tracked_index = index,
                            reported_index,
                            "parallel branch returned a mismatched task index"
                        );
                    }
                    (index, outcome)
                }
                Ok((task_id, (reported_index, Err(error)))) => {
                    let index = take_parallel_task_index(&mut active_indexes, task_id)
                        .unwrap_or(reported_index);
                    let (task_id, agent) = labels
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));
                    (index, StepOutcome::failed(task_id, agent, error))
                }
                Err(error) => {
                    let index = take_parallel_task_index(&mut active_indexes, error.id())
                        .unwrap_or_else(|| {
                        tracing::error!(%error, "parallel branch join failed without a tracked index");
                        usize::MAX
                    });
                    let (task_id, agent) = labels
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));
                    (
                        index,
                        StepOutcome::failed(task_id, agent, error.to_string()),
                    )
                }
            };
            let accepted = index < task_count && results[index].is_none();
            if accepted {
                if outcome.success {
                    success_count += 1;
                }
                results[index] = Some(TaskResult::from(outcome));
                completed_count += 1;
            }

            if accepted && should_return_early && success_count >= target_successes {
                returned_early = true;
                break;
            }

            while active_count < max_concurrency {
                let Some((index, spec)) = pending.next() else {
                    break;
                };
                let task_id = spawn_parallel_task_step(
                    &mut join_set,
                    Arc::clone(&scoped_executor),
                    event_tx.clone(),
                    index,
                    spec,
                );
                active_indexes.insert(task_id, index);
                active_count += 1;
            }
        }

        if timed_out || returned_early || active_count > 0 {
            parallel_cancellation.cancel();
            settle_cancelled_parallel_tasks(&mut join_set).await;
        }

        let unfinished_message = if timed_out {
            format!(
                "Task timed out before parallel_task finished collecting child results after {} ms.",
                timeout_ms.unwrap_or_default()
            )
        } else if returned_early {
            format!(
                "Task cancelled after parallel_task collected {success_count} successful child result(s)."
            )
        } else {
            "Task did not return a result before parallel_task ended.".to_string()
        };
        let results = results
            .into_iter()
            .enumerate()
            .map(|(index, result)| {
                result.unwrap_or_else(|| {
                    let (task_id, agent) = labels
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));
                    TaskResult::from(StepOutcome::failed(
                        task_id,
                        agent,
                        unfinished_message.clone(),
                    ))
                })
            })
            .collect();

        ParallelTaskRun {
            results,
            timed_out,
            returned_early,
            timeout_ms,
            min_success_count,
        }
    }
}

async fn settle_cancelled_parallel_tasks(
    join_set: &mut JoinSet<(usize, std::result::Result<StepOutcome, String>)>,
) {
    const SETTLEMENT_GRACE: std::time::Duration = std::time::Duration::from_millis(500);
    let deadline = tokio::time::Instant::now() + SETTLEMENT_GRACE;
    while !join_set.is_empty() {
        match tokio::time::timeout_at(deadline, join_set.join_next()).await {
            Ok(Some(_)) => {}
            Ok(None) => return,
            Err(_) => break,
        }
    }

    if join_set.is_empty() {
        return;
    }
    join_set.abort_all();
    while join_set.join_next().await.is_some() {}
}

fn spawn_parallel_task_step(
    join_set: &mut JoinSet<(usize, std::result::Result<StepOutcome, String>)>,
    executor: Arc<dyn AgentExecutor>,
    event_tx: Option<broadcast::Sender<AgentEvent>>,
    index: usize,
    spec: AgentStepSpec,
) -> tokio::task::Id {
    join_set
        .spawn(async move {
            let outcome = AssertUnwindSafe(executor.execute_step(spec, event_tx))
                .catch_unwind()
                .await
                .map_err(panic_payload_to_string);
            (index, outcome)
        })
        .id()
}

fn take_parallel_task_index(
    active_indexes: &mut HashMap<tokio::task::Id, usize>,
    task_id: tokio::task::Id,
) -> Option<usize> {
    active_indexes.remove(&task_id)
}

fn panic_payload_to_string(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return format!("parallel branch panicked: {message}");
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return format!("parallel branch panicked: {message}");
    }
    "parallel branch panicked: unknown panic payload".to_string()
}

pub(super) struct ParallelTaskRun {
    pub(super) results: Vec<TaskResult>,
    pub(super) timed_out: bool,
    pub(super) returned_early: bool,
    pub(super) timeout_ms: Option<u64>,
    pub(super) min_success_count: Option<usize>,
}

impl From<TaskResult> for StepOutcome {
    fn from(r: TaskResult) -> Self {
        StepOutcome {
            task_id: r.task_id,
            session_id: r.session_id,
            agent: r.agent,
            output: r.output,
            success: r.success,
            structured: r.structured,
            source_anchors: r.source_anchors,
        }
    }
}

impl From<StepOutcome> for TaskResult {
    fn from(o: StepOutcome) -> Self {
        TaskResult {
            output: o.output,
            session_id: o.session_id,
            agent: o.agent,
            success: o.success,
            task_id: o.task_id,
            structured: o.structured,
            source_anchors: o.source_anchors,
        }
    }
}

/// The local, in-process executor: every step runs as a child `AgentLoop` on
/// this node's tokio runtime. This is the default; a host substitutes
/// its own [`AgentExecutor`] to place steps across a cluster.
#[async_trait]
impl AgentExecutor for TaskExecutor {
    async fn execute_step(
        &self,
        spec: AgentStepSpec,
        event_tx: Option<broadcast::Sender<AgentEvent>>,
    ) -> StepOutcome {
        self.execute_step_with_parent_cancellation(
            spec,
            event_tx,
            self.parent_cancellation.as_ref(),
        )
        .await
    }

    fn concurrency_hint(&self) -> usize {
        self.max_parallel_tasks
    }
}

impl TaskExecutor {
    async fn execute_step_with_parent_cancellation(
        &self,
        spec: AgentStepSpec,
        event_tx: Option<broadcast::Sender<AgentEvent>>,
        parent_cancellation: Option<&CancellationToken>,
    ) -> StepOutcome {
        let agent = spec.agent.clone();
        let task_id = spec.task_id.clone();
        let _permit = match self.acquire_parallel_permit(parent_cancellation).await {
            Ok(permit) => permit,
            Err(error) => return StepOutcome::failed(task_id, agent, error),
        };
        let params = TaskParams {
            agent: spec.agent,
            description: spec.description,
            prompt: spec.prompt,
            background: false,
            max_steps: spec.max_steps,
            output_schema: spec.output_schema,
        };
        match self
            .execute_with_task_id_scoped(
                task_id.clone(),
                params,
                event_tx,
                spec.parent_session_id.as_deref(),
                true,
                parent_cancellation,
            )
            .await
        {
            Ok(result) => result.into(),
            Err(e) => StepOutcome::failed(task_id, agent, format!("Task failed: {e}")),
        }
    }

    async fn acquire_parallel_permit(
        &self,
        parent_cancellation: Option<&CancellationToken>,
    ) -> std::result::Result<tokio::sync::OwnedSemaphorePermit, String> {
        let acquire = Arc::clone(&self.parallel_permits).acquire_owned();
        match parent_cancellation {
            Some(cancellation) => {
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => {
                        Err("Task cancelled while waiting for parallel provider capacity".to_string())
                    }
                    permit = acquire => permit.map_err(|error| {
                        format!("Parallel provider capacity closed unexpectedly: {error}")
                    }),
                }
            }
            None => acquire.await.map_err(|error| {
                format!("Parallel provider capacity closed unexpectedly: {error}")
            }),
        }
    }

    /// Coerce a step's free-text output into a JSON object validated against
    /// `schema`, reusing the structured-output machinery with built-in repair.
    /// This is one extra LLM call beyond the step's own run.
    pub(super) async fn coerce_to_schema(
        llm_client: &dyn LlmClient,
        output: &str,
        schema: serde_json::Value,
        cancellation: &CancellationToken,
    ) -> Result<serde_json::Value> {
        let req = StructuredRequest {
            prompt: format!(
                "Convert the following task result into a single JSON object that conforms to \
                 the required schema. Use only information present in the result.\n\n\
                 --- TASK RESULT ---\n{output}"
            ),
            system: Some(
                "You output exactly one JSON object matching the provided schema.".to_string(),
            ),
            schema,
            schema_name: "step_output".to_string(),
            schema_description: None,
            // Request tool mode when available; unknown providers safely
            // downgrade to prompt+schema parsing.
            mode: StructuredMode::Tool,
            max_repair_attempts: 2,
        };
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => anyhow::bail!("Operation cancelled by user"),
            result = generate_blocking(llm_client, &req) => result?,
        };
        Ok(result.object)
    }

    pub(super) async fn generate_structured_task(
        llm_client: &dyn LlmClient,
        prompt: &str,
        system: Option<&str>,
        schema: serde_json::Value,
        cancellation: &CancellationToken,
    ) -> Result<serde_json::Value> {
        let req = StructuredRequest {
            prompt: prompt.to_string(),
            system: Some(format!(
                "{}\n\nReturn exactly one JSON object matching the provided schema.",
                system.unwrap_or("Make the requested structured decision without tools.")
            )),
            schema,
            schema_name: "step_output".to_string(),
            schema_description: None,
            mode: StructuredMode::Tool,
            max_repair_attempts: 2,
        };
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => anyhow::bail!("Operation cancelled by user"),
            result = generate_blocking(llm_client, &req) => result?,
        };
        Ok(result.object)
    }
}

struct ScopedTaskExecutor {
    executor: Arc<TaskExecutor>,
    parent_cancellation: CancellationToken,
}

#[async_trait]
impl AgentExecutor for ScopedTaskExecutor {
    async fn execute_step(
        &self,
        spec: AgentStepSpec,
        event_tx: Option<broadcast::Sender<AgentEvent>>,
    ) -> StepOutcome {
        self.executor
            .execute_step_with_parent_cancellation(spec, event_tx, Some(&self.parent_cancellation))
            .await
    }

    fn concurrency_hint(&self) -> usize {
        self.executor.max_parallel_tasks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn aborted_join_keeps_the_spawned_branch_index() {
        let mut join_set = JoinSet::new();
        let handle = join_set.spawn(async {
            std::future::pending::<(usize, std::result::Result<StepOutcome, String>)>().await
        });
        let mut active_indexes = HashMap::from([(handle.id(), 7)]);
        handle.abort();

        let error = join_set
            .join_next_with_id()
            .await
            .expect("aborted task should settle")
            .expect_err("aborted task should return JoinError");

        assert_eq!(
            take_parallel_task_index(&mut active_indexes, error.id()),
            Some(7)
        );
        assert!(active_indexes.is_empty());
    }
}
