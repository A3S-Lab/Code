//! Host-facing hook control.
//!
//! Execution uses a hook executor seam. This module owns the public mutation
//! surface for the in-process hook engine.

use super::AgentSession;
use std::sync::Arc;

pub(super) struct HookControl<'a> {
    session: &'a AgentSession,
}

impl<'a> HookControl<'a> {
    pub(super) fn from_session(session: &'a AgentSession) -> Self {
        Self { session }
    }

    pub(super) fn register_hook(&self, hook: crate::hooks::Hook) {
        self.session.hook_engine.register(hook);
    }

    pub(super) fn unregister_hook(&self, hook_id: &str) -> Option<crate::hooks::Hook> {
        self.session.hook_engine.unregister(hook_id)
    }

    pub(super) fn register_hook_handler(
        &self,
        hook_id: &str,
        handler: Arc<dyn crate::hooks::HookHandler>,
    ) {
        self.session.hook_engine.register_handler(hook_id, handler);
    }

    pub(super) fn unregister_hook_handler(&self, hook_id: &str) {
        self.session.hook_engine.unregister_handler(hook_id);
    }

    pub(super) fn hook_count(&self) -> usize {
        self.session.hook_engine.hook_count()
    }
}
