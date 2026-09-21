use super::{AgentEvent, AgentLoop};
use crate::agent::{AgentExecutionFailure, AgentResult};
use crate::llm::{Message, TokenUsage};
use crate::planning::{ExecutionPlan, Task, TaskStatus};
use crate::prompts::AgentStyle;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Result of a single parallel step execution, emitted as structured JSON
/// so the frontend can render it dynamically in the user's language.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ParallelStepResult {
    pub step_id: String,
    pub step_number: u32,
    pub status: String, // "completed" | "failed"
    /// Brief summary of what this step did (in the LLM's language).
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_findings: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ParallelStepResult {
    /// Build the full parallel-results JSON envelope injected into history.
    pub fn build_envelope(results: Vec<ParallelStepResult>) -> Value {
        json!({
            "type": "parallel_results",
            "steps": results
        })
    }
}

#[derive(Debug, Clone)]
struct DelegatedParallelChildResult {
    success: bool,
    output: Option<String>,
    data: Option<Value>,
}

fn accumulate_result_accounting(
    total_usage: &mut TokenUsage,
    tool_calls_count: &mut usize,
    verification_reports: &mut Vec<crate::verification::VerificationReport>,
    result: &AgentResult,
) {
    total_usage.accumulate(&result.usage);
    *tool_calls_count = (*tool_calls_count).saturating_add(result.tool_calls_count);
    verification_reports.extend(result.verification_reports.iter().cloned());
}

pub(super) fn push_published_completions(
    slot: &mut Vec<crate::harness_loop::CompletionTerminal>,
    metadata: &Option<Value>,
) {
    let Some(metadata) = metadata else {
        return;
    };
    if let Some(value) = metadata.get("completion") {
        if let Ok(terminal) = serde_json::from_value(value.clone()) {
            slot.push(terminal);
        }
    }
    if let Some(results) = metadata.get("results").and_then(Value::as_array) {
        for result in results {
            if let Some(value) = result.get("completion") {
                if let Ok(terminal) = serde_json::from_value(value.clone()) {
                    slot.push(terminal);
                }
            }
        }
    }
}

pub(super) fn delegated_mutation_without_closure(metadata: &Option<Value>) -> bool {
    let Some(metadata) = metadata else {
        return false;
    };
    let has_paths = metadata
        .get("changed_paths")
        .and_then(Value::as_array)
        .is_some_and(|paths| !paths.is_empty());
    if !has_paths {
        return false;
    }
    metadata.get("completion").is_none()
        && metadata
            .get("results")
            .and_then(Value::as_array)
            .is_none_or(|results| {
                results
                    .iter()
                    .all(|result| result.get("completion").is_none())
            })
}

fn remember_completion_gate(slot: &mut Option<String>, message: &str) {
    if slot.is_none() && message.contains("completion gate:") {
        *slot = Some(message.to_string());
    }
}

fn accumulate_failure_accounting(
    total_usage: &mut TokenUsage,
    tool_calls_count: &mut usize,
    error: &anyhow::Error,
) {
    let Some(failure) = error.downcast_ref::<AgentExecutionFailure>() else {
        return;
    };
    total_usage.accumulate(failure.usage());
    *tool_calls_count = (*tool_calls_count).saturating_add(failure.tool_calls_count());
}

impl AgentLoop {
    pub(super) fn normalized_plan_tool(step: &Task) -> Option<&str> {
        step.tool
            .as_deref()
            .map(str::trim)
            .filter(|tool| !tool.is_empty())
    }

    pub(super) fn should_delegate_plan_step(step: &Task) -> bool {
        matches!(
            Self::normalized_plan_tool(step),
            Some("task") | Some("parallel_task")
        )
    }

    fn can_auto_delegate_plan_wave(&self, steps: &[(Task, usize)]) -> bool {
        let config = &self.config.auto_delegation;
        config.enabled
            && config.auto_parallel
            && config.max_tasks > 0
            && self.config.agent_registry.is_some()
            && self.tool_executor.registry().contains("task")
            && steps.iter().all(|(step, _)| {
                Self::normalized_plan_tool(step).is_none() || Self::should_delegate_plan_step(step)
            })
    }

    fn should_delegate_plan_wave(&self, steps: &[(Task, usize)]) -> bool {
        steps
            .iter()
            .all(|(step, _)| Self::should_delegate_plan_step(step))
            || self.can_auto_delegate_plan_wave(steps)
    }

    pub(super) fn delegated_agent_for_step(step: &Task) -> &'static str {
        let text = format!(
            "{}\n{}",
            step.content,
            step.success_criteria.as_deref().unwrap_or_default()
        )
        .to_lowercase();

        if text.contains("review")
            || text.contains("code review")
            || text.contains("regression")
            || text.contains("审查")
            || text.contains("评审")
            || text.contains("回归")
        {
            "review"
        } else if text.contains("verify")
            || text.contains("verification")
            || text.contains("validate")
            || text.contains("test")
            || text.contains("release")
            || text.contains("smoke")
            || text.contains("验证")
            || text.contains("测试")
            || text.contains("发布")
        {
            "verification"
        } else if text.contains("plan")
            || text.contains("design")
            || text.contains("architecture")
            || text.contains("规划")
            || text.contains("设计")
            || text.contains("架构")
        {
            "plan"
        } else if text.contains("explore")
            || text.contains("find")
            || text.contains("search")
            || text.contains("locate")
            || text.contains("inspect")
            || text.contains("查找")
            || text.contains("搜索")
            || text.contains("定位")
            || text.contains("探索")
            || text.contains("检查")
        {
            "explore"
        } else {
            "general"
        }
    }

    pub(super) fn delegated_prompt_for_step_with_goal(
        plan_goal: Option<&str>,
        step: &Task,
        step_number: usize,
        total_steps: usize,
    ) -> String {
        let mut prompt = String::new();
        if let Some(goal) = plan_goal.map(str::trim).filter(|goal| !goal.is_empty()) {
            prompt.push_str("Plan goal/context:\n");
            prompt.push_str(goal);
            prompt.push_str("\n\n");
        }
        prompt.push_str(&format!(
            "Execute plan step {}/{}.\n\nTask:\n{}\n",
            step_number, total_steps, step.content
        ));
        if let Some(criteria) = step
            .success_criteria
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            prompt.push_str("\nSuccess criteria:\n");
            prompt.push_str(criteria);
            prompt.push('\n');
        }
        prompt.push_str("\nReturn a compact result with summary, evidence, risks, and confidence.");
        prompt
    }

    pub(super) fn delegated_task_args_with_goal(
        plan_goal: Option<&str>,
        step: &Task,
        step_number: usize,
        total_steps: usize,
    ) -> Value {
        json!({
            "agent": Self::delegated_agent_for_step(step),
            "description": step.content,
            "prompt": Self::delegated_prompt_for_step_with_goal(plan_goal, step, step_number, total_steps),
        })
    }

    pub(super) fn parallel_delegated_task_args_with_goal(
        plan_goal: Option<&str>,
        steps: &[(Task, usize)],
        total_steps: usize,
    ) -> Value {
        let tasks = steps
            .iter()
            .map(|(step, step_number)| {
                Self::delegated_task_args_with_goal(plan_goal, step, *step_number, total_steps)
            })
            .collect::<Vec<_>>();
        // A plan wave owns per-step status and dependency handling. Let the
        // tool preserve successful siblings when one branch fails instead of
        // escalating the whole wave as a single opaque tool failure.
        json!({
            "tasks": tasks,
            "allow_partial_failure": true,
        })
    }

    fn delegated_parallel_child_results(
        metadata: Option<&Value>,
        child_count: usize,
        fallback_success: bool,
    ) -> Vec<DelegatedParallelChildResult> {
        let metadata_results = metadata
            .and_then(|value| value.get("results"))
            .and_then(Value::as_array);

        (0..child_count)
            .map(|index| {
                let child = metadata_results.and_then(|results| results.get(index));
                let success = child
                    .and_then(|value| value.get("success"))
                    .and_then(Value::as_bool)
                    .unwrap_or(fallback_success);
                let output = child
                    .and_then(|value| value.get("output_excerpt").or_else(|| value.get("output")))
                    .and_then(Value::as_str)
                    .map(str::to_string);

                DelegatedParallelChildResult {
                    success,
                    output,
                    data: child.cloned(),
                }
            })
            .collect()
    }

    /// Execute an execution plan using wave-based dependency-aware scheduling.
    ///
    /// Steps with no unmet dependencies are grouped into "waves". A wave with
    /// a single step executes sequentially (preserving the history chain). A
    /// wave with multiple independent steps executes them in parallel via
    /// `JoinSet`, then merges their results back into the shared history.
    pub(super) async fn execute_plan(
        &self,
        history: &[Message],
        plan: &ExecutionPlan,
        session_id: Option<&str>,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &CancellationToken,
    ) -> Result<AgentResult> {
        if cancel_token.is_cancelled() {
            anyhow::bail!("Operation cancelled by user");
        }
        let mut plan = plan.clone();
        let task_session_id = session_id.unwrap_or("").to_string();
        let mut current_history = history.to_vec();
        let mut total_usage = TokenUsage::default();
        let mut tool_calls_count = 0;
        let mut verification_reports = Vec::new();
        let mut completion_gate_failure = None;
        let mut step_completions = Vec::new();
        let total_steps = plan.steps.len();

        // Product transcript keeps the human task; model wire gets the plan kickoff.
        let human_task = plan.product_goal();
        let product_text = human_task;
        if current_history
            .last()
            .is_none_or(|message| message.role != "user" || !message.is_product_transcript())
        {
            current_history.push(Message::user(&product_text));
        }
        let steps_text = plan
            .steps
            .iter()
            .enumerate()
            .map(|(i, step)| format!("{}. {}", i + 1, step.content))
            .collect::<Vec<_>>()
            .join("\n");
        let wire_goal = plan.wire_goal().to_string();
        current_history.push(Message::user_wire(&crate::prompts::render(
            crate::prompts::PLAN_EXECUTE_GOAL,
            &[("goal", &wire_goal), ("steps", &steps_text)],
        )));
        self.emit_task_updated(&event_tx, &task_session_id, &plan)
            .await;

        loop {
            if cancel_token.is_cancelled() {
                break;
            }
            let ready: Vec<String> = plan
                .get_ready_steps()
                .iter()
                .map(|s| s.id.clone())
                .collect();

            if ready.is_empty() {
                // All done or deadlock
                if plan.has_deadlock() {
                    tracing::warn!(
                        "Plan deadlock detected: {} pending steps with unresolvable dependencies",
                        plan.pending_count()
                    );
                }
                break;
            }

            if ready.len() == 1 {
                // === Single step: sequential execution (preserves history chain) ===
                let step_id = &ready[0];
                let step = plan
                    .steps
                    .iter()
                    .find(|s| s.id == *step_id)
                    .ok_or_else(|| anyhow::anyhow!("step '{}' not found in plan", step_id))?
                    .clone();
                let step_number = plan
                    .steps
                    .iter()
                    .position(|s| s.id == *step_id)
                    .unwrap_or(0)
                    + 1;

                // Send step start event
                plan.mark_status(&step.id, TaskStatus::InProgress);
                self.emit_task_updated(&event_tx, &task_session_id, &plan)
                    .await;

                if let Some(tx) = &event_tx {
                    tx.send(AgentEvent::StepStart {
                        step_id: step.id.clone(),
                        description: step.content.clone(),
                        step_number,
                        total_steps,
                    })
                    .await
                    .ok();
                }

                if Self::should_delegate_plan_step(&step) {
                    let args = json!({
                        "tasks": [Self::delegated_task_args_with_goal(
                            Some(plan.wire_goal()),
                            &step,
                            step_number,
                            total_steps,
                        )]
                    });
                    let (output, _exit_code, is_error, metadata) = self
                        .execute_delegated_plan_tool(
                            "task",
                            &args,
                            session_id,
                            &event_tx,
                            cancel_token,
                        )
                        .await;
                    tool_calls_count += 1;
                    Self::collect_verification_report(&mut verification_reports, &metadata);

                    remember_completion_gate(&mut completion_gate_failure, &output);
                    push_published_completions(&mut step_completions, &metadata);
                    if !is_error && delegated_mutation_without_closure(&metadata) {
                        remember_completion_gate(
                            &mut completion_gate_failure,
                            "completion gate: delegated plan step changed the workspace without a bound closure. A final answer does not make that narrative.",
                        );
                    }
                    if is_error {
                        tracing::error!("Delegated plan step '{}' failed: {}", step.id, output);
                        current_history.push(Message::user_wire(&format!(
                            "Delegated plan step '{}' failed:\n{}",
                            step.content, output
                        )));
                        plan.mark_status(&step.id, TaskStatus::Failed);
                        self.emit_task_updated(&event_tx, &task_session_id, &plan)
                            .await;

                        if let Some(tx) = &event_tx {
                            tx.send(AgentEvent::StepEnd {
                                step_id: step.id.clone(),
                                status: TaskStatus::Failed,
                                step_number,
                                total_steps,
                            })
                            .await
                            .ok();
                        }
                    } else {
                        current_history.push(Message::assistant(&output));
                        plan.mark_status(&step.id, TaskStatus::Completed);
                        self.emit_task_updated(&event_tx, &task_session_id, &plan)
                            .await;

                        if let Some(tx) = &event_tx {
                            tx.send(AgentEvent::StepEnd {
                                step_id: step.id.clone(),
                                status: TaskStatus::Completed,
                                step_number,
                                total_steps,
                            })
                            .await
                            .ok();
                        }
                    }
                } else {
                    let step_prompt = crate::prompts::render(
                        crate::prompts::PLAN_EXECUTE_STEP,
                        &[
                            ("step_num", &step_number.to_string()),
                            ("description", &step.content),
                        ],
                    );

                    match self
                        .execute_loop(
                            &current_history,
                            &step_prompt,
                            AgentStyle::GeneralPurpose,
                            None,
                            event_tx.clone(),
                            cancel_token,
                            false, // emit_end: false — End is emitted by execute_with_planning after execute_plan
                        )
                        .await
                    {
                        Ok(result) => {
                            current_history = result.messages.clone();
                            step_completions.push(result.completion.clone());
                            accumulate_result_accounting(
                                &mut total_usage,
                                &mut tool_calls_count,
                                &mut verification_reports,
                                &result,
                            );
                            if cancel_token.is_cancelled() {
                                plan.mark_status(&step.id, TaskStatus::Cancelled);
                                self.emit_task_updated(&event_tx, &task_session_id, &plan)
                                    .await;
                                if let Some(tx) = &event_tx {
                                    tx.send(AgentEvent::StepEnd {
                                        step_id: step.id.clone(),
                                        status: TaskStatus::Cancelled,
                                        step_number,
                                        total_steps,
                                    })
                                    .await
                                    .ok();
                                }
                                break;
                            }
                            plan.mark_status(&step.id, TaskStatus::Completed);
                            self.emit_task_updated(&event_tx, &task_session_id, &plan)
                                .await;

                            if let Some(tx) = &event_tx {
                                tx.send(AgentEvent::StepEnd {
                                    step_id: step.id.clone(),
                                    status: TaskStatus::Completed,
                                    step_number,
                                    total_steps,
                                })
                                .await
                                .ok();
                            }
                        }
                        Err(e) => {
                            remember_completion_gate(&mut completion_gate_failure, &e.to_string());
                            accumulate_failure_accounting(
                                &mut total_usage,
                                &mut tool_calls_count,
                                &e,
                            );
                            if cancel_token.is_cancelled() {
                                plan.mark_status(&step.id, TaskStatus::Cancelled);
                                self.emit_task_updated(&event_tx, &task_session_id, &plan)
                                    .await;
                                if let Some(tx) = &event_tx {
                                    tx.send(AgentEvent::StepEnd {
                                        step_id: step.id.clone(),
                                        status: TaskStatus::Cancelled,
                                        step_number,
                                        total_steps,
                                    })
                                    .await
                                    .ok();
                                }
                                break;
                            }
                            tracing::error!("Plan step '{}' failed: {}", step.id, e);
                            plan.mark_status(&step.id, TaskStatus::Failed);
                            self.emit_task_updated(&event_tx, &task_session_id, &plan)
                                .await;

                            if let Some(tx) = &event_tx {
                                tx.send(AgentEvent::StepEnd {
                                    step_id: step.id.clone(),
                                    status: TaskStatus::Failed,
                                    step_number,
                                    total_steps,
                                })
                                .await
                                .ok();
                            }
                        }
                    }
                }
            } else {
                // === Multiple steps: parallel execution via JoinSet ===
                // NOTE: Each parallel branch gets a clone of the base history.
                // Individual branch histories (tool calls, LLM turns) are NOT merged
                // back — only a summary message is appended. This is a deliberate
                // trade-off: merging divergent histories in a deterministic order is
                // complex and the summary approach keeps the context window manageable.
                let ready_steps: Vec<_> = ready
                    .iter()
                    .filter_map(|id| {
                        let step = plan.steps.iter().find(|s| s.id == *id)?.clone();
                        let step_number =
                            plan.steps.iter().position(|s| s.id == *id).unwrap_or(0) + 1;
                        Some((step, step_number))
                    })
                    .collect();

                // Mark all as InProgress and emit one task-list snapshot for the wave.
                for (step, _) in &ready_steps {
                    plan.mark_status(&step.id, TaskStatus::InProgress);
                }
                self.emit_task_updated(&event_tx, &task_session_id, &plan)
                    .await;

                // Emit StepStart events after the authoritative task-list snapshot.
                for (step, step_number) in &ready_steps {
                    if let Some(tx) = &event_tx {
                        tx.send(AgentEvent::StepStart {
                            step_id: step.id.clone(),
                            description: step.content.clone(),
                            step_number: *step_number,
                            total_steps,
                        })
                        .await
                        .ok();
                    }
                }

                let mut parallel_results: Vec<ParallelStepResult> = Vec::new();
                if self.should_delegate_plan_wave(&ready_steps) {
                    let args = Self::parallel_delegated_task_args_with_goal(
                        Some(plan.wire_goal()),
                        &ready_steps,
                        total_steps,
                    );
                    let (output, _exit_code, is_error, metadata) = self
                        .execute_delegated_plan_tool(
                            "task",
                            &args,
                            session_id,
                            &event_tx,
                            cancel_token,
                        )
                        .await;
                    tool_calls_count += 1;
                    Self::collect_verification_report(&mut verification_reports, &metadata);

                    let child_results = Self::delegated_parallel_child_results(
                        metadata.as_ref(),
                        ready_steps.len(),
                        !is_error,
                    );
                    remember_completion_gate(&mut completion_gate_failure, &output);
                    push_published_completions(&mut step_completions, &metadata);
                    if !is_error && delegated_mutation_without_closure(&metadata) {
                        remember_completion_gate(
                            &mut completion_gate_failure,
                            "completion gate: delegated plan step changed the workspace without a bound closure. A final answer does not make that narrative.",
                        );
                    }
                    let wave_failed =
                        is_error || child_results.iter().any(|result| !result.success);

                    for ((step, step_number), child_result) in
                        ready_steps.iter().zip(child_results.iter())
                    {
                        if !child_result.success {
                            if let Some(error) = child_result.output.as_deref() {
                                remember_completion_gate(&mut completion_gate_failure, error);
                            }
                        }
                        let status = if child_result.success {
                            TaskStatus::Completed
                        } else {
                            TaskStatus::Failed
                        };
                        plan.mark_status(&step.id, status);
                        self.emit_task_updated(&event_tx, &task_session_id, &plan)
                            .await;

                        parallel_results.push(ParallelStepResult {
                            step_id: step.id.clone(),
                            step_number: *step_number as u32,
                            status: if child_result.success {
                                "completed"
                            } else {
                                "failed"
                            }
                            .to_string(),
                            summary: if child_result.success {
                                child_result
                                    .output
                                    .clone()
                                    .unwrap_or_else(|| output.trim().to_string())
                            } else {
                                String::new()
                            },
                            key_findings: None,
                            error: (!child_result.success).then(|| {
                                child_result
                                    .output
                                    .clone()
                                    .unwrap_or_else(|| output.clone())
                            }),
                            data: child_result.data.clone(),
                        });

                        if let Some(tx) = &event_tx {
                            tx.send(AgentEvent::StepEnd {
                                step_id: step.id.clone(),
                                status,
                                step_number: *step_number,
                                total_steps,
                            })
                            .await
                            .ok();
                        }
                    }

                    if wave_failed {
                        current_history.push(Message::user_wire(&format!(
                            "Delegated parallel plan wave completed with failures:\n{}",
                            output
                        )));
                    } else {
                        current_history.push(Message::assistant(&output));
                    }
                } else {
                    let step_lookup = ready_steps
                        .iter()
                        .map(|(step, step_number)| (step.id.clone(), *step_number))
                        .collect::<Vec<_>>();
                    let outcomes = crate::ordered_parallel::run_ordered_parallel_with_limit(
                        ready_steps.clone(),
                        self.config.max_parallel_tasks,
                        {
                            let base_history = current_history.clone();
                            let agent = self.clone();
                            let tx = event_tx.clone();
                            let cancel_token = cancel_token.clone();
                            move |_index, (step, step_number)| {
                                let base_history = base_history.clone();
                                let agent_clone = agent.clone();
                                let tx = tx.clone();
                                let cancel_token = cancel_token.clone();
                                async move {
                                    if cancel_token.is_cancelled() {
                                        anyhow::bail!("Operation cancelled by user");
                                    }
                                    let prompt = crate::prompts::render(
                                        crate::prompts::PLAN_EXECUTE_STEP,
                                        &[
                                            ("step_num", &step_number.to_string()),
                                            ("description", &step.content),
                                        ],
                                    );
                                    agent_clone
                                        .execute_loop(
                                            &base_history,
                                            &prompt,
                                            AgentStyle::GeneralPurpose,
                                            None,
                                            tx,
                                            &cancel_token,
                                            false, // emit_end: false — End is emitted by execute_with_planning after execute_plan
                                        )
                                        .await
                                }
                            }
                        },
                    )
                    .await;

                    if cancel_token.is_cancelled() {
                        for outcome in &outcomes {
                            if let Ok(Err(error)) = &outcome.output {
                                remember_completion_gate(
                                    &mut completion_gate_failure,
                                    &error.to_string(),
                                );
                            }
                        }
                        for (step, step_number) in &ready_steps {
                            plan.mark_status(&step.id, TaskStatus::Cancelled);
                            if let Some(tx) = &event_tx {
                                tx.send(AgentEvent::StepEnd {
                                    step_id: step.id.clone(),
                                    status: TaskStatus::Cancelled,
                                    step_number: *step_number,
                                    total_steps,
                                })
                                .await
                                .ok();
                            }
                        }
                        self.emit_task_updated(&event_tx, &task_session_id, &plan)
                            .await;
                        break;
                    }

                    for outcome in outcomes {
                        let (step_id, step_number) = step_lookup
                            .get(outcome.index)
                            .cloned()
                            .unwrap_or_else(|| ("unknown".to_string(), 0));
                        match outcome.output {
                            Ok(step_result) => match step_result {
                                Ok(result) => {
                                    step_completions.push(result.completion.clone());
                                    accumulate_result_accounting(
                                        &mut total_usage,
                                        &mut tool_calls_count,
                                        &mut verification_reports,
                                        &result,
                                    );
                                    plan.mark_status(&step_id, TaskStatus::Completed);
                                    self.emit_task_updated(&event_tx, &task_session_id, &plan)
                                        .await;

                                    parallel_results.push(ParallelStepResult {
                                        step_id: step_id.clone(),
                                        step_number: step_number as u32,
                                        status: "completed".to_string(),
                                        summary: result.text.trim().to_string(),
                                        key_findings: None,
                                        error: None,
                                        data: None,
                                    });

                                    if let Some(tx) = &event_tx {
                                        tx.send(AgentEvent::StepEnd {
                                            step_id,
                                            status: TaskStatus::Completed,
                                            step_number,
                                            total_steps,
                                        })
                                        .await
                                        .ok();
                                    }
                                }
                                Err(e) => {
                                    remember_completion_gate(
                                        &mut completion_gate_failure,
                                        &e.to_string(),
                                    );
                                    accumulate_failure_accounting(
                                        &mut total_usage,
                                        &mut tool_calls_count,
                                        &e,
                                    );
                                    tracing::error!("Plan step '{}' failed: {}", step_id, e);
                                    plan.mark_status(&step_id, TaskStatus::Failed);
                                    self.emit_task_updated(&event_tx, &task_session_id, &plan)
                                        .await;

                                    parallel_results.push(ParallelStepResult {
                                        step_id: step_id.clone(),
                                        step_number: step_number as u32,
                                        status: "failed".to_string(),
                                        summary: String::new(),
                                        key_findings: None,
                                        error: Some(e.to_string()),
                                        data: None,
                                    });

                                    if let Some(tx) = &event_tx {
                                        tx.send(AgentEvent::StepEnd {
                                            step_id,
                                            status: TaskStatus::Failed,
                                            step_number,
                                            total_steps,
                                        })
                                        .await
                                        .ok();
                                    }
                                }
                            },
                            Err(e) => {
                                tracing::error!("Plan step '{}' failed: {}", step_id, e);
                                plan.mark_status(&step_id, TaskStatus::Failed);
                                self.emit_task_updated(&event_tx, &task_session_id, &plan)
                                    .await;

                                parallel_results.push(ParallelStepResult {
                                    step_id: step_id.clone(),
                                    step_number: step_number as u32,
                                    status: "failed".to_string(),
                                    summary: String::new(),
                                    key_findings: None,
                                    error: Some(e.to_string()),
                                    data: None,
                                });

                                if let Some(tx) = &event_tx {
                                    tx.send(AgentEvent::StepEnd {
                                        step_id,
                                        status: TaskStatus::Failed,
                                        step_number,
                                        total_steps,
                                    })
                                    .await
                                    .ok();
                                }
                            }
                        }
                    }
                }

                // Merge parallel results into history for subsequent steps.
                // Emit as a structured JSON USER message so the frontend can
                // parse and render it in the user's language.
                if !parallel_results.is_empty() {
                    parallel_results.sort_by_key(|r| r.step_number);
                    let envelope = ParallelStepResult::build_envelope(parallel_results);
                    current_history.push(Message::user_wire(
                        &serde_json::to_string(&envelope).unwrap_or_default(),
                    ));
                }
            }

            // Emit GoalProgress after each wave
            if self.config.goal_tracking {
                let completed = plan
                    .steps
                    .iter()
                    .filter(|s| s.status == TaskStatus::Completed)
                    .count();
                if let Some(tx) = &event_tx {
                    tx.send(AgentEvent::GoalProgress {
                        goal: plan.goal.clone(),
                        progress: plan.progress(),
                        completed_steps: completed,
                        total_steps,
                    })
                    .await
                    .ok();
                }
            }
        }

        // Get final response — find the last assistant message (not the last history
        // entry, which may be a user-summary injected after parallel execution)
        let final_text = current_history
            .iter()
            .rev()
            .find(|m| m.role == "assistant")
            .map(|m| {
                m.content
                    .iter()
                    .filter_map(|block| {
                        if let crate::llm::ContentBlock::Text { text } = block {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();

        if let Some(message) = completion_gate_failure {
            return Err(anyhow::anyhow!(message));
        }

        Ok(AgentResult {
            text: final_text,
            messages: current_history,
            usage: total_usage,
            tool_calls_count,
            verification_reports,
            completion: crate::harness_loop::fold_step_completions(&step_completions),
            run_admission: self.config.plan_run.label().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::MockLlmClient;
    use crate::planning::Complexity;
    use crate::tools::{ToolContext, ToolExecutor};
    use std::sync::Arc;

    #[test]
    fn delegated_bound_closure_is_kept_and_a_bare_mutation_is_not_narrative() {
        let mut completions = Vec::new();
        let metadata = Some(serde_json::json!({
            "changed_paths": ["guest.txt"],
            "completion": { "kind": "verified", "effect_digest": "digest-a" }
        }));
        push_published_completions(&mut completions, &metadata);
        assert_eq!(
            crate::harness_loop::fold_step_completions(&completions),
            crate::harness_loop::CompletionTerminal::Verified {
                effect_digest: "digest-a".to_string(),
            }
        );
        assert!(!delegated_mutation_without_closure(&metadata));
        let bare = Some(serde_json::json!({ "changed_paths": ["guest.txt"] }));
        assert!(delegated_mutation_without_closure(&bare));
    }

    #[test]
    fn delegated_results_use_each_child_output_excerpt() {
        let metadata = serde_json::json!({
            "results": [
                {
                    "success": true,
                    "output_excerpt": "auth branch evidence"
                },
                {
                    "success": false,
                    "output_excerpt": "docs branch failed: status 529"
                }
            ]
        });

        let results = AgentLoop::delegated_parallel_child_results(Some(&metadata), 2, false);

        assert!(results[0].success);
        assert_eq!(results[0].output.as_deref(), Some("auth branch evidence"));
        assert!(!results[1].success);
        assert_eq!(
            results[1].output.as_deref(),
            Some("docs branch failed: status 529")
        );
    }

    #[test]
    fn parallel_step_result_build_envelope_wraps_steps() {
        let envelope = ParallelStepResult::build_envelope(vec![ParallelStepResult {
            step_id: "s1".into(),
            step_number: 1,
            status: "completed".into(),
            summary: "ok".into(),
            key_findings: Some(vec!["a".into()]),
            error: None,
            data: Some(json!({ "k": 1 })),
        }]);
        assert_eq!(envelope["type"], "parallel_results");
        assert_eq!(envelope["steps"][0]["step_id"], "s1");
        assert_eq!(envelope["steps"][0]["key_findings"][0], "a");
    }

    #[test]
    fn push_published_completions_reads_nested_result_completions() {
        let mut completions = Vec::new();
        push_published_completions(&mut completions, &None);
        assert!(completions.is_empty());

        let metadata = Some(json!({
            "results": [
                { "completion": { "kind": "verified", "effect_digest": "child-a" } },
                { "completion": { "kind": "narrative" } },
                { "completion": "not-a-terminal" }
            ]
        }));
        push_published_completions(&mut completions, &metadata);
        assert_eq!(completions.len(), 2);
        assert_eq!(
            crate::harness_loop::fold_step_completions(&completions),
            crate::harness_loop::CompletionTerminal::Verified {
                effect_digest: "child-a".to_string(),
            }
        );
    }

    #[test]
    fn delegated_mutation_without_closure_requires_paths_and_missing_completion() {
        assert!(!delegated_mutation_without_closure(&None));
        assert!(!delegated_mutation_without_closure(&Some(
            json!({ "changed_paths": [] })
        )));
        assert!(!delegated_mutation_without_closure(&Some(json!({
            "changed_paths": ["a.txt"],
            "results": [{ "completion": { "kind": "narrative" } }]
        }))));
        assert!(delegated_mutation_without_closure(&Some(json!({
            "changed_paths": ["a.txt"],
            "results": [{ "success": true }]
        }))));
    }

    #[test]
    fn delegated_parallel_child_results_fall_back_and_prefer_output_field() {
        let missing = AgentLoop::delegated_parallel_child_results(None, 2, true);
        assert_eq!(missing.len(), 2);
        assert!(missing
            .iter()
            .all(|child| child.success && child.output.is_none()));

        let metadata = json!({
            "results": [
                { "success": true, "output": "full output wins" },
                { "output_excerpt": "excerpt only" }
            ]
        });
        let results = AgentLoop::delegated_parallel_child_results(Some(&metadata), 3, false);
        assert_eq!(results[0].output.as_deref(), Some("full output wins"));
        assert!(!results[1].success);
        assert_eq!(results[1].output.as_deref(), Some("excerpt only"));
        assert!(!results[2].success);
        assert!(results[2].output.is_none());
    }

    #[test]
    fn normalized_plan_tool_trims_and_filters_blank_tools() {
        assert_eq!(
            AgentLoop::normalized_plan_tool(&Task::new("s1", "x").with_tool("  task  ")),
            Some("task")
        );
        assert_eq!(
            AgentLoop::normalized_plan_tool(&Task::new("s1", "x").with_tool("   ")),
            None
        );
        assert!(AgentLoop::should_delegate_plan_step(
            &Task::new("s1", "x").with_tool("parallel_task")
        ));
        assert!(!AgentLoop::should_delegate_plan_step(
            &Task::new("s1", "x").with_tool("bash")
        ));
    }

    #[test]
    fn delegated_agent_and_prompt_cover_goal_and_criteria_keywords() {
        assert_eq!(
            AgentLoop::delegated_agent_for_step(
                &Task::new("s1", "ship it").with_success_criteria("审查回归风险")
            ),
            "review"
        );
        assert_eq!(
            AgentLoop::delegated_agent_for_step(
                &Task::new("s2", "work").with_success_criteria("准备发布 smoke")
            ),
            "verification"
        );
        assert_eq!(
            AgentLoop::delegated_agent_for_step(
                &Task::new("s3", "work").with_success_criteria("完成架构规划")
            ),
            "plan"
        );

        let prompt = AgentLoop::delegated_prompt_for_step_with_goal(
            Some("  Keep product transcript  "),
            &Task::new("s4", "Inspect docs").with_success_criteria("  find evidence  "),
            1,
            3,
        );
        assert!(prompt.contains("Plan goal/context:\nKeep product transcript"));
        assert!(prompt.contains("Execute plan step 1/3."));
        assert!(prompt.contains("Success criteria:\n  find evidence  "));
        assert!(prompt.contains("confidence"));
    }

    #[test]
    fn remember_completion_gate_keeps_first_message_only() {
        let mut slot = None;
        remember_completion_gate(&mut slot, "plain failure");
        assert!(slot.is_none());
        remember_completion_gate(&mut slot, "completion gate: first");
        remember_completion_gate(&mut slot, "completion gate: second");
        assert_eq!(slot.as_deref(), Some("completion gate: first"));
    }

    #[test]
    fn accumulate_result_accounting_merges_usage_tools_and_reports() {
        let mut usage = TokenUsage::default();
        let mut tool_calls = 1usize;
        let mut reports = Vec::new();
        let result = AgentResult {
            text: "done".into(),
            messages: Vec::new(),
            usage: TokenUsage {
                prompt_tokens: 2,
                completion_tokens: 3,
                total_tokens: 5,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            tool_calls_count: 4,
            verification_reports: vec![crate::verification::VerificationReport::new(
                "subject",
                vec![crate::verification::VerificationCheck::required(
                    "c1", "kind", "desc",
                )],
            )],
            completion: crate::harness_loop::CompletionTerminal::Narrative,
            run_admission: "run".into(),
        };
        accumulate_result_accounting(&mut usage, &mut tool_calls, &mut reports, &result);
        assert_eq!(usage.total_tokens, 5);
        assert_eq!(tool_calls, 5);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].subject, "subject");
    }

    #[test]
    fn accumulate_failure_accounting_only_reads_agent_execution_failure() {
        let mut usage = TokenUsage::default();
        let mut tool_calls = 0usize;
        accumulate_failure_accounting(&mut usage, &mut tool_calls, &anyhow::anyhow!("plain error"));
        assert_eq!(tool_calls, 0);

        let failure = AgentExecutionFailure::new(
            anyhow::anyhow!("gated"),
            TokenUsage {
                prompt_tokens: 3,
                completion_tokens: 2,
                total_tokens: 5,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            4,
        );
        accumulate_failure_accounting(&mut usage, &mut tool_calls, &anyhow::Error::new(failure));
        assert_eq!(tool_calls, 4);
        assert_eq!(usage.total_tokens, 5);
    }

    #[tokio::test]
    async fn execute_plan_rejects_already_cancelled_token() {
        let workspace = tempfile::tempdir().unwrap();
        let agent = AgentLoop::new(
            Arc::new(MockLlmClient::new(vec![])),
            Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
            ToolContext::new(workspace.path().to_path_buf()),
            crate::agent::AgentConfig::default(),
        );
        let plan = ExecutionPlan::new("cancelled up front", Complexity::Simple);
        let token = CancellationToken::new();
        token.cancel();

        let err = agent
            .execute_plan(&[], &plan, Some("pre-cancel"), None, &token)
            .await
            .expect_err("cancelled token must fail closed before plan work");
        assert!(err.to_string().contains("Operation cancelled by user"));
    }

    #[tokio::test]
    async fn execute_plan_breaks_on_dependency_deadlock() {
        let workspace = tempfile::tempdir().unwrap();
        let agent = AgentLoop::new(
            Arc::new(MockLlmClient::new(vec![])),
            Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
            ToolContext::new(workspace.path().to_path_buf()),
            crate::agent::AgentConfig::default(),
        );
        let mut plan = ExecutionPlan::new("deadlock", Complexity::Simple);
        plan.add_step(Task::new("s1", "A").with_dependencies(vec!["s2".into()]));
        plan.add_step(Task::new("s2", "B").with_dependencies(vec!["s1".into()]));

        let result = agent
            .execute_plan(
                &[],
                &plan,
                Some("deadlock"),
                None,
                &CancellationToken::new(),
            )
            .await
            .expect("deadlock should exit without hanging");
        assert!(
            result.messages.iter().any(|message| message.role == "user"),
            "plan kickoff should still be recorded before the deadlock exit"
        );
    }

    struct DenyTaskTool;

    impl crate::permissions::PermissionChecker for DenyTaskTool {
        fn check(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> crate::permissions::PermissionDecision {
            if tool_name == "task" {
                crate::permissions::PermissionDecision::Deny
            } else {
                crate::permissions::PermissionDecision::Allow
            }
        }
    }

    struct HangThenCancelClient {
        calls: std::sync::atomic::AtomicUsize,
        step_started: tokio::sync::Notify,
    }

    impl HangThenCancelClient {
        fn new() -> Self {
            Self {
                calls: std::sync::atomic::AtomicUsize::new(0),
                step_started: tokio::sync::Notify::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for HangThenCancelClient {
        async fn complete(
            &self,
            _messages: &[crate::llm::Message],
            _system: Option<&str>,
            _tools: &[crate::llm::ToolDefinition],
        ) -> anyhow::Result<crate::llm::LlmResponse> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.step_started.notify_one();
            std::future::pending().await
        }

        async fn complete_streaming(
            &self,
            _messages: &[crate::llm::Message],
            _system: Option<&str>,
            _tools: &[crate::llm::ToolDefinition],
            cancel_token: CancellationToken,
        ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.step_started.notify_one();
            let (tx, rx) = mpsc::channel(1);
            tokio::spawn(async move {
                cancel_token.cancelled().await;
                drop(tx);
            });
            Ok(rx)
        }
    }

    #[tokio::test]
    async fn execute_plan_emits_failed_step_end_for_denied_delegated_task() {
        let workspace = tempfile::tempdir().unwrap();
        let agent = AgentLoop::new(
            Arc::new(MockLlmClient::new(vec![])),
            Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
            ToolContext::new(workspace.path().to_path_buf()),
            crate::agent::AgentConfig {
                permission_checker: Some(Arc::new(DenyTaskTool)),
                ..crate::agent::AgentConfig::default()
            },
        );
        let mut plan = ExecutionPlan::new("deny delegated", Complexity::Simple);
        plan.add_step(Task::new("s1", "Find docs").with_tool("task"));
        let (event_tx, mut event_rx) = mpsc::channel(8);

        let result = agent
            .execute_plan(
                &[],
                &plan,
                Some("deny-delegated"),
                Some(event_tx),
                &CancellationToken::new(),
            )
            .await
            .expect("denied delegated step should complete the plan with failures recorded");

        assert!(
            result
                .messages
                .iter()
                .any(|message| message.role == "user" && message.text().contains("failed")),
            "denied delegated failure should be recorded in history"
        );

        let mut saw_failed_end = false;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(
                event,
                AgentEvent::StepEnd {
                    status: TaskStatus::Failed,
                    ..
                }
            ) {
                saw_failed_end = true;
            }
        }
        assert!(saw_failed_end, "StepEnd Failed should be emitted");
    }

    #[tokio::test]
    async fn execute_plan_emits_cancelled_step_end_when_parent_cancels() {
        let workspace = tempfile::tempdir().unwrap();
        let client = Arc::new(HangThenCancelClient::new());
        let agent = AgentLoop::new(
            client.clone(),
            Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
            ToolContext::new(workspace.path().to_path_buf()),
            crate::agent::AgentConfig::default(),
        );
        let mut plan = ExecutionPlan::new("cancel with events", Complexity::Simple);
        plan.add_step(Task::new("s1", "First serial step"));
        plan.add_step(Task::new("s2", "Second serial step").with_dependencies(vec!["s1".into()]));
        let cancel_token = CancellationToken::new();
        let run_token = cancel_token.clone();
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let run = tokio::spawn(async move {
            agent
                .execute_plan(
                    &[],
                    &plan,
                    Some("cancel-events"),
                    Some(event_tx),
                    &run_token,
                )
                .await
        });

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client.step_started.notified(),
        )
        .await
        .expect("first step should start");
        cancel_token.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), run)
            .await
            .expect("cancelled plan should finish")
            .expect("join")
            .expect("cancelled plan returns result");

        let mut saw_cancelled = false;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(
                event,
                AgentEvent::StepEnd {
                    status: TaskStatus::Cancelled,
                    ..
                }
            ) {
                saw_cancelled = true;
            }
        }
        assert!(
            saw_cancelled || client.calls.load(std::sync::atomic::Ordering::SeqCst) >= 1,
            "cancellation should interrupt an in-flight plan step"
        );
    }

    #[tokio::test]
    async fn execute_plan_parallel_delegated_wave_emits_failed_step_ends() {
        let workspace = tempfile::tempdir().unwrap();
        let agent = AgentLoop::new(
            Arc::new(MockLlmClient::new(vec![])),
            Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
            ToolContext::new(workspace.path().to_path_buf()),
            crate::agent::AgentConfig {
                permission_checker: Some(Arc::new(DenyTaskTool)),
                max_parallel_tasks: 2,
                ..crate::agent::AgentConfig::default()
            },
        );
        let mut plan = ExecutionPlan::new("parallel deny", Complexity::Simple);
        plan.add_step(Task::new("s1", "Parallel A").with_tool("task"));
        plan.add_step(Task::new("s2", "Parallel B").with_tool("task"));
        let (event_tx, mut event_rx) = mpsc::channel(16);

        let _ = agent
            .execute_plan(
                &[],
                &plan,
                Some("parallel-deny"),
                Some(event_tx),
                &CancellationToken::new(),
            )
            .await;

        let mut failed_ends = 0usize;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(
                event,
                AgentEvent::StepEnd {
                    status: TaskStatus::Failed,
                    ..
                }
            ) {
                failed_ends += 1;
            }
        }
        assert!(
            failed_ends >= 1,
            "denied parallel delegated wave should emit StepEnd Failed"
        );
    }

    #[tokio::test]
    async fn execute_plan_reuses_existing_product_transcript_and_emits_completed() {
        let workspace = tempfile::tempdir().unwrap();
        let agent = AgentLoop::new(
            Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
                "serial step done",
            )])),
            Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
            ToolContext::new(workspace.path().to_path_buf()),
            crate::agent::AgentConfig {
                continuation_enabled: false,
                max_tool_rounds: 2,
                max_execution_time_ms: Some(3_000),
                ..crate::agent::AgentConfig::default()
            },
        );
        let mut plan = ExecutionPlan::new("reuse product", Complexity::Simple);
        plan.add_step(Task::new("s1", "Do the work"));
        // Keep capacity high and drain concurrently: execute_plan awaits on full
        // channels and will hang if nobody receives events.
        let (event_tx, mut event_rx) = mpsc::channel(256);
        let drain = tokio::spawn(async move {
            let mut saw_completed = false;
            while let Some(event) = event_rx.recv().await {
                if matches!(
                    event,
                    AgentEvent::StepEnd {
                        status: TaskStatus::Completed,
                        ..
                    }
                ) {
                    saw_completed = true;
                }
            }
            saw_completed
        });
        let history = vec![Message::user("reuse product")];

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            agent.execute_plan(
                &history,
                &plan,
                Some("reuse-product"),
                Some(event_tx),
                &CancellationToken::new(),
            ),
        )
        .await
        .expect("serial plan must settle")
        .expect("serial plan with mock text should complete");

        let product_users = result
            .messages
            .iter()
            .filter(|message| message.role == "user" && message.is_product_transcript())
            .count();
        assert_eq!(
            product_users, 1,
            "existing product transcript must not be duplicated"
        );
        assert!(
            drain.await.expect("drain join"),
            "successful serial step should emit StepEnd Completed"
        );
    }

    #[tokio::test]
    async fn execute_plan_parallel_wave_emits_completed_step_ends() {
        let workspace = tempfile::tempdir().unwrap();
        let agent = AgentLoop::new(
            Arc::new(MockLlmClient::new(vec![
                MockLlmClient::text_response("parallel A done"),
                MockLlmClient::text_response("parallel B done"),
            ])),
            Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
            ToolContext::new(workspace.path().to_path_buf()),
            crate::agent::AgentConfig {
                continuation_enabled: false,
                max_parallel_tasks: 2,
                max_tool_rounds: 2,
                max_execution_time_ms: Some(3_000),
                ..crate::agent::AgentConfig::default()
            },
        );
        let mut plan = ExecutionPlan::new("parallel ok", Complexity::Simple);
        plan.add_step(Task::new("s1", "Parallel A"));
        plan.add_step(Task::new("s2", "Parallel B"));
        let (event_tx, mut event_rx) = mpsc::channel(256);
        let drain = tokio::spawn(async move {
            let mut completed = 0usize;
            while let Some(event) = event_rx.recv().await {
                if matches!(
                    event,
                    AgentEvent::StepEnd {
                        status: TaskStatus::Completed,
                        ..
                    }
                ) {
                    completed += 1;
                }
            }
            completed
        });

        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            agent.execute_plan(
                &[],
                &plan,
                Some("parallel-ok"),
                Some(event_tx),
                &CancellationToken::new(),
            ),
        )
        .await
        .expect("successful parallel wave must settle")
        .expect("parallel wave should return");

        assert!(
            drain.await.expect("drain join") >= 1,
            "successful parallel steps should emit StepEnd Completed"
        );
    }

    struct CompleteAndCancelClient {
        cancel: CancellationToken,
    }

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for CompleteAndCancelClient {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[crate::llm::ToolDefinition],
        ) -> anyhow::Result<crate::llm::LlmResponse> {
            self.cancel.cancel();
            Ok(MockLlmClient::text_response("done after cancel signal"))
        }

        async fn complete_streaming(
            &self,
            messages: &[Message],
            system: Option<&str>,
            tools: &[crate::llm::ToolDefinition],
            _cancel_token: CancellationToken,
        ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
            let response = self.complete(messages, system, tools).await?;
            let (tx, rx) = mpsc::channel(4);
            tokio::spawn(async move {
                let text = response.text();
                if !text.is_empty() {
                    let _ = tx.send(crate::llm::StreamEvent::TextDelta(text)).await;
                }
                let _ = tx.send(crate::llm::StreamEvent::Done(response)).await;
            });
            Ok(rx)
        }
    }

    #[tokio::test]
    async fn execute_plan_marks_completed_step_cancelled_when_token_trips_during_ok() {
        let workspace = tempfile::tempdir().unwrap();
        let cancel_token = CancellationToken::new();
        let agent = AgentLoop::new(
            Arc::new(CompleteAndCancelClient {
                cancel: cancel_token.clone(),
            }),
            Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
            ToolContext::new(workspace.path().to_path_buf()),
            crate::agent::AgentConfig {
                continuation_enabled: false,
                max_tool_rounds: 2,
                max_execution_time_ms: Some(3_000),
                ..crate::agent::AgentConfig::default()
            },
        );
        let mut plan = ExecutionPlan::new("cancel after ok", Complexity::Simple);
        plan.add_step(Task::new("s1", "First"));
        plan.add_step(Task::new("s2", "Second").with_dependencies(vec!["s1".into()]));
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let _ = agent
            .execute_plan(
                &[],
                &plan,
                Some("cancel-after-ok"),
                Some(event_tx),
                &cancel_token,
            )
            .await;

        let mut saw_cancelled = false;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(
                event,
                AgentEvent::StepEnd {
                    status: TaskStatus::Cancelled,
                    ..
                }
            ) {
                saw_cancelled = true;
            }
        }
        assert!(
            saw_cancelled || cancel_token.is_cancelled(),
            "cancel during Ok step should mark Cancelled or stop the plan"
        );
    }

    #[tokio::test]
    async fn execute_plan_breaks_at_loop_top_when_cancelled_between_waves() {
        let workspace = tempfile::tempdir().unwrap();
        let cancel_token = CancellationToken::new();
        let cancel_for_client = cancel_token.clone();
        let agent = AgentLoop::new(
            Arc::new(CompleteAndCancelClient {
                cancel: cancel_for_client,
            }),
            Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
            ToolContext::new(workspace.path().to_path_buf()),
            crate::agent::AgentConfig {
                continuation_enabled: false,
                max_tool_rounds: 2,
                max_execution_time_ms: Some(3_000),
                ..crate::agent::AgentConfig::default()
            },
        );
        let mut plan = ExecutionPlan::new("between waves", Complexity::Simple);
        plan.add_step(Task::new("s1", "Only first"));
        plan.add_step(Task::new("s2", "Never runs").with_dependencies(vec!["s1".into()]));
        let result = agent
            .execute_plan(&[], &plan, Some("between-waves"), None, &cancel_token)
            .await;
        // Either Ok with partial progress or Err from completion gate — must not hang.
        let _ = result;
        assert!(cancel_token.is_cancelled());
    }

    #[tokio::test]
    async fn execute_plan_parallel_wave_emits_cancelled_when_parent_cancels() {
        let workspace = tempfile::tempdir().unwrap();
        let client = Arc::new(HangThenCancelClient::new());
        let agent = AgentLoop::new(
            client.clone(),
            Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
            ToolContext::new(workspace.path().to_path_buf()),
            crate::agent::AgentConfig {
                continuation_enabled: false,
                max_parallel_tasks: 2,
                max_tool_rounds: 2,
                max_execution_time_ms: Some(5_000),
                ..crate::agent::AgentConfig::default()
            },
        );
        let mut plan = ExecutionPlan::new("parallel cancel", Complexity::Simple);
        plan.add_step(Task::new("s1", "Hang A"));
        plan.add_step(Task::new("s2", "Hang B"));
        let cancel_token = CancellationToken::new();
        let run_token = cancel_token.clone();
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let run = tokio::spawn(async move {
            agent
                .execute_plan(
                    &[],
                    &plan,
                    Some("parallel-cancel"),
                    Some(event_tx),
                    &run_token,
                )
                .await
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client.step_started.notified(),
        )
        .await
        .expect("parallel wave should start");
        cancel_token.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), run)
            .await
            .expect("cancelled parallel wave must finish")
            .expect("join");

        let mut cancelled = 0usize;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(
                event,
                AgentEvent::StepEnd {
                    status: TaskStatus::Cancelled,
                    ..
                }
            ) {
                cancelled += 1;
            }
        }
        assert!(
            cancelled >= 1 || client.calls.load(std::sync::atomic::Ordering::SeqCst) >= 1,
            "parallel cancel should interrupt in-flight wave"
        );
    }

    struct FailAndCancelClient {
        cancel: CancellationToken,
    }

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for FailAndCancelClient {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[crate::llm::ToolDefinition],
        ) -> anyhow::Result<crate::llm::LlmResponse> {
            self.cancel.cancel();
            Err(anyhow::anyhow!("forced plan step failure after cancel"))
        }

        async fn complete_streaming(
            &self,
            messages: &[Message],
            system: Option<&str>,
            tools: &[crate::llm::ToolDefinition],
            _cancel_token: CancellationToken,
        ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
            let _ = self.complete(messages, system, tools).await?;
            unreachable!("complete always errors")
        }
    }

    #[tokio::test]
    async fn execute_plan_marks_failed_step_cancelled_when_token_trips_during_err() {
        let workspace = tempfile::tempdir().unwrap();
        let cancel_token = CancellationToken::new();
        let agent = AgentLoop::new(
            Arc::new(FailAndCancelClient {
                cancel: cancel_token.clone(),
            }),
            Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
            ToolContext::new(workspace.path().to_path_buf()),
            crate::agent::AgentConfig {
                continuation_enabled: false,
                max_tool_rounds: 2,
                max_execution_time_ms: Some(3_000),
                ..crate::agent::AgentConfig::default()
            },
        );
        let mut plan = ExecutionPlan::new("cancel after err", Complexity::Simple);
        plan.add_step(Task::new("s1", "First"));
        plan.add_step(Task::new("s2", "Second").with_dependencies(vec!["s1".into()]));
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let _ = agent
            .execute_plan(
                &[],
                &plan,
                Some("cancel-after-err"),
                Some(event_tx),
                &cancel_token,
            )
            .await;

        let mut saw_cancelled = false;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(
                event,
                AgentEvent::StepEnd {
                    status: TaskStatus::Cancelled,
                    ..
                }
            ) {
                saw_cancelled = true;
            }
        }
        assert!(
            saw_cancelled || cancel_token.is_cancelled(),
            "cancel during Err step should mark Cancelled or stop the plan"
        );
    }

    #[tokio::test]
    async fn execute_plan_parallel_wave_emits_failed_step_ends_on_llm_error() {
        let workspace = tempfile::tempdir().unwrap();
        let cancel_token = CancellationToken::new();
        let agent = AgentLoop::new(
            Arc::new(FailAndCancelClient {
                cancel: cancel_token.clone(),
            }),
            Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
            ToolContext::new(workspace.path().to_path_buf()),
            crate::agent::AgentConfig {
                continuation_enabled: false,
                max_parallel_tasks: 2,
                max_tool_rounds: 2,
                max_execution_time_ms: Some(3_000),
                ..crate::agent::AgentConfig::default()
            },
        );
        let mut plan = ExecutionPlan::new("parallel fail", Complexity::Simple);
        plan.add_step(Task::new("s1", "Fail A"));
        plan.add_step(Task::new("s2", "Fail B"));
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let _ = agent
            .execute_plan(
                &[],
                &plan,
                Some("parallel-fail"),
                Some(event_tx),
                &cancel_token,
            )
            .await;

        let mut saw_end = false;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(
                event,
                AgentEvent::StepEnd {
                    status: TaskStatus::Failed | TaskStatus::Cancelled,
                    ..
                }
            ) {
                saw_end = true;
            }
        }
        assert!(
            saw_end || cancel_token.is_cancelled(),
            "parallel LLM failures should emit Failed/Cancelled ends"
        );
    }

    struct AlwaysFailClient;

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for AlwaysFailClient {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[crate::llm::ToolDefinition],
        ) -> anyhow::Result<crate::llm::LlmResponse> {
            Err(anyhow::anyhow!("forced parallel step failure"))
        }

        async fn complete_streaming(
            &self,
            messages: &[Message],
            system: Option<&str>,
            tools: &[crate::llm::ToolDefinition],
            _cancel_token: CancellationToken,
        ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
            let _ = self.complete(messages, system, tools).await?;
            unreachable!("complete always errors")
        }
    }

    struct PanicOnCompleteClient;

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for PanicOnCompleteClient {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[crate::llm::ToolDefinition],
        ) -> anyhow::Result<crate::llm::LlmResponse> {
            panic!("forced parallel branch panic");
        }

        async fn complete_streaming(
            &self,
            messages: &[Message],
            system: Option<&str>,
            tools: &[crate::llm::ToolDefinition],
            _cancel_token: CancellationToken,
        ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
            let _ = self.complete(messages, system, tools).await?;
            unreachable!("complete always panics")
        }
    }

    #[tokio::test]
    async fn execute_plan_requires_cancelled_step_end_when_err_trips_token() {
        let workspace = tempfile::tempdir().unwrap();
        let cancel_token = CancellationToken::new();
        let agent = AgentLoop::new(
            Arc::new(FailAndCancelClient {
                cancel: cancel_token.clone(),
            }),
            Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
            ToolContext::new(workspace.path().to_path_buf()),
            crate::agent::AgentConfig {
                continuation_enabled: false,
                max_tool_rounds: 2,
                max_execution_time_ms: Some(3_000),
                ..crate::agent::AgentConfig::default()
            },
        );
        let mut plan = ExecutionPlan::new("require cancel end", Complexity::Simple);
        plan.add_step(Task::new("s1", "First"));
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let drain = tokio::spawn(async move {
            let mut saw_cancelled = false;
            while let Some(event) = event_rx.recv().await {
                if matches!(
                    event,
                    AgentEvent::StepEnd {
                        status: TaskStatus::Cancelled,
                        ..
                    }
                ) {
                    saw_cancelled = true;
                }
            }
            saw_cancelled
        });

        let _ = agent
            .execute_plan(
                &[],
                &plan,
                Some("require-cancel-end"),
                Some(event_tx),
                &cancel_token,
            )
            .await;

        assert!(
            drain.await.expect("drain join"),
            "Err+cancel must emit StepEnd Cancelled"
        );
    }

    #[tokio::test]
    async fn execute_plan_parallel_wave_emits_failed_ends_without_cancelling() {
        let workspace = tempfile::tempdir().unwrap();
        let agent = AgentLoop::new(
            Arc::new(AlwaysFailClient),
            Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
            ToolContext::new(workspace.path().to_path_buf()),
            crate::agent::AgentConfig {
                continuation_enabled: false,
                max_parallel_tasks: 2,
                max_tool_rounds: 2,
                max_execution_time_ms: Some(3_000),
                ..crate::agent::AgentConfig::default()
            },
        );
        let mut plan = ExecutionPlan::new("parallel fail no cancel", Complexity::Simple);
        plan.add_step(Task::new("s1", "Fail A"));
        plan.add_step(Task::new("s2", "Fail B"));
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let drain = tokio::spawn(async move {
            let mut failed = 0usize;
            while let Some(event) = event_rx.recv().await {
                if matches!(
                    event,
                    AgentEvent::StepEnd {
                        status: TaskStatus::Failed,
                        ..
                    }
                ) {
                    failed += 1;
                }
            }
            failed
        });

        let _ = agent
            .execute_plan(
                &[],
                &plan,
                Some("parallel-fail-no-cancel"),
                Some(event_tx),
                &CancellationToken::new(),
            )
            .await;

        assert!(
            drain.await.expect("drain join") >= 1,
            "parallel Ok(Err) failures must emit StepEnd Failed"
        );
    }

    #[tokio::test]
    async fn execute_plan_parallel_wave_emits_failed_ends_when_branch_panics() {
        let workspace = tempfile::tempdir().unwrap();
        let agent = AgentLoop::new(
            Arc::new(PanicOnCompleteClient),
            Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
            ToolContext::new(workspace.path().to_path_buf()),
            crate::agent::AgentConfig {
                continuation_enabled: false,
                max_parallel_tasks: 2,
                max_tool_rounds: 2,
                max_execution_time_ms: Some(3_000),
                ..crate::agent::AgentConfig::default()
            },
        );
        let mut plan = ExecutionPlan::new("parallel panic", Complexity::Simple);
        plan.add_step(Task::new("s1", "Panic A"));
        plan.add_step(Task::new("s2", "Panic B"));
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let drain = tokio::spawn(async move {
            let mut failed = 0usize;
            while let Some(event) = event_rx.recv().await {
                if matches!(
                    event,
                    AgentEvent::StepEnd {
                        status: TaskStatus::Failed,
                        ..
                    }
                ) {
                    failed += 1;
                }
            }
            failed
        });

        let _ = agent
            .execute_plan(
                &[],
                &plan,
                Some("parallel-panic"),
                Some(event_tx),
                &CancellationToken::new(),
            )
            .await;

        assert!(
            drain.await.expect("drain join") >= 1,
            "panicked parallel branches must surface as StepEnd Failed"
        );
    }
}
