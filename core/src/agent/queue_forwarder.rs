use super::AgentEvent;
use crate::session_lane_queue::SessionLaneQueue;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub(super) struct QueueEventForwarder {
    handle: Option<JoinHandle<()>>,
}

impl QueueEventForwarder {
    pub(super) fn start(
        queue: Option<&Arc<SessionLaneQueue>>,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> Self {
        let (Some(queue), Some(tx)) = (queue, event_tx) else {
            return Self { handle: None };
        };

        let mut rx = queue.subscribe();
        let tx = tx.clone();
        let cancel = cancel_token.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    event = rx.recv() => {
                        match event {
                            Ok(e) => {
                                if tx.send(e).await.is_err() {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    _ = cancel.cancelled() => {
                        break;
                    }
                }
            }
        });

        Self {
            handle: Some(handle),
        }
    }
}

impl Drop for QueueEventForwarder {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}
