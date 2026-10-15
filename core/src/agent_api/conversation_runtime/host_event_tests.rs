#![cfg(test)]

use super::*;
use tokio_util::sync::CancellationToken;

fn text(value: &str) -> AgentEvent {
    AgentEvent::TextDelta { text: value.into() }
}

#[tokio::test]
async fn cancellation_releases_a_full_host_send_without_replacing_the_queued_prefix() {
    let (tx, mut rx) = mpsc::channel(1);
    tx.send(text("original prefix")).await.unwrap();
    let cancellation = CancellationToken::new();
    let mut send = Box::pin(send_host_event(
        &tx,
        text("cancelled tail"),
        Some(&cancellation),
    ));
    assert!(futures::poll!(&mut send).is_pending());
    cancellation.cancel();
    send.await;
    drop(tx);
    assert!(
        matches!(rx.recv().await, Some(AgentEvent::TextDelta { text }) if text == "original prefix")
    );
    assert!(rx.recv().await.is_none());
}

#[tokio::test]
async fn a_ready_send_remains_observable_after_cancellation() {
    let (tx, mut rx) = mpsc::channel(1);
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    send_host_event(&tx, text("available slot"), Some(&cancellation)).await;
    assert!(
        matches!(rx.recv().await, Some(AgentEvent::TextDelta { text }) if text == "available slot")
    );
}

#[tokio::test]
async fn uncancelled_host_delivery_keeps_backpressure_and_order() {
    let (tx, mut rx) = mpsc::channel(1);
    tx.send(text("first")).await.unwrap();
    let cancellation = CancellationToken::new();
    let mut send = Box::pin(send_host_event(&tx, text("second"), Some(&cancellation)));
    assert!(futures::poll!(&mut send).is_pending());
    assert!(matches!(rx.recv().await, Some(AgentEvent::TextDelta { text }) if text == "first"));
    send.await;
    assert!(matches!(rx.recv().await, Some(AgentEvent::TextDelta { text }) if text == "second"));
}
