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
use a3s_effect::LogStore;

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
        let log = self
            .run_admission
            .fact_log_slot()
            .lock()
            .map_err(|_| CodeError::Session("compaction original log is poisoned".into()))?
            .clone()
            .ok_or_else(|| CodeError::Session("compaction original log is absent".into()))?;
        let writer = log
            .try_writer_fence()
            .map_err(|error| CodeError::Session(error.to_string()))?;
        let facts = log
            .read(&crate::fact_control::thread_for_session(&self.session_id))
            .map_err(|error| CodeError::Session(error.to_string()))?;
        let persistence = SessionPersistenceContext::from_session(self);
        let source = persistence.source_from_facts(&facts)?;
        let from_log = crate::fact_control::compaction_transcript_from_facts(&facts);
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
        let cancellation = self.session_cancel.child_token();
        let client = super::agent_loop_runtime::build_agent_loop(self).scoped_llm_client_for_parts(
            Some(&self.session_id),
            &None,
            &cancellation,
        );
        let compacted = crate::compaction::compact_messages(
            &self.session_id,
            &messages,
            &client,
            budget,
            focus,
            source.as_ref(),
        )
        .await
        .map_err(|error| CodeError::Session(format!("{error:#}")))?;
        let Some(compacted) = compacted else {
            return Err(CodeError::Session("Nothing to compact yet.".into()));
        };
        self.close_handle.mutate_immediate(|| -> Result<()> {
            crate::fact_control::append_compaction_done(
                &log,
                &writer,
                &self.session_id,
                &compacted.summary,
                source.as_ref(),
            )
            .map_err(|error| CodeError::Session(error.to_string()))?;
            *write_or_recover(&self.history) = vec![Message::user_wire(&format!(
                "{}{}",
                crate::prompts::CONTEXT_SUMMARY_PREFIX,
                compacted.summary
            ))];
            Ok(())
        })??;
        persistence.auto_save_if_enabled().await;
        Ok(())
    }
}
