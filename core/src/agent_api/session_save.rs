//! Host-facing session save control.
//!
//! Persistence snapshots are owned by `session_persistence`; this module keeps
//! the public `save()` entrypoint thin without exposing snapshot construction to
//! the facade.

use super::{session_persistence::SessionPersistenceContext, AgentSession};
use crate::error::Result;

pub(super) async fn save(session: &AgentSession) -> Result<()> {
    SessionPersistenceContext::from_session(session)
        .save()
        .await
}
