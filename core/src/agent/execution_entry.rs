#[cfg(test)]
use super::AgentEvent;
use super::{AgentLoop, AgentResult, InvocationContext};
use crate::llm::Message;
use anyhow::Result;
#[cfg(test)]
use tokio::sync::mpsc;

impl AgentLoop {
    /// Execute the agent loop for a prompt
    ///
    /// Takes the conversation history and a new user prompt.
    /// Returns the agent result and updated message history.
    /// When event_tx is provided, uses streaming LLM API for real-time text output.
    #[cfg(test)]
    pub async fn execute(
        &self,
        history: &[Message],
        prompt: &str,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentResult> {
        self.execute_with_session(history, prompt, None, event_tx, None)
            .await
    }

    /// Execute a run whose user message is already present in `messages`.
    /// Resume callers may seed the cumulative accounting state.
    pub(crate) async fn execute_from_messages_with_invocation_seeded(
        &self,
        messages: Vec<Message>,
        invocation: &InvocationContext,
        seed: Option<super::execution_state::ExecutionSeed>,
    ) -> Result<AgentResult> {
        let agent = invocation.bind_agent_loop(self);
        let session_id = invocation.session_id_option();
        let event_tx = invocation.event_tx().clone();
        let token = invocation.cancellation();
        tracing::info!(
            a3s.run.id = invocation.run_id(),
            a3s.session.id = session_id.unwrap_or("none"),
            a3s.agent.max_turns = agent.config.max_tool_rounds,
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

        let result = agent
            .execute_loop_inner(
                &messages,
                "",
                &effective_prompt,
                None, // no pre-computed style; resolve inside the loop
                session_id,
                event_tx,
                token,
                true, // emit_end: this is a standalone execution
                seed,
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
