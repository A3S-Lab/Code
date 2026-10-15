#![cfg(test)]

use super::*;
use std::time::Duration;

fn progress(value: f64) -> McpNotification {
    McpNotification::Progress {
        progress_token: "original".into(),
        progress: value,
        total: Some(2.0),
    }
}

fn value(notification: McpNotification) -> f64 {
    match notification {
        McpNotification::Progress { progress, .. } => progress,
        other => panic!("expected actual progress, got {other:?}"),
    }
}

#[tokio::test]
async fn single_public_claim_retains_the_buffered_fifo() {
    let (sender, receiver) = NotificationReceiver::channel(2);
    sender.send(progress(1.0)).await;
    sender.send(progress(2.0)).await;
    let mut public = receiver.take();
    assert_eq!(value(public.recv().await.unwrap()), 1.0);
    assert_eq!(value(public.recv().await.unwrap()), 2.0);
    assert!(receiver.take().recv().await.is_none());
}

#[tokio::test]
async fn cache_signal_and_public_frame_both_observe_the_same_change() {
    let (sender, receiver) = NotificationReceiver::channel(2);
    let mut public = receiver.take();
    let mut changes = receiver.tool_list_changes();
    sender.send(McpNotification::ToolsListChanged).await;
    changes.changed().await.unwrap();
    assert!(matches!(
        public.recv().await,
        Some(McpNotification::ToolsListChanged)
    ));
    sender.send(progress(1.0)).await;
    assert_eq!(value(public.recv().await.unwrap()), 1.0);
}

#[tokio::test]
async fn a_change_before_cache_subscription_is_retained() {
    let (sender, receiver) = NotificationReceiver::channel(1);
    sender.send(McpNotification::ToolsListChanged).await;
    let mut changes = receiver.tool_list_changes();
    assert!(changes.has_changed().unwrap());
    changes.changed().await.unwrap();
    assert!(!changes.has_changed().unwrap());
}

#[tokio::test]
async fn unclaimed_full_public_queue_does_not_block_cache_or_transport() {
    let (sender, receiver) = NotificationReceiver::channel(1);
    let mut changes = receiver.tool_list_changes();
    tokio::time::timeout(Duration::from_secs(1), async {
        sender.send(progress(1.0)).await;
        sender.send(progress(2.0)).await;
        sender.send(McpNotification::ToolsListChanged).await;
    })
    .await
    .unwrap();
    changes.changed().await.unwrap();
    assert_eq!(value(receiver.take().recv().await.unwrap()), 1.0);
}

#[tokio::test]
async fn claimed_public_queue_retains_backpressure_and_the_original_pending_send() {
    let (sender, receiver) = NotificationReceiver::channel(1);
    let mut public = receiver.take();
    sender.send(progress(1.0)).await;
    let pending = sender.send(progress(2.0));
    tokio::pin!(pending);
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut pending)
            .await
            .is_err()
    );
    assert_eq!(value(public.recv().await.unwrap()), 1.0);
    pending.await;
    assert_eq!(value(public.recv().await.unwrap()), 2.0);
}

#[tokio::test]
async fn a_dropped_public_observer_does_not_stop_cache_changes() {
    let (sender, receiver) = NotificationReceiver::channel(1);
    drop(receiver.take());
    let mut changes = receiver.tool_list_changes();
    sender.send(McpNotification::ToolsListChanged).await;
    changes.changed().await.unwrap();
}

#[tokio::test]
async fn actual_sender_drop_closes_both_notification_surfaces() {
    let (sender, receiver) = NotificationReceiver::channel(1);
    let mut public = receiver.take();
    let mut changes = receiver.tool_list_changes();
    drop(sender);
    assert!(public.recv().await.is_none());
    assert!(changes.changed().await.is_err());
}
