use super::{AgentEvent, AgentLoop, AgentResult, InvocationContext};
use crate::hooks::ErrorType;
use crate::llm::Message;
use crate::planning::{LlmPlanner, PreAnalysis};
use crate::prompts::{AgentStyle, PlanningMode};
use anyhow::Result;
use tokio::sync::mpsc;

struct ExecutionRoute {
    style: AgentStyle,
    use_planning: bool,
    effective_prompt: String,
    pre_analysis: Option<PreAnalysis>,
}

impl AgentLoop {
    pub(super) fn preserve_original_prompt_for_execution(
        original_prompt: &str,
        optimized_input: &str,
    ) -> String {
        let original = original_prompt.trim();
        let optimized = optimized_input.trim();

        if original.is_empty() {
            return optimized.to_string();
        }
        if optimized.is_empty() || optimized == original {
            return original.to_string();
        }
        if optimized.contains(original) {
            return optimized.to_string();
        }

        format!("Original user request:\n{original}\n\nPlanner-optimized request:\n{optimized}")
    }

    pub(super) fn should_run_pre_analysis(&self) -> bool {
        match self.config.planning_mode {
            PlanningMode::Disabled => false,
            PlanningMode::Enabled => true,
            PlanningMode::Auto => true,
        }
    }

    /// Execute the agent loop for a prompt with session context
    ///
    /// Takes the conversation history, user prompt, and optional session ID.
    /// When session_id is provided, context providers can use it for session-specific context.
    pub async fn execute_with_session(
        &self,
        history: &[Message],
        prompt: &str,
        session_id: Option<&str>,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
        cancel_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<AgentResult> {
        let default_token = tokio_util::sync::CancellationToken::new();
        let token = cancel_token.unwrap_or(&default_token).clone();
        let run_id = self
            .checkpoint_run_id
            .clone()
            .unwrap_or_else(|| format!("standalone-{}", uuid::Uuid::new_v4()));
        let invocation = self.invocation_context(run_id, session_id, event_tx, token);
        self.execute_with_invocation(history, prompt, &invocation)
            .await
    }

    /// Execute using one immutable per-run context shared by routing, model,
    /// planning, and tool paths.
    pub(crate) async fn execute_with_invocation(
        &self,
        history: &[Message],
        prompt: &str,
        invocation: &InvocationContext,
    ) -> Result<AgentResult> {
        let agent = invocation.bind_agent_loop(self);
        let session_id = invocation.session_id_option();
        let event_tx = invocation.event_tx().clone();
        let token = invocation.cancellation();
        tracing::info!(
            a3s.run.id = invocation.run_id(),
            a3s.session.id = session_id.unwrap_or("none"),
            a3s.agent.max_turns = agent.config.max_tool_rounds,
            "a3s.agent.execute started"
        );

        let route = match agent
            .resolve_execution_route(prompt, session_id, &event_tx, token)
            .await
        {
            Ok(route) => route,
            Err(error) => {
                if let Some(message) = crate::llm::non_retryable_llm_error_message(&error) {
                    if let Some(tx) = &event_tx {
                        tx.send(AgentEvent::Error {
                            message: message.to_string(),
                        })
                        .await
                        .ok();
                    }
                }
                return Err(error);
            }
        };
        if token.is_cancelled() {
            anyhow::bail!("Operation cancelled by user");
        }
        let mut effective_prompt = route.effective_prompt.clone();
        let mut auto_tool_calls_count = 0;
        if !route.use_planning {
            if let Some(outcome) = agent
                .maybe_apply_auto_delegation(&effective_prompt, session_id, &event_tx, token)
                .await?
            {
                effective_prompt = outcome.prompt;
                auto_tool_calls_count = outcome.tool_calls_count;
            }
        }

        let mut result = if route.use_planning {
            agent
                .execute_with_planning(
                    history,
                    &effective_prompt,
                    session_id,
                    event_tx,
                    route.pre_analysis,
                    token,
                )
                .await
        } else {
            agent
                .execute_loop(
                    history,
                    &effective_prompt,
                    route.style,
                    session_id,
                    event_tx,
                    token,
                    true,
                )
                .await
        };
        if let Ok(result) = &mut result {
            result.tool_calls_count += auto_tool_calls_count;
        }

        agent.record_execution_result(session_id, &result).await;
        result
    }

    async fn resolve_execution_route(
        &self,
        prompt: &str,
        session_id: Option<&str>,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> Result<ExecutionRoute> {
        let pre_analysis = self
            .run_pre_analysis(prompt, session_id, event_tx, cancel_token)
            .await?;
        let style = self.resolve_execution_style(prompt, pre_analysis.as_ref());
        let use_planning = self.resolve_planning_decision(style, pre_analysis.as_ref());
        let effective_prompt = pre_analysis
            .as_ref()
            .map(|analysis| {
                Self::preserve_original_prompt_for_execution(prompt, &analysis.optimized_input)
            })
            .unwrap_or_else(|| prompt.to_string());

        Ok(ExecutionRoute {
            style,
            use_planning,
            effective_prompt,
            pre_analysis,
        })
    }

    async fn run_pre_analysis(
        &self,
        prompt: &str,
        session_id: Option<&str>,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> Result<Option<PreAnalysis>> {
        if !self.should_run_pre_analysis() {
            return Ok(None);
        }

        let llm_client = self.scoped_llm_client_for_parts(session_id, event_tx, cancel_token);
        let result = LlmPlanner::pre_analyze(&llm_client, prompt).await;

        match result {
            Ok(analysis) => {
                tracing::debug!(
                    intent = ?analysis.intent,
                    requires_planning = analysis.requires_planning,
                    plan_steps = analysis.execution_plan.steps.len(),
                    "Pre-analysis completed"
                );
                Ok(Some(analysis))
            }
            Err(e) if Self::planning_control_error(&e, cancel_token) => Err(e),
            Err(e) => {
                tracing::warn!(error = %e, "Pre-analysis failed; using local style fallback");
                Ok(None)
            }
        }
    }

    fn resolve_execution_style(
        &self,
        prompt: &str,
        pre_analysis: Option<&PreAnalysis>,
    ) -> AgentStyle {
        if let Some(analysis) = pre_analysis {
            return analysis.intent;
        }

        let (style, confidence) = AgentStyle::detect_with_confidence(prompt);
        tracing::debug!(
            intent.classification = ?style,
            intent.confidence = ?confidence,
            intent.source = "local_fallback",
            "Intent classified locally"
        );
        style
    }

    pub(super) fn resolve_planning_decision(
        &self,
        _style: AgentStyle,
        pre_analysis: Option<&PreAnalysis>,
    ) -> bool {
        match self.config.planning_mode {
            PlanningMode::Disabled => false,
            PlanningMode::Enabled => true,
            PlanningMode::Auto => pre_analysis
                .map(|analysis| analysis.requires_planning)
                .unwrap_or(false),
        }
    }

    async fn record_execution_result(
        &self,
        session_id: Option<&str>,
        result: &Result<AgentResult>,
    ) {
        match result {
            Ok(r) => {
                tracing::info!(
                    a3s.agent.tool_calls_count = r.tool_calls_count,
                    a3s.llm.total_tokens = r.usage.total_tokens,
                    "a3s.agent.execute completed"
                );
                self.config.rl_trajectory_recorder.record_execution_end(
                    session_id.unwrap_or(""),
                    true,
                    Some(&r.text),
                    Some(&r.usage),
                    Some(r.tool_calls_count),
                    None,
                );
                self.fire_post_response(
                    session_id.unwrap_or(""),
                    &r.text,
                    r.tool_calls_count,
                    &r.usage,
                    0,
                )
                .await;
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "a3s.agent.execute failed"
                );
                self.config.rl_trajectory_recorder.record_execution_end(
                    session_id.unwrap_or(""),
                    false,
                    None,
                    None,
                    None,
                    Some(&e.to_string()),
                );
                self.fire_on_error(
                    session_id.unwrap_or(""),
                    ErrorType::Other,
                    &e.to_string(),
                    serde_json::json!({"phase": "execute"}),
                )
                .await;
            }
        }
    }
}
