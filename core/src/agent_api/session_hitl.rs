//! Host-facing HITL control.
//!
//! Tool execution uses the lower-level confirmation runtime; this module owns
//! the public session control surface for listing, resolving, and cancelling
//! pending confirmations.

use super::AgentSession;
use crate::hitl::PendingConfirmationInfo;

pub(super) struct HitlControl<'a> {
    session: &'a AgentSession,
}

impl<'a> HitlControl<'a> {
    pub(super) fn from_session(session: &'a AgentSession) -> Self {
        Self { session }
    }

    pub(super) async fn pending_confirmations(&self) -> Vec<PendingConfirmationInfo> {
        match &self.session.config.confirmation_manager {
            Some(manager) => manager.pending_confirmations().await,
            None => Vec::new(),
        }
    }

    pub(super) async fn cancel_confirmations(&self) -> usize {
        match &self.session.config.confirmation_manager {
            Some(manager) => manager.cancel_all().await,
            None => 0,
        }
    }
}
