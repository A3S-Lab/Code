use super::execution_state::ExecutionLoopState;
use super::{AgentEvent, AgentLoop};
use crate::hooks::{
    ErrorType, GenerateEndEvent, GenerateStartEvent, HookEvent, TokenUsageInfo, ToolCallInfo,
};
use crate::llm::{
    estimate_prompt_tokens, non_retryable_llm_error_message, LlmResponse, Message, ToolCall,
    ToolDefinition,
};
use crate::retry::RetryConfig;
use anyhow::Context;
use std::time::Duration;
use tokio::sync::mpsc;

const DEFAULT_AUTO_COMPACT_TIMEOUT_MS: u64 = 60_000;
const MAX_STREAM_INTERRUPTION_RETRIES: u32 = 10;

#[derive(Debug, thiserror::Error)]
#[error("LLM response stream ended before the final response")]
struct IncompleteLlmStream;

fn stream_interruption_retry_delay(retry_index: u32) -> Duration {
    RetryConfig::default().delay_for_attempt(retry_index)
}

pub(super) struct LlmTurnOutput {
    pub(super) turn: usize,
    pub(super) response: LlmResponse,
    pub(super) tool_calls: Vec<ToolCall>,
}

pub(super) struct LlmTurnRequest<'a> {
    pub(super) augmented_system: &'a Option<String>,
    pub(super) effective_prompt: &'a str,
    pub(super) session_id: Option<&'a str>,
    pub(super) event_tx: &'a Option<mpsc::Sender<AgentEvent>>,
    pub(super) cancel_token: &'a tokio_util::sync::CancellationToken,
    pub(super) force_no_tools: bool,
}

struct LlmCallRequest<'a> {
    turn: usize,
    messages: &'a [Message],
    system: Option<&'a str>,
    tools: &'a [ToolDefinition],
    session_id: Option<&'a str>,
    event_tx: &'a Option<mpsc::Sender<AgentEvent>>,
    cancel_token: &'a tokio_util::sync::CancellationToken,
}

fn is_budget_exhausted(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<crate::error::CodeError>()
        .is_some_and(|error| matches!(error, crate::error::CodeError::BudgetExhausted { .. }))
}

impl AgentLoop {
    pub(super) async fn execute_llm_turn(
        &self,
        state: &mut ExecutionLoopState,
        request: LlmTurnRequest<'_>,
    ) -> anyhow::Result<LlmTurnOutput> {
        let LlmTurnRequest {
            augmented_system,
            effective_prompt,
            session_id,
            event_tx,
            cancel_token,
            force_no_tools,
        } = request;
        let turn = state.next_turn();
        self.ensure_turn_can_start(turn, state, event_tx, force_no_tools)
            .await?;
        self.emit_turn_start(turn, event_tx).await;

        let mut selected_tools = if force_no_tools {
            Vec::new()
        } else {
            crate::tools::select_tools_for_messages(&self.config.tools, &state.messages)
        };
        if let Some(permission_checker) = &self.config.permission_checker {
            selected_tools.retain(|tool| permission_checker.expose_to_model(&tool.name));
        }
        let estimated_prompt_tokens = estimate_prompt_tokens(
            &state.messages,
            augmented_system.as_deref(),
            &selected_tools,
        );
        let pre_compaction_fixed_prompt_tokens =
            estimate_prompt_tokens(&[], augmented_system.as_deref(), &selected_tools);
        let compacted_before_call = self
            .maybe_auto_compact(
                state,
                estimated_prompt_tokens,
                pre_compaction_fixed_prompt_tokens,
                session_id,
                event_tx,
                cancel_token,
            )
            .await;

        // Compaction can change the history-sensitive tool selection. Rebuild
        // the request definitions from the context that will actually be sent.
        selected_tools = if force_no_tools {
            Vec::new()
        } else {
            crate::tools::select_tools_for_messages(&self.config.tools, &state.messages)
        };
        if let Some(permission_checker) = &self.config.permission_checker {
            selected_tools.retain(|tool| permission_checker.expose_to_model(&tool.name));
        }
        let request_fixed_prompt_tokens =
            estimate_prompt_tokens(&[], augmented_system.as_deref(), &selected_tools);

        tracing::info!(
            a3s.llm.streaming = event_tx.is_some(),
            "LLM completion started"
        );

        self.config.rl_trajectory_recorder.record_llm_request(
            session_id.unwrap_or(""),
            turn,
            &state.messages,
            augmented_system.as_deref(),
            &selected_tools,
            estimate_prompt_tokens(
                &state.messages,
                augmented_system.as_deref(),
                &selected_tools,
            ),
        );

        self.fire_generate_start(session_id.unwrap_or(""), effective_prompt, augmented_system)
            .await;

        let llm_start = std::time::Instant::now();
        let response = self
            .call_llm_with_circuit_breaker(LlmCallRequest {
                turn,
                messages: &state.messages,
                system: augmented_system.as_deref(),
                tools: &selected_tools,
                session_id,
                event_tx,
                cancel_token,
            })
            .await?;

        state.record_usage(&response.usage);
        self.complete_llm_turn(
            turn,
            effective_prompt,
            &response,
            llm_start,
            event_tx,
            session_id,
        )
        .await;

        state.messages.push(response.message.clone());
        let tool_calls = response.tool_calls();
        self.emit_turn_end(turn, &response, event_tx).await;
        if !compacted_before_call {
            self.maybe_auto_compact(
                state,
                response.usage.prompt_tokens,
                request_fixed_prompt_tokens,
                session_id,
                event_tx,
                cancel_token,
            )
            .await;
        }

        Ok(LlmTurnOutput {
            turn,
            response,
            tool_calls,
        })
    }

    async fn ensure_turn_can_start(
        &self,
        turn: usize,
        state: &ExecutionLoopState,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        force_no_tools: bool,
    ) -> anyhow::Result<()> {
        if let Some(error) = state.check_execution_timeout(self.config.max_execution_time_ms) {
            tracing::warn!(
                elapsed_ms = state.elapsed_ms(),
                max_time_ms = self.config.max_execution_time_ms.unwrap_or_default(),
                turns = turn.saturating_sub(1),
                "Execution timeout exceeded"
            );
            self.emit_error(event_tx, error.clone()).await;
            anyhow::bail!(error);
        }

        let is_reserved_finalization_turn =
            force_no_tools && turn == self.config.max_tool_rounds.saturating_add(1);
        if !is_reserved_finalization_turn {
            if let Some(error) = state.turn_limit_error(self.config.max_tool_rounds) {
                self.emit_error(event_tx, error.clone()).await;
                anyhow::bail!(error);
            }
        }

        Ok(())
    }

    async fn emit_turn_start(&self, turn: usize, event_tx: &Option<mpsc::Sender<AgentEvent>>) {
        if let Some(tx) = event_tx {
            tx.send(AgentEvent::TurnStart { turn }).await.ok();
        }

        tracing::info!(
            turn = turn,
            max_turns = self.config.max_tool_rounds,
            "Agent turn started"
        );
    }

    async fn call_llm_with_circuit_breaker(
        &self,
        request: LlmCallRequest<'_>,
    ) -> anyhow::Result<LlmResponse> {
        let threshold = self.config.circuit_breaker_threshold.max(1);
        let mut attempt = 0u32;
        let mut stream_retries = 0u32;
        let llm_client = self.scoped_llm_client_for_parts(
            request.session_id,
            request.event_tx,
            request.cancel_token,
        );

        loop {
            attempt += 1;
            let result = self
                .call_llm(
                    &llm_client,
                    request.messages,
                    request.system,
                    request.tools,
                    request.event_tx,
                    request.cancel_token,
                )
                .await;
            match result {
                Ok(response) => return Ok(response),
                Err(error) => {
                    if request.cancel_token.is_cancelled() {
                        anyhow::bail!(error);
                    }

                    // A host budget denial is a control decision, not a
                    // transient provider failure. Retrying would bypass the
                    // denied check on a later attempt and can overspend.
                    if is_budget_exhausted(&error) {
                        return Err(error);
                    }

                    let non_retryable_message = non_retryable_llm_error_message(&error);
                    let stream_interrupted = error.downcast_ref::<IncompleteLlmStream>().is_some();
                    if stream_interrupted
                        && non_retryable_message.is_none()
                        && stream_retries < MAX_STREAM_INTERRUPTION_RETRIES
                    {
                        let retry_index = stream_retries;
                        stream_retries += 1;
                        let delay = stream_interruption_retry_delay(retry_index);
                        tracing::warn!(
                            turn = request.turn,
                            attempt,
                            retry = stream_retries,
                            max_retries = MAX_STREAM_INTERRUPTION_RETRIES,
                            delay_ms = delay.as_millis() as u64,
                            error = %error,
                            "LLM response stream was interrupted; restarting the same turn"
                        );

                        // Re-emitting the same turn number is the transactional
                        // boundary for stream consumers: discard provisional
                        // deltas/tool drafts from the failed attempt, but keep
                        // the original user message and all prior turns.
                        self.emit_turn_start(request.turn, request.event_tx).await;
                        tokio::select! {
                            biased;
                            _ = request.cancel_token.cancelled() => {
                                anyhow::bail!("Operation cancelled by user")
                            }
                            _ = tokio::time::sleep(delay) => {}
                        }
                        continue;
                    }

                    if !stream_interrupted
                        && non_retryable_message.is_none()
                        && attempt < threshold
                        && (request.event_tx.is_none() || attempt == 1)
                    {
                        tracing::warn!(
                            turn = request.turn,
                            attempt = attempt,
                            threshold = threshold,
                            error = %error,
                            "LLM call failed, will retry"
                        );
                        tokio::select! {
                            biased;
                            _ = request.cancel_token.cancelled() => {
                                anyhow::bail!("Operation cancelled by user")
                            }
                            _ = tokio::time::sleep(Duration::from_millis(100 * attempt as u64)) => {}
                        }
                        continue;
                    }

                    let msg = if let Some(message) = non_retryable_message {
                        message.to_string()
                    } else if stream_interrupted {
                        format!(
                            "LLM response stream interrupted after {} retries ({} attempts): {}",
                            stream_retries, attempt, error
                        )
                    } else if attempt > 1 {
                        format!(
                            "LLM circuit breaker triggered: failed after {} attempt(s): {}",
                            attempt, error
                        )
                    } else {
                        format!("LLM call failed: {}", error)
                    };
                    tracing::error!(turn = request.turn, attempt = attempt, "{}", msg);
                    self.fire_on_error(
                        request.session_id.unwrap_or(""),
                        ErrorType::LlmFailure,
                        &msg,
                        serde_json::json!({"turn": request.turn, "attempt": attempt}),
                    )
                    .await;
                    self.emit_error(request.event_tx, msg.clone()).await;
                    anyhow::bail!(msg);
                }
            }
        }
    }

    async fn complete_llm_turn(
        &self,
        turn: usize,
        effective_prompt: &str,
        response: &LlmResponse,
        llm_start: std::time::Instant,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        session_id: Option<&str>,
    ) {
        let llm_duration = llm_start.elapsed();
        tracing::info!(
            turn = turn,
            streaming = event_tx.is_some(),
            prompt_tokens = response.usage.prompt_tokens,
            completion_tokens = response.usage.completion_tokens,
            total_tokens = response.usage.total_tokens,
            stop_reason = response.stop_reason.as_deref().unwrap_or("unknown"),
            duration_ms = llm_duration.as_millis() as u64,
            "LLM completion finished"
        );

        self.fire_generate_end(
            session_id.unwrap_or(""),
            effective_prompt,
            response,
            llm_duration.as_millis() as u64,
        )
        .await;

        crate::telemetry::record_llm_usage(
            response.usage.prompt_tokens,
            response.usage.completion_tokens,
            response.usage.total_tokens,
            response.stop_reason.as_deref(),
        );
        tracing::info!(
            turn = turn,
            a3s.llm.total_tokens = response.usage.total_tokens,
            "Turn token usage"
        );
        self.config.rl_trajectory_recorder.record_llm_response(
            session_id.unwrap_or(""),
            turn,
            response,
            llm_duration.as_millis() as u64,
        );
    }

    async fn emit_turn_end(
        &self,
        turn: usize,
        response: &LlmResponse,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) {
        if let Some(tx) = event_tx {
            tx.send(AgentEvent::TurnEnd {
                turn,
                usage: response.usage.clone(),
            })
            .await
            .ok();
        }
    }

    async fn maybe_auto_compact(
        &self,
        state: &mut ExecutionLoopState,
        used: usize,
        fixed_prompt_tokens: usize,
        session_id: Option<&str>,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> bool {
        if !self.config.auto_compact {
            return false;
        }

        let max = self.config.max_context_tokens;
        let threshold = self.config.auto_compact_threshold;

        if !crate::compaction::should_auto_compact(used, max, threshold) {
            return false;
        }

        let before_len = state.messages.len();
        let percent_before = used as f32 / max as f32;
        let compaction_budget = crate::compaction::CompactionBudget::for_auto_compaction(
            max,
            threshold,
            fixed_prompt_tokens,
        );

        tracing::info!(
            used_tokens = used,
            max_tokens = max,
            percent = percent_before,
            threshold = threshold,
            target_tokens = compaction_budget.target_context_tokens,
            fixed_prompt_tokens,
            message_token_limit = compaction_budget.message_token_limit,
            "Auto-compact triggered"
        );

        let mut changed = false;
        if let Some(pruned) = crate::compaction::prune_tool_outputs(
            &state.messages,
            compaction_budget.target_context_tokens,
        ) {
            state.messages = pruned;
            changed = true;
            tracing::info!("Tool output pruning applied");
        }

        let timeout_ms = self
            .config
            .llm_api_timeout_ms
            .unwrap_or(DEFAULT_AUTO_COMPACT_TIMEOUT_MS)
            .max(1);
        let compaction_client =
            self.scoped_llm_client_for_parts(session_id, event_tx, cancel_token);
        let compact_result = tokio::select! {
            _ = cancel_token.cancelled() => {
                tracing::warn!("Auto-compact cancelled before summary generation completed");
                None
            }
            result = tokio::time::timeout(
                Duration::from_millis(timeout_ms),
                crate::compaction::compact_messages(
                    session_id.unwrap_or(""),
                    &state.messages,
                    &compaction_client,
                    compaction_budget,
                ),
            ) => {
                match result {
                    Ok(Ok(compacted)) => compacted,
                    Ok(Err(error)) => {
                        tracing::warn!(error = %error, "Auto-compact summary generation failed");
                        None
                    }
                    Err(_) => {
                        tracing::warn!(
                            timeout_ms,
                            "Auto-compact summary generation timed out; keeping current context"
                        );
                        None
                    }
                }
            }
        };

        let mut compact_summary = None;
        if let Some(compacted) = compact_result {
            state.messages = compacted.messages;
            compact_summary = Some(compacted.summary);
            changed = true;
            let estimated_tokens_after = fixed_prompt_tokens
                .saturating_add(crate::llm::estimate_message_tokens(&state.messages));
            tracing::info!(
                estimated_tokens_after,
                target_tokens = compaction_budget.target_context_tokens,
                retained_messages = state.messages.len(),
                "Auto-compact summary applied"
            );
        }

        if !changed {
            return true;
        }

        self.config.rl_trajectory_recorder.record_context_compacted(
            session_id.unwrap_or(""),
            before_len,
            &state.messages,
            percent_before,
        );

        if let Some(tx) = event_tx {
            tx.send(AgentEvent::ContextCompacted {
                session_id: session_id.unwrap_or("").to_string(),
                before_messages: before_len,
                after_messages: state.messages.len(),
                percent_before,
                summary: compact_summary,
            })
            .await
            .ok();
        }
        true
    }

    pub(super) async fn emit_error(
        &self,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        message: String,
    ) {
        if let Some(tx) = event_tx {
            tx.send(AgentEvent::Error { message }).await.ok();
        }
    }

    /// Call the LLM, handling streaming vs non-streaming internally.
    ///
    /// Streaming events (`TextDelta`, `ToolStart`) are forwarded to `event_tx`
    /// as they arrive. Non-streaming mode simply awaits the complete response.
    ///
    /// Tool definitions are selected once per turn by the centralized tool selector.
    ///
    /// Returns `Err` on any LLM API failure. The circuit breaker in
    /// `execute_loop` wraps this call with retry logic for non-streaming mode.
    async fn call_llm(
        &self,
        llm_client: &std::sync::Arc<dyn crate::llm::LlmClient>,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<LlmResponse> {
        if event_tx.is_some() {
            let mut stream_rx = match self
                .scoped_streaming_completion(llm_client, messages, system, tools, cancel_token)
                .await
            {
                Ok(rx) => rx,
                Err(stream_error) => {
                    // Do not fall back to non-streaming if cancelled — propagate cancellation
                    if cancel_token.is_cancelled() {
                        anyhow::bail!("Operation cancelled by user");
                    }
                    // A provider can mark errors that require external state to
                    // change (for example, an account quota reset). Repeating the
                    // same request through the fallback cannot make them succeed.
                    if is_budget_exhausted(&stream_error)
                        || non_retryable_llm_error_message(&stream_error).is_some()
                    {
                        return Err(stream_error);
                    }
                    tracing::warn!(
                        error = %stream_error,
                        "LLM streaming setup failed; falling back to non-streaming completion"
                    );
                    return self
                        .call_non_streaming_llm(
                            llm_client,
                            messages,
                            system,
                            tools,
                            cancel_token,
                        )
                        .await
                        .with_context(|| {
                            format!(
                                "LLM streaming call failed ({stream_error}); non-streaming fallback also failed"
                            )
                        });
                }
            };

            let mut final_response: Option<LlmResponse> = None;
            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        tracing::info!("🛑 LLM streaming cancelled by CancellationToken");
                        anyhow::bail!("Operation cancelled by user");
                    }
                    event = stream_rx.recv() => {
                        match event {
                            Some(crate::llm::StreamEvent::TextDelta(text)) => {
                                if let Some(tx) = event_tx {
                                    tx.send(AgentEvent::TextDelta { text }).await.ok();
                                }
                            }
                            Some(crate::llm::StreamEvent::ReasoningDelta(text)) => {
                                if let Some(tx) = event_tx {
                                    tx.send(AgentEvent::ReasoningDelta { text }).await.ok();
                                }
                            }
                            Some(crate::llm::StreamEvent::ToolUseStart { id, name }) => {
                                if let Some(tx) = event_tx {
                                    tx.send(AgentEvent::ToolStart { id, name }).await.ok();
                                }
                            }
                            Some(crate::llm::StreamEvent::ToolUseInputDelta { id, delta }) => {
                                if let Some(tx) = event_tx {
                                    tx.send(AgentEvent::ToolInputDelta { id, delta }).await.ok();
                                }
                            }
                            Some(crate::llm::StreamEvent::Done(resp)) => {
                                final_response = Some(resp);
                                break;
                            }
                            None => break,
                        }
                    }
                }
            }
            final_response.ok_or_else(|| anyhow::Error::new(IncompleteLlmStream))
        } else {
            self.call_non_streaming_llm(llm_client, messages, system, tools, cancel_token)
                .await
                .context("LLM call failed")
        }
    }

    async fn call_non_streaming_llm(
        &self,
        llm_client: &std::sync::Arc<dyn crate::llm::LlmClient>,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<LlmResponse> {
        tokio::select! {
            biased;
            _ = cancel_token.cancelled() => anyhow::bail!("Operation cancelled by user"),
            response = llm_client.complete(messages, system, tools) => response,
        }
    }

    async fn scoped_streaming_completion(
        &self,
        llm_client: &std::sync::Arc<dyn crate::llm::LlmClient>,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
        llm_client
            .complete_streaming(messages, system, tools, cancel_token.clone())
            .await
    }

    /// Fire GenerateStart hook event before an LLM call.
    async fn fire_generate_start(
        &self,
        session_id: &str,
        prompt: &str,
        system_prompt: &Option<String>,
    ) {
        if let Some(he) = &self.config.hook_engine {
            let event = HookEvent::GenerateStart(GenerateStartEvent {
                session_id: session_id.to_string(),
                prompt: prompt.to_string(),
                system_prompt: system_prompt.clone(),
                model_provider: String::new(),
                model_name: String::new(),
                available_tools: self.config.tools.iter().map(|t| t.name.clone()).collect(),
            });
            let _ = he.fire(&event).await;
        }
    }

    /// Fire GenerateEnd hook event after an LLM call.
    async fn fire_generate_end(
        &self,
        session_id: &str,
        prompt: &str,
        response: &LlmResponse,
        duration_ms: u64,
    ) {
        if let Some(he) = &self.config.hook_engine {
            let tool_calls: Vec<ToolCallInfo> = response
                .tool_calls()
                .iter()
                .map(|tc| {
                    let args = if tc.args.is_null() {
                        serde_json::Value::Object(Default::default())
                    } else {
                        tc.args.clone()
                    };
                    ToolCallInfo {
                        name: tc.name.clone(),
                        args,
                    }
                })
                .collect();

            let event = HookEvent::GenerateEnd(GenerateEndEvent {
                session_id: session_id.to_string(),
                prompt: prompt.to_string(),
                response_text: response.text().to_string(),
                tool_calls,
                usage: TokenUsageInfo {
                    prompt_tokens: response.usage.prompt_tokens as i32,
                    completion_tokens: response.usage.completion_tokens as i32,
                    total_tokens: response.usage.total_tokens as i32,
                },
                duration_ms,
            });
            let _ = he.fire(&event).await;
        }
    }
}

#[cfg(test)]
mod stream_retry_tests {
    use super::*;

    #[test]
    fn stream_retry_delay_is_exponential_and_capped() {
        let first = stream_interruption_retry_delay(0).as_millis();
        let second = stream_interruption_retry_delay(1).as_millis();
        let third = stream_interruption_retry_delay(2).as_millis();
        let capped = stream_interruption_retry_delay(9).as_millis();

        assert!((750..=1_250).contains(&first));
        assert!((1_500..=2_500).contains(&second));
        assert!((3_000..=5_000).contains(&third));
        assert!((22_500..=37_500).contains(&capped));
    }
}
