#![cfg(test)]

use super::*;
use std::collections::BTreeMap;
use std::future::Future;
use std::task::Poll;

async fn payload_bytes(inbox: &RunControlInbox) -> Vec<u8> {
    let state = inbox.state.lock().await;
    let queue: Vec<_> = state
        .queue
        .iter()
        .map(|pending| (&pending.request, &pending.receipt))
        .collect();
    let in_flight: BTreeMap<_, _> = state
        .in_flight
        .iter()
        .map(|(id, pending)| (id, (&pending.request, &pending.receipt)))
        .collect();
    let seen: BTreeMap<_, _> = state
        .seen
        .iter()
        .map(|(id, seen)| (id, (&seen.fingerprint, &seen.request, &seen.receipt)))
        .collect();
    serde_json::to_vec(&serde_json::json!({
        "queue": queue,
        "in_flight": in_flight,
        "seen": seen,
        "seen_order": state.seen_order,
        "last_sequence": state.last_sequence,
        "turn_id": state.turn_id,
        "turn_revision": state.turn_revision,
        "interrupt_requested": state.interrupt_requested,
    }))
    .unwrap()
}

#[tokio::test]
async fn seal_preserves_queued_bytes_and_exact_retry_without_cancelling() {
    let cancellation = CancellationToken::new();
    let inbox = RunControlInbox::new("session-original", "run-original", cancellation.clone());
    let request = RunControlRequest::steer("run-original", "保留原始输入\n\t  ")
        .with_session_id("session-original");
    let accepted = inbox.submit(request.clone(), 11).await.unwrap();
    let before = payload_bytes(&inbox).await;

    let observed = inbox.seal_and_observe_pending().await;
    assert!(observed.has_accepted());
    assert_eq!(observed.queued_accepted, 1);
    assert_eq!(observed.in_flight_accepted, 0);
    assert_eq!(observed.retained_accepted, 1);
    assert_eq!(payload_bytes(&inbox).await, before);
    assert!(!inbox.snapshot().await.active);
    assert!(!cancellation.is_cancelled());
    assert_eq!(inbox.submit(request.clone(), 12).await.unwrap(), accepted);

    let mut conflict = request;
    conflict.command = RunControlCommand::Steer {
        input: "different exact payload".into(),
    };
    assert!(matches!(
        inbox.submit(conflict, 13).await,
        Err(RunControlError::DuplicateRequest { .. })
    ));
    assert!(matches!(
        inbox
            .submit(RunControlRequest::steer("run-original", "new"), 14)
            .await,
        Err(RunControlError::Closed)
    ));
    assert_eq!(payload_bytes(&inbox).await, before);
    assert_eq!(inbox.seal_and_observe_pending().await, observed);
}

#[tokio::test]
async fn seal_observes_drained_accepted_controls_with_an_empty_queue() {
    let inbox = RunControlInbox::new("session-original", "run-original", CancellationToken::new());
    for input in ["first original payload", "second original payload"] {
        inbox
            .submit(RunControlRequest::steer("run-original", input), 11)
            .await
            .unwrap();
    }
    let actual_drained = inbox.drain().await;
    assert_eq!(actual_drained.len(), 2);
    assert_eq!(inbox.snapshot().await.queued_controls, 0);
    let before = payload_bytes(&inbox).await;

    let observed = inbox.seal_and_observe_pending().await;
    assert!(observed.has_accepted());
    assert_eq!(observed.queued_accepted, 0);
    assert_eq!(observed.in_flight_accepted, 2);
    assert_eq!(observed.retained_accepted, 2);
    assert_eq!(payload_bytes(&inbox).await, before);
    for pending in actual_drained {
        assert_eq!(
            inbox.submit(pending.request, 12).await.unwrap(),
            pending.receipt
        );
    }
}

#[tokio::test]
async fn seal_retains_in_flight_payloads_after_seen_receipt_eviction() {
    let inbox = RunControlInbox::new("session-original", "run-original", CancellationToken::new());
    let first = RunControlRequest::steer("run-original", "oldest accepted input remains owned");
    inbox.submit(first.clone(), 11).await.unwrap();
    let first_drained = inbox.drain().await;
    // Inject the historical orphan directly: current bounded admission must
    // never evict an Accepted receipt or admit unlimited in-flight controls.
    {
        let mut state = inbox.state.lock().await;
        state.seen.remove(&first.request_id);
        state.seen_order.retain(|id| id != &first.request_id);
        let retained = state.in_flight.get(&first.request_id).unwrap();
        assert_eq!(retained.request, first);
        assert_eq!(retained.receipt, first_drained[0].receipt);
    }
    let before = payload_bytes(&inbox).await;

    let observed = inbox.seal_and_observe_pending().await;
    assert!(observed.has_accepted());
    assert_eq!(observed.queued_accepted, 0);
    assert_eq!(observed.in_flight_accepted, 1);
    assert_eq!(observed.retained_accepted, 0);
    assert_eq!(payload_bytes(&inbox).await, before);
}

#[tokio::test]
async fn original_application_after_seal_changes_only_the_real_receipt() {
    let inbox = RunControlInbox::new("session-original", "run-original", CancellationToken::new());
    let request = RunControlRequest::steer("run-original", "actual original application");
    inbox.submit(request.clone(), 11).await.unwrap();
    let pending = inbox.drain().await.pop().unwrap();
    assert!(inbox.seal_and_observe_pending().await.has_accepted());

    let applied = inbox
        .mark_applied(&pending, Some("turn-original".into()), 2, 12)
        .await;
    assert_eq!(applied.state, RunControlReceiptState::Applied);
    assert_eq!(applied.request_id, request.request_id);
    assert!(applied.error.is_none());
    assert!(!inbox.seal_and_observe_pending().await.has_accepted());
    assert!(!inbox.snapshot().await.active);
    assert_eq!(inbox.submit(request.clone(), 13).await.unwrap(), applied);
    let state = inbox.state.lock().await;
    assert_eq!(
        state.seen.get(&request.request_id).unwrap().request,
        request
    );
    assert!(state.in_flight.is_empty());
    assert!(state.queue.is_empty());
}

#[tokio::test]
async fn cancelled_seal_waiter_preserves_acceptance_and_original_payloads() {
    let inbox = RunControlInbox::new("session-original", "run-original", CancellationToken::new());
    let request = RunControlRequest::steer("run-original", "waiter loss preserves this input");
    inbox.submit(request.clone(), 11).await.unwrap();
    let before = payload_bytes(&inbox).await;
    let actual_admission = inbox.admission.lock().await;
    let mut seal = Box::pin(inbox.seal_and_observe_pending());
    std::future::poll_fn(|cx| {
        assert!(seal.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(seal);
    assert!(inbox.snapshot().await.active);
    assert_eq!(payload_bytes(&inbox).await, before);
    drop(actual_admission);

    assert!(inbox.seal_and_observe_pending().await.has_accepted());
    assert_eq!(payload_bytes(&inbox).await, before);
    assert_eq!(
        inbox.submit(request, 12).await.unwrap().state,
        RunControlReceiptState::Accepted
    );
}
