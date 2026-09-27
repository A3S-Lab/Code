use agent_client_protocol as acp;
use tokio::sync::{mpsc, oneshot};

use crate::extensions::notification::SessionNotification as A3sSessionNotification;
use acp::SessionNotification as AcpSessionNotification;

/// The buffer debounces and merges everything, then `emit_buffered` emits it without firing per-chunk hooks or persistence writes.
/// The two variants stay separate because ACP and A3S chunks merge under different rules and wire envelopes.
/// One-shot A3S events (RetryState, ImageCompressed, HookExecution, etc.) instead take `send_a3s_notification` for per-event hooks and persistence.
#[derive(Debug, Clone)]
pub(crate) enum SessionNotification {
    Acp(Box<AcpSessionNotification>),
    A3s(Box<A3sSessionNotification>),
}

impl SessionNotification {
    pub(crate) fn session_id(&self) -> &acp::SessionId {
        match self {
            Self::Acp(n) => &n.session_id,
            Self::A3s(n) => &n.session_id,
        }
    }

    /// Returns true if this notification is a streaming chunk that should be buffered for merging and debouncing.
    pub(crate) fn is_streaming_chunk(&self) -> bool {
        match self {
            Self::Acp(n) => matches!(
                n.update,
                acp::SessionUpdate::AgentMessageChunk(_) | acp::SessionUpdate::AgentThoughtChunk(_)
            ),
            Self::A3s(n) => matches!(
                n.update,
                crate::extensions::notification::SessionUpdate::ToolCallDeltaChunk { .. }
            ),
        }
    }

    pub(crate) fn agent_timestamp_ms(&self) -> Option<u64> {
        match self {
            Self::Acp(n) => n
                .meta
                .as_ref()
                .and_then(|m| m.get("agentTimestampMs"))
                .and_then(|v| v.as_u64()),
            Self::A3s(n) => n
                .meta
                .as_ref()
                .and_then(|m| m.get("agentTimestampMs"))
                .and_then(|v| v.as_u64()),
        }
    }

    /// Returns true if this notification can be merged with `prev`'s pending slot based on their timestamps.
    pub(crate) fn is_in_timestamp_window(&self, prev: &Self, max_duration_ms: u64) -> bool {
        match (prev.agent_timestamp_ms(), self.agent_timestamp_ms()) {
            // ACP events have timestamps, so we can window-check.
            (Some(prev_ts), Some(incoming_ts)) => incoming_ts <= prev_ts + max_duration_ms,
            // Either side missing the agentTimestampMs meta means the time window cannot be checked
            _ => true,
        }
    }
}

impl From<AcpSessionNotification> for SessionNotification {
    fn from(n: AcpSessionNotification) -> Self {
        Self::Acp(Box::new(n))
    }
}

impl From<A3sSessionNotification> for SessionNotification {
    fn from(n: A3sSessionNotification) -> Self {
        Self::A3s(Box::new(n))
    }
}

#[cfg(test)]
impl SessionNotification {
    pub(crate) fn expect_acp(&self) -> &AcpSessionNotification {
        match self {
            Self::Acp(n) => n,
            Self::A3s(_) => panic!("expected Acp notification, got A3s"),
        }
    }

    pub(crate) fn into_acp(self) -> AcpSessionNotification {
        match self {
            Self::Acp(n) => *n,
            Self::A3s(_) => panic!("expected Acp notification, got A3s"),
        }
    }
}

#[derive(Debug)]
pub(crate) enum SessionEvent {
    Notification(SessionNotification),
    FlushReplay {
        respond_to: Option<oneshot::Sender<()>>,
    },
}

impl SessionEvent {
    pub(crate) fn flush_with_ack() -> (Self, oneshot::Receiver<()>) {
        let (tx, rx) = oneshot::channel();
        (
            Self::FlushReplay {
                respond_to: Some(tx),
            },
            rx,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FlushReplayError {
    EventChannelClosed,
    Timeout,
}

/// Flush replay-buffered notifications through the session actor loop.
/// The `FlushComplete` command runs inside the actor loop, so it flushes `replay_buffer` inline instead.
pub(crate) async fn flush_replay_actor(
    event_tx: &mpsc::UnboundedSender<SessionEvent>,
) -> Result<(), FlushReplayError> {
    let (event, rx) = SessionEvent::flush_with_ack();
    event_tx
        .send(event)
        .map_err(|_| FlushReplayError::EventChannelClosed)?;
    tokio::time::timeout(std::time::Duration::from_secs(5), rx)
        .await
        .map_err(|_| FlushReplayError::Timeout)?
        .map_err(|_| FlushReplayError::EventChannelClosed)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn flush_replay_actor_acknowledges() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let (event_tx, mut event_rx) = mpsc::unbounded_channel::<SessionEvent>();
                let (event, rx) = SessionEvent::flush_with_ack();
                event_tx.send(event).expect("event send should succeed");

                let event = event_rx.recv().await.expect("event should arrive");

                match event {
                    SessionEvent::FlushReplay { respond_to } => {
                        let tx = respond_to.expect("flush replay should carry ack sender");
                        tx.send(()).expect("ack send should succeed");
                    }
                    other => panic!("unexpected event: {other:?}"),
                }

                rx.await.expect("ack should be received");
            })
            .await;
    }
}
