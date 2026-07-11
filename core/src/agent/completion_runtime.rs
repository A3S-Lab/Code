use super::execution_state::ExecutionLoopState;
use super::{AgentEvent, AgentLoop};
use crate::llm::{LlmResponse, Message};
use crate::prompts::CONTINUATION;
use crate::verification::VerificationSummary;
use futures::future::join_all;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const REASONING_ONLY_REPAIR: &str = "\
Your previous assistant message contained reasoning/thinking content but no \
normal reply content and no tool call. Continue from that state now. If tool \
work is still required, call the needed tool. Otherwise provide the final answer \
in normal assistant content only; do not put the final answer in reasoning.";

const REASONING_ONLY_FALLBACK: &str = "\
The model completed but returned only reasoning content and did not provide a \
final answer.";

pub(super) enum CompletionFlow {
    Continue,
    Finished(String),
}

impl AgentLoop {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn complete_no_tool_response(
        &self,
        state: &mut ExecutionLoopState,
        turn: usize,
        response: &LlmResponse,
        effective_prompt: &str,
        session_id: Option<&str>,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        emit_end: bool,
        cancel_token: &CancellationToken,
    ) -> CompletionFlow {
        let candidate_text = response.text();

        if self.inject_reasoning_only_repair_if_needed(state, turn, response) {
            return CompletionFlow::Continue;
        }

        let candidate_text = if candidate_text.trim().is_empty()
            && Self::is_terminal_reasoning_only_response(response)
        {
            REASONING_ONLY_FALLBACK.to_string()
        } else {
            candidate_text
        };

        if self.inject_continuation_if_needed(state, turn, &candidate_text) {
            return CompletionFlow::Continue;
        }

        let final_text = self.sanitize_final_text(&candidate_text);
        self.log_execution_completed(state, turn);
        self.emit_end_if_requested(state, response, &final_text, event_tx, emit_end)
            .await;

        if let Some(sid) = session_id {
            self.schedule_turn_memory_extraction(
                state,
                effective_prompt,
                &final_text,
                sid,
                event_tx,
                cancel_token,
            )
            .await;
            self.notify_turn_complete(sid, effective_prompt, &final_text)
                .await;
        }

        CompletionFlow::Finished(final_text)
    }

    fn inject_reasoning_only_repair_if_needed(
        &self,
        state: &mut ExecutionLoopState,
        turn: usize,
        response: &LlmResponse,
    ) -> bool {
        if !Self::is_terminal_reasoning_only_response(response) {
            return false;
        }

        if !state.should_inject_reasoning_only_repair(
            self.config.continuation_enabled,
            self.config.max_tool_rounds,
        ) {
            return false;
        }

        tracing::info!(
            turn = turn,
            "Injecting reasoning-only repair message - response had no content"
        );
        state.messages.push(Message::user(REASONING_ONLY_REPAIR));
        true
    }

    fn is_terminal_reasoning_only_response(response: &LlmResponse) -> bool {
        if !response.text().trim().is_empty() {
            return false;
        }
        if response
            .message
            .reasoning_content
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            return false;
        }

        let reason = response
            .stop_reason
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        !(reason.contains("length")
            || reason.contains("max_tokens")
            || reason.contains("tool")
            || reason.contains("filter"))
    }

    fn inject_continuation_if_needed(
        &self,
        state: &mut ExecutionLoopState,
        turn: usize,
        candidate_text: &str,
    ) -> bool {
        if !state.should_inject_continuation(
            Self::looks_incomplete(candidate_text),
            self.config.continuation_enabled,
            self.config.max_continuation_turns,
            self.config.max_tool_rounds,
        ) {
            return false;
        }

        tracing::info!(
            turn = turn,
            continuation = state.continuation_count(),
            max_continuation = self.config.max_continuation_turns,
            "Injecting continuation message - response looks incomplete"
        );
        state.messages.push(Message::user(CONTINUATION));
        true
    }

    fn sanitize_final_text(&self, text: &str) -> String {
        if let Some(ref sp) = self.config.security_provider {
            sp.sanitize_output(text)
        } else {
            text.to_string()
        }
    }

    fn log_execution_completed(&self, state: &ExecutionLoopState, turn: usize) {
        tracing::info!(
            tool_calls_count = state.tool_calls_count,
            total_prompt_tokens = state.total_usage.prompt_tokens,
            total_completion_tokens = state.total_usage.completion_tokens,
            total_tokens = state.total_usage.total_tokens,
            turns = turn,
            "Agent execution completed"
        );
    }

    async fn emit_end_if_requested(
        &self,
        state: &ExecutionLoopState,
        response: &LlmResponse,
        final_text: &str,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        emit_end: bool,
    ) {
        if !emit_end {
            return;
        }

        if let Some(tx) = event_tx {
            let verification_summary =
                VerificationSummary::from_reports(&state.verification_reports);
            tx.send(AgentEvent::End {
                text: final_text.to_string(),
                usage: state.total_usage.clone(),
                verification_summary: Box::new(verification_summary),
                meta: response.meta.clone(),
            })
            .await
            .ok();
        }
    }

    /// Notify providers of turn completion for memory extraction.
    async fn notify_turn_complete(&self, session_id: &str, prompt: &str, response: &str) {
        let futures = self
            .config
            .context_providers
            .iter()
            .map(|p| p.on_turn_complete(session_id, prompt, response));
        let outcomes = join_all(futures).await;

        for (i, result) in outcomes.into_iter().enumerate() {
            if let Err(e) = result {
                tracing::warn!(
                    "Context provider '{}' on_turn_complete failed: {}",
                    self.config.context_providers[i].name(),
                    e
                );
            }
        }
    }

    /// Detect whether the LLM's no-tool-call response looks like an intermediate
    /// step rather than a genuine final answer.
    ///
    /// Returns `true` when continuation should be injected. Heuristics:
    /// - Response ends with a colon or ellipsis (mid-thought)
    /// - Response contains phrases that signal incomplete work
    /// - Response is very short (< 80 chars) and doesn't look like a summary
    pub(super) fn looks_incomplete(text: &str) -> bool {
        let t = text.trim();
        if t.is_empty() {
            return true;
        }

        if t.len() < 80 && !t.contains('\n') {
            let ends_continuation =
                t.ends_with(':') || t.ends_with("...") || t.ends_with('…') || t.ends_with(',');
            if ends_continuation {
                return true;
            }
        }

        let lower = t.to_lowercase();
        [
            "i'll ",
            "i will ",
            "let me ",
            "i need to ",
            "i should ",
            "next, i",
            "first, i",
            "now i",
            "i'll start",
            "i'll begin",
            "i'll now",
            "let's start",
            "let's begin",
            "to do this",
            "i'm going to",
        ]
        .iter()
        .any(|phrase| lower.contains(phrase))
    }
}
