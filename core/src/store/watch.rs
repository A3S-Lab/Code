//! Commit watch notifications for session stores (KRN-6 / STORE-WATCH1).
//!
//! Backends that advertise [`super::SessionStoreCapabilities::watch`] publish
//! one event after a complete snapshot generation becomes durable. Events
//! carry session identity and digest only — never session plaintext.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

pub const SESSION_STORE_COMMIT_EVENT_SCHEMA_V1: &str = "a3s.code.session-store-commit-event.v1";

/// Notification that one complete session generation was committed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionStoreCommitEventV1 {
    pub schema: String,
    pub session_id: String,
    pub snapshot_digest: String,
    pub committed_at_ms: u64,
}

impl SessionStoreCommitEventV1 {
    pub fn new(
        session_id: impl Into<String>,
        snapshot_digest: impl Into<String>,
        committed_at_ms: u64,
    ) -> Result<Self> {
        let event = Self {
            schema: SESSION_STORE_COMMIT_EVENT_SCHEMA_V1.to_owned(),
            session_id: session_id.into(),
            snapshot_digest: snapshot_digest.into(),
            committed_at_ms,
        };
        event.validate()?;
        Ok(event)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != SESSION_STORE_COMMIT_EVENT_SCHEMA_V1 {
            bail!("session store commit event schema is unsupported");
        }
        if self.session_id.trim().is_empty() {
            bail!("session store commit event session_id must be non-empty");
        }
        if self.snapshot_digest.trim().is_empty() {
            bail!("session store commit event snapshot_digest must be non-empty");
        }
        Ok(())
    }
}

/// Subscription to durable snapshot commit notifications.
pub struct SessionStoreCommitWatch {
    receiver: broadcast::Receiver<SessionStoreCommitEventV1>,
}

impl SessionStoreCommitWatch {
    pub(super) fn new(receiver: broadcast::Receiver<SessionStoreCommitEventV1>) -> Self {
        Self { receiver }
    }

    /// Wait for the next committed generation. Lagged receivers skip to the
    /// newest available event rather than replaying missed ones.
    pub async fn recv(&mut self) -> Result<SessionStoreCommitEventV1> {
        loop {
            match self.receiver.recv().await {
                Ok(event) => {
                    event.validate()?;
                    return Ok(event);
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    bail!("session store commit watch closed")
                }
            }
        }
    }
}

pub(super) fn commit_watch_channel() -> (
    broadcast::Sender<SessionStoreCommitEventV1>,
    broadcast::Receiver<SessionStoreCommitEventV1>,
) {
    broadcast::channel(64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_event_rejects_empty_identity() {
        assert!(SessionStoreCommitEventV1::new("", "sha256:aa", 1).is_err());
        assert!(SessionStoreCommitEventV1::new("s", "", 1).is_err());
        let ok = SessionStoreCommitEventV1::new("s", "sha256:aa", 1).unwrap();
        assert_eq!(ok.session_id, "s");
    }
}
