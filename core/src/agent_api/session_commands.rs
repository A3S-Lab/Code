//! Host-facing slash command registry control.
//!
//! Slash commands are part of the session control plane: hosts can inspect and
//! extend the registry, while conversation runtime dispatches commands before
//! calling the LLM. This module keeps registry locking and dispatch locality in
//! one place.

use super::AgentSession;
use crate::commands::{CommandContext, CommandOutput, CommandRegistry, SlashCommand};
use std::sync::{Arc, MutexGuard};

pub(super) fn registry(session: &AgentSession) -> MutexGuard<'_, CommandRegistry> {
    session
        .command_registry
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(super) fn register(session: &AgentSession, cmd: Arc<dyn SlashCommand>) {
    registry(session).register(cmd);
}

pub(super) fn dispatch(
    session: &AgentSession,
    prompt: &str,
    ctx: &CommandContext,
) -> Option<CommandOutput> {
    registry(session).dispatch(prompt, ctx)
}
