//! Host-facing session save control.
//!
//! Persistence snapshots are owned by `session_persistence`; this module keeps
//! the public `save()` entrypoint thin without exposing snapshot construction to
//! the facade.

use super::{
    execution_coordinator::ExecutionCoordinator, session_persistence::SessionPersistenceContext,
    AgentSession,
};
use crate::error::Result;

pub(super) async fn save(session: &AgentSession) -> Result<()> {
    // A manual save must not sample history, usage, tasks, and artifacts from
    // different points in an active conversation. Auto-save runs inside the
    // admitted run lifecycle and calls `SessionPersistenceContext` directly.
    let _lease = ExecutionCoordinator::admit(session, "save").await?;
    SessionPersistenceContext::from_session(session)
        .save()
        .await
}
