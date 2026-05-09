use super::{AgentEvent, AgentLoop, AgentResult};
use crate::llm::Message;
use anyhow::Result;
use tokio::sync::mpsc;

impl AgentLoop {
    /// Execute the agent loop for a prompt
    ///
    /// Takes the conversation history and a new user prompt.
    /// Returns the agent result and updated message history.
    /// When event_tx is provided, uses streaming LLM API for real-time text output.
    pub async fn execute(
        &self,
        history: &[Message],
        prompt: &str,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentResult> {
        self.execute_with_session(history, prompt, None, event_tx, None)
            .await
    }

    /// Execute the agent loop with pre-built messages (user message already included).
    ///
    /// Used by `send_with_attachments` / `stream_with_attachments` where the
    /// user message contains multi-modal content and is already appended to
    /// the messages vec.
    pub async fn execute_from_messages(
        &self,
        messages: Vec<Message>,
        session_id: Option<&str>,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
        cancel_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<AgentResult> {
        let default_token = tokio_util::sync::CancellationToken::new();
        let token = cancel_token.unwrap_or(&default_token);
        tracing::info!(
            a3s.session.id = session_id.unwrap_or("none"),
            a3s.agent.max_turns = self.config.max_tool_rounds,
            "a3s.agent.execute_from_messages started"
        );

        // Extract the last user message text for hooks, memory recall, and events.
        // Pass empty prompt so execute_loop skips adding a duplicate user message,
        // but provide effective_prompt for hook/memory/event purposes.
        let effective_prompt = messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.text())
            .unwrap_or_default();

        let result = self
            .execute_loop_inner(
                &messages,
                "",
                &effective_prompt,
                None, // no pre-computed style; resolve inside the loop
                session_id,
                event_tx,
                token,
                true, // emit_end: this is a standalone execution
            )
            .await;

        match &result {
            Ok(r) => tracing::info!(
                a3s.agent.tool_calls_count = r.tool_calls_count,
                a3s.llm.total_tokens = r.usage.total_tokens,
                "a3s.agent.execute_from_messages completed"
            ),
            Err(e) => tracing::warn!(
                error = %e,
                "a3s.agent.execute_from_messages failed"
            ),
        }

        result
    }
}
