//! Host-requested conversation compaction.
//!
//! `/compact` summarizes the visible fact log with the session model and
//! appends `compaction.done`. The next user turn folds that summary in place
//! of the older transcript. This does not resume the actor, so compaction
//! itself is not a model turn.

use super::execution_coordinator::ExecutionCoordinator;
use super::session_persistence::SessionPersistenceContext;
use super::AgentSession;
use crate::compaction::CompactionBudget;
use crate::error::{write_or_recover, CodeError, Result};
use crate::llm::Message;

impl AgentSession {
    /// Resolved context window for this session, in tokens.
    pub fn context_window_tokens(&self) -> usize {
        self.config.max_context_tokens
    }

    /// Summarize the current conversation and keep that summary for later turns.
    ///
    /// `focus` is host instruction for the summary. It is not written into the
    /// transcript. A session with fewer than two visible messages returns an
    /// error and leaves the log unchanged.
    pub async fn compact_conversation(&self, focus: Option<&str>) -> Result<()> {
        let _lease = ExecutionCoordinator::admit(self, "compact").await?;
        let focus = focus.map(str::trim).filter(|focus| !focus.is_empty());
        let from_log =
            crate::fact_control::compaction_transcript(&self.workspace, &self.session_id)
                .map_err(|error| CodeError::Session(error.to_string()))?;
        let messages = if from_log.len() >= 2 {
            from_log
        } else if from_log.is_empty() {
            self.history()
        } else {
            from_log
        };
        if messages.len() < 2 {
            return Err(CodeError::Session("Nothing to compact yet.".into()));
        }
        let budget = CompactionBudget::for_auto_compaction(
            self.config.max_context_tokens,
            self.config.auto_compact_threshold,
            0,
        );
        let compacted = crate::compaction::compact_messages(
            &self.session_id,
            &messages,
            &self.llm_client,
            budget,
            focus,
        )
        .await
        .map_err(|error| CodeError::Session(error.to_string()))?;
        let Some(compacted) = compacted else {
            return Err(CodeError::Session("Nothing to compact yet.".into()));
        };
        crate::fact_control::append_compaction_done(
            &self.workspace,
            &self.session_id,
            &compacted.summary,
        )
        .map_err(|error| CodeError::Session(error.to_string()))?;
        *write_or_recover(&self.history) = vec![Message::user_wire(&format!(
            "{}{}",
            crate::prompts::CONTEXT_SUMMARY_PREFIX,
            compacted.summary
        ))];
        SessionPersistenceContext::from_session(self)
            .auto_save_if_enabled()
            .await;
        Ok(())
    }
}
