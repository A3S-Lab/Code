//! One public FIFO and an independent, coalesced tools-list change signal.
//!
//! Before a host claims the FIFO, retain its bounded prefix without blocking
//! transport replies. Once claimed, preserve normal bounded backpressure.
//! Cache refresh never takes the host's notification receiver.

use crate::mcp::protocol::McpNotification;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, watch, Mutex};

pub(super) struct NotificationSender {
    sender: mpsc::Sender<McpNotification>,
    claimed: Arc<AtomicBool>,
    preclaim_overflow_reported: AtomicBool,
    tool_list_changes: watch::Sender<()>,
}

pub(super) struct NotificationReceiver {
    receiver: Mutex<Option<mpsc::Receiver<McpNotification>>>,
    claimed: Arc<AtomicBool>,
    tool_list_changes: watch::Receiver<()>,
}

impl NotificationReceiver {
    pub(super) fn channel(capacity: usize) -> (NotificationSender, Self) {
        let (sender, receiver) = mpsc::channel(capacity);
        let (changed_tx, changed_rx) = watch::channel(());
        let claimed = Arc::new(AtomicBool::new(false));
        (
            NotificationSender {
                sender,
                claimed: Arc::clone(&claimed),
                preclaim_overflow_reported: AtomicBool::new(false),
                tool_list_changes: changed_tx,
            },
            Self {
                receiver: Mutex::new(Some(receiver)),
                claimed,
                tool_list_changes: changed_rx,
            },
        )
    }

    pub(super) fn take(&self) -> mpsc::Receiver<McpNotification> {
        if let Ok(mut slot) = self.receiver.try_lock() {
            if let Some(receiver) = slot.take() {
                self.claimed.store(true, Ordering::Release);
                return receiver;
            }
        }
        let (_, receiver) = mpsc::channel(1);
        receiver
    }

    pub(super) fn tool_list_changes(&self) -> watch::Receiver<()> {
        self.tool_list_changes.clone()
    }
}

impl NotificationSender {
    pub(super) async fn send(&self, notification: McpNotification) {
        if matches!(notification, McpNotification::ToolsListChanged) {
            self.tool_list_changes.send_replace(());
        }
        if self.claimed.load(Ordering::Acquire) {
            // A dropped host observer must not stop the independent cache
            // signal or the transport reader. Active observers retain FIFO
            // backpressure; transport shutdown cancels this bounded wait.
            let _ = self.sender.send(notification).await;
        } else if matches!(
            self.sender.try_send(notification),
            Err(mpsc::error::TrySendError::Full(_))
        ) && !self
            .preclaim_overflow_reported
            .swap(true, Ordering::Relaxed)
        {
            tracing::warn!(
                "MCP notifications exceeded the bounded buffer before a host observer subscribed; newest notifications are discarded"
            );
        }
    }
}

#[cfg(test)]
mod tests;
