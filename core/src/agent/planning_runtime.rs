use super::{AgentEvent, AgentLoop, AgentResult};
use crate::llm::Message;
use crate::planning::{AgentGoal, ExecutionPlan, LlmPlanner, PreAnalysis};
use anyhow::Result;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

impl AgentLoop {
    pub(super) fn preserve_plan_goal_context(
        mut plan: ExecutionPlan,
        execution_prompt: &str,
    ) -> ExecutionPlan {
        let context = execution_prompt.trim();
        let goal = plan.goal.trim();

        if context.is_empty() || goal == context || goal.contains(context) {
            return plan;
        }

        plan.goal = format!(
            "Original user request and planning context:\n{context}\n\nPlanner goal:\n{goal}"
        );
        plan
    }

    pub(super) async fn emit_task_updated(
        &self,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        session_id: &str,
        plan: &ExecutionPlan,
    ) {
        if let Some(tx) = event_tx {
            tx.send(AgentEvent::TaskUpdated {
                session_id: session_id.to_string(),
                tasks: plan.steps.clone(),
            })
            .await
            .ok();
        }
    }

    async fn plan_scoped(
        &self,
        prompt: &str,
        session_id: Option<&str>,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &CancellationToken,
    ) -> Result<ExecutionPlan> {
        let llm_client = self.scoped_llm_client_for_parts(session_id, event_tx, cancel_token);
        match LlmPlanner::create_plan(&llm_client, prompt).await {
            Ok(plan) => Ok(plan),
            Err(e) if Self::planning_control_error(&e, cancel_token) => Err(e),
            Err(e) => {
                tracing::warn!("LLM plan creation failed, using fallback: {}", e);
                Ok(LlmPlanner::fallback_plan(prompt))
            }
        }
    }

    /// Execute with planning phase.
    ///
    /// If `pre_analysis` is provided (from a single pre-analysis LLM call in
    /// `execute_with_session`), the goal and plan are already available and no
    /// additional LLM calls are needed for planning. Otherwise, falls back to
    /// calling `extract_goal` and `plan` individually.
    pub async fn execute_with_planning(
        &self,
        history: &[Message],
        prompt: &str,
        session_id: Option<&str>,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
        pre_analysis: Option<PreAnalysis>,
        cancel_token: &CancellationToken,
    ) -> Result<AgentResult> {
        if cancel_token.is_cancelled() {
            anyhow::bail!("Operation cancelled by user");
        }
        let session_id_str = session_id.unwrap_or("");
        let planning_prompt = self.fire_pre_planning(session_id_str, prompt).await?;
        if cancel_token.is_cancelled() {
            anyhow::bail!("Operation cancelled by user");
        }
        let pre_analysis = if planning_prompt == prompt {
            pre_analysis
        } else {
            None
        };

        // Send planning start event
        if let Some(tx) = &event_tx {
            tx.send(AgentEvent::PlanningStart {
                prompt: planning_prompt.clone(),
            })
            .await
            .ok();
        }

        // Use pre-analysis result if available (goal + plan already computed in one LLM call).
        let planning_result: Result<(Option<AgentGoal>, ExecutionPlan)> = async {
            if let Some(analysis) = pre_analysis {
                Ok((
                    Some(analysis.goal.clone()),
                    Self::preserve_plan_goal_context(
                        analysis.execution_plan.clone(),
                        &planning_prompt,
                    ),
                ))
            } else {
                // Fall back: extract goal and create plan via separate LLM calls.
                let g = if self.config.goal_tracking {
                    Some(
                        self.extract_goal_scoped(
                            &planning_prompt,
                            session_id,
                            &event_tx,
                            cancel_token,
                        )
                        .await?,
                    )
                } else {
                    None
                };
                let p = self
                    .plan_scoped(&planning_prompt, session_id, &event_tx, cancel_token)
                    .await?;
                Ok((g, p))
            }
        }
        .await;

        let (goal, plan) = match planning_result {
            Ok(result) => {
                self.fire_post_planning(session_id_str, &planning_prompt, Some(&result.1), None)
                    .await;
                result
            }
            Err(err) => {
                let message = err.to_string();
                self.fire_post_planning(session_id_str, &planning_prompt, None, Some(&message))
                    .await;
                return Err(err);
            }
        };

        // Send GoalExtracted event if goal_tracking is enabled.
        if self.config.goal_tracking {
            if let Some(ref g) = goal {
                if let Some(tx) = &event_tx {
                    tx.send(AgentEvent::GoalExtracted { goal: g.clone() })
                        .await
                        .ok();
                }
            }
        }

        // Send planning end event
        if let Some(tx) = &event_tx {
            tx.send(AgentEvent::PlanningEnd {
                estimated_steps: plan.steps.len(),
                plan: plan.clone(),
            })
            .await
            .ok();
        }

        let plan_start = std::time::Instant::now();

        // Execute the plan step by step
        let result = self
            .execute_plan(history, &plan, session_id, event_tx.clone(), cancel_token)
            .await?;

        // Evaluate and emit goal achievement before the terminal End event.
        // Consumers use End as the boundary at which they decide whether an
        // engineered goal must continue, so emitting GoalAchieved afterwards
        // made a verified goal indistinguishable from an unfinished one.
        if self.config.goal_tracking {
            if let Some(ref g) = goal {
                let evaluation_state = if result.verification_reports.is_empty() {
                    result.text.clone()
                } else {
                    format!(
                        "Assistant result:\n{}\n\nStructured verification evidence:\n{}",
                        result.text,
                        result.verification_summary_text(),
                    )
                };
                let achieved = self
                    .check_goal_achievement_scoped(
                        g,
                        &evaluation_state,
                        session_id,
                        &event_tx,
                        cancel_token,
                    )
                    .await?;
                if achieved {
                    if let Some(tx) = &event_tx {
                        tx.send(AgentEvent::GoalAchieved {
                            goal: g.description.clone(),
                            total_steps: result.messages.len(),
                            duration_ms: plan_start.elapsed().as_millis() as i64,
                        })
                        .await
                        .ok();
                    }
                }
            }
        }

        // Emit the final End event (execute_loop_inner does not emit End in planning mode).
        // It must remain the terminal lifecycle event after all goal signals.
        if let Some(tx) = &event_tx {
            tx.send(AgentEvent::End {
                text: result.text.clone(),
                usage: result.usage.clone(),
                verification_summary: Box::new(result.verification_summary()),
                meta: None,
            })
            .await
            .ok();
        }

        Ok(result)
    }

    /// Extract goal from prompt
    ///
    /// Delegates to [`LlmPlanner`] for structured JSON goal extraction,
    /// falling back to heuristic logic if the LLM call fails.
    #[cfg(test)]
    pub async fn extract_goal(
        &self,
        prompt: &str,
        cancel_token: &CancellationToken,
    ) -> Result<AgentGoal> {
        self.extract_goal_scoped(prompt, None, &None, cancel_token)
            .await
    }

    async fn extract_goal_scoped(
        &self,
        prompt: &str,
        session_id: Option<&str>,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &CancellationToken,
    ) -> Result<AgentGoal> {
        let llm_client = self.scoped_llm_client_for_parts(session_id, event_tx, cancel_token);
        match LlmPlanner::extract_goal(&llm_client, prompt).await {
            Ok(goal) => Ok(goal),
            Err(e) if Self::planning_control_error(&e, cancel_token) => Err(e),
            Err(e) => {
                tracing::warn!("LLM goal extraction failed, using fallback: {}", e);
                Ok(LlmPlanner::fallback_goal(prompt))
            }
        }
    }

    /// Check if goal is achieved
    ///
    /// Delegates to [`LlmPlanner`] for structured JSON achievement check,
    /// falling back to heuristic logic if the LLM call fails.
    #[cfg(test)]
    pub async fn check_goal_achievement(
        &self,
        goal: &AgentGoal,
        current_state: &str,
        cancel_token: &CancellationToken,
    ) -> Result<bool> {
        self.check_goal_achievement_scoped(goal, current_state, None, &None, cancel_token)
            .await
    }

    async fn check_goal_achievement_scoped(
        &self,
        goal: &AgentGoal,
        current_state: &str,
        session_id: Option<&str>,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &CancellationToken,
    ) -> Result<bool> {
        let llm_client = self.scoped_llm_client_for_parts(session_id, event_tx, cancel_token);
        match LlmPlanner::check_achievement(&llm_client, goal, current_state).await {
            Ok(result) => Ok(result.achieved),
            Err(e) if Self::planning_control_error(&e, cancel_token) => Err(e),
            Err(e) => {
                tracing::warn!("LLM achievement check failed, using fallback: {}", e);
                let result = LlmPlanner::fallback_check_achievement(goal, current_state);
                Ok(result.achieved)
            }
        }
    }

    pub(super) fn planning_control_error(
        error: &anyhow::Error,
        cancel_token: &CancellationToken,
    ) -> bool {
        cancel_token.is_cancelled()
            || crate::llm::non_retryable_llm_error_message(error).is_some()
            || error
                .downcast_ref::<crate::error::CodeError>()
                .is_some_and(|error| {
                    matches!(error, crate::error::CodeError::BudgetExhausted { .. })
                })
    }
}
