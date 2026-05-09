//! Read-only session view.
//!
//! Public callers need stable access to session identity, workspace, history,
//! memory, and traces. This module centralizes those snapshots so the facade
//! does not directly expose how session state is stored.

use super::AgentSession;
use crate::error::read_or_recover;
use crate::llm::Message;
use crate::trace::TraceEvent;
use std::path::Path;
use std::sync::Arc;

pub(super) struct SessionView<'a> {
    session: &'a AgentSession,
}

impl<'a> SessionView<'a> {
    pub(super) fn from_session(session: &'a AgentSession) -> Self {
        Self { session }
    }

    pub(super) fn history(&self) -> Vec<Message> {
        read_or_recover(&self.session.history).clone()
    }

    pub(super) fn memory(&self) -> Option<&'a Arc<crate::memory::AgentMemory>> {
        self.session.memory.as_ref()
    }

    pub(super) fn id(&self) -> &'a str {
        &self.session.session_id
    }

    pub(super) fn workspace(&self) -> &'a Path {
        &self.session.workspace
    }

    pub(super) fn init_warning(&self) -> Option<&'a str> {
        self.session.init_warning.as_deref()
    }

    pub(super) fn trace_events(&self) -> Vec<TraceEvent> {
        self.session.trace_sink.events()
    }

    pub(super) async fn active_tools(&self) -> Vec<crate::run::ActiveToolSnapshot> {
        super::runtime_events::active_tool_snapshots(&self.session.active_tools).await
    }
}
