#![cfg(test)]

use super::*;
use a3s_effect::{append_input, message_fact, ActorError, FileLog, LogStore, MemoryLog};
use std::sync::Mutex as StdMutex;

async fn adapter_campaign(log: &dyn LogStore) {
    let inbox = RunControlInbox::new("session", "run", CancellationToken::new());
    let first = RunControlRequest::steer("run", "首条\n\t  ");
    let second = RunControlRequest::steer("run", "second exact input");
    inbox.submit(first.clone(), 1).await.unwrap();
    assert_eq!(inbox.drain().await.len(), 1);
    inbox.submit(second.clone(), 2).await.unwrap();
    let mut order = Vec::new();
    let decided = inbox
        .persist_pending(true, 3, |item| {
            order.push(item.receipt.sequence);
            let RunControlCommand::Steer { input } = &item.request.command else {
                panic!()
            };
            append_input(
                log,
                "session",
                message_fact(&item.receipt.request_id, input),
            )
        })
        .await;
    assert!(decided.failure.is_none());
    assert_eq!(order, [1, 2]);
    assert_eq!(decided.decisions.len(), 2);
    assert!(!inbox.seal_and_observe_pending().await.has_accepted());
    let facts = log.read("session").unwrap();
    assert_eq!(facts.len(), 2);
    assert_eq!(facts[0].payload["text"], "首条\n\t  ");
    assert_eq!(facts[1].payload["text"], "second exact input");
    for request in [first, second] {
        assert_eq!(
            inbox.submit(request.clone(), 4).await.unwrap().state,
            RunControlReceiptState::Applied
        );
        let mut conflict = request;
        conflict.command = RunControlCommand::Steer {
            input: "changed".into(),
        };
        assert!(matches!(
            inbox.submit(conflict, 4).await,
            Err(RunControlError::DuplicateRequest { .. })
        ));
    }
    let repeated = inbox
        .persist_pending(true, 5, |_| panic!("terminal receipt is immutable"))
        .await;
    assert!(repeated.decisions.is_empty());
    assert_eq!(log.read("session").unwrap(), facts);
}

#[tokio::test]
async fn ordered_queued_and_in_flight_inputs_use_the_same_memory_and_file_contract() {
    adapter_campaign(&MemoryLog::new()).await;
    let directory = tempfile::tempdir().unwrap();
    adapter_campaign(&FileLog::open(directory.path()).unwrap()).await;
    let reopened = FileLog::open(directory.path()).unwrap();
    assert_eq!(reopened.read("session").unwrap().len(), 2);
}

#[tokio::test]
async fn first_error_preserves_prefix_and_fails_unattempted_steers_but_applies_interrupt() {
    let inbox = RunControlInbox::new("session", "run", CancellationToken::new());
    let mut requests = Vec::new();
    for text in ["prefix", "failing", "unattempted"] {
        let request = RunControlRequest::steer("run", text);
        inbox.submit(request.clone(), 1).await.unwrap();
        requests.push(request);
    }
    let interrupt = RunControlRequest::interrupt("run");
    inbox.submit(interrupt.clone(), 1).await.unwrap();
    let mut attempted = Vec::new();
    let original = ActorError::Schema("original IO failure".into());
    let decided = inbox
        .persist_pending(true, 2, |item| {
            attempted.push(item.receipt.sequence);
            if item.receipt.sequence == 2 {
                Err(original.clone())
            } else {
                Ok(())
            }
        })
        .await;
    assert_eq!(attempted, [1, 2]);
    assert_eq!(decided.failure, Some(original));
    for (index, request) in requests.into_iter().enumerate() {
        let receipt = inbox.submit(request, 3).await.unwrap();
        if index == 0 {
            assert_eq!(receipt.state, RunControlReceiptState::Applied);
            assert!(receipt.error.is_none());
        } else {
            assert_eq!(receipt.state, RunControlReceiptState::Settled);
            assert_eq!(receipt.error.unwrap().code, "STEER_PERSISTENCE_FAILED");
        }
    }
    assert_eq!(
        inbox.submit(interrupt, 3).await.unwrap().state,
        RunControlReceiptState::Applied
    );
    assert!(inbox.is_cancelled());
    assert!(!inbox.seal_and_observe_pending().await.has_accepted());
}

#[tokio::test]
async fn owned_accepted_survives_terminal_cache_churn_and_in_flight_counts_toward_bound() {
    let inbox = RunControlInbox::new("session", "run", CancellationToken::new());
    let first = RunControlRequest::steer("run", "retain the oldest accepted receipt");
    let accepted = inbox.submit(first.clone(), 1).await.unwrap();
    inbox.drain().await;
    // Exercise the retained receipt cache without applying the original input.
    for index in 0..RUN_CONTROL_MAX_SEEN_REQUESTS + 16 {
        let request = RunControlRequest::steer("run", format!("other-{index}"));
        inbox.submit(request, 2).await.unwrap();
        let pending = inbox.drain().await.pop().unwrap();
        inbox.mark_applied(&pending, None, 0, 3).await;
    }
    assert_eq!(inbox.submit(first.clone(), 4).await.unwrap(), accepted);
    assert!(inbox.state.lock().await.seen.len() <= RUN_CONTROL_MAX_SEEN_REQUESTS);
    for index in 1..RUN_CONTROL_MAX_QUEUE {
        inbox
            .submit(
                RunControlRequest::steer("run", format!("pending-{index}")),
                5,
            )
            .await
            .unwrap();
        inbox.drain().await;
    }
    assert!(matches!(
        inbox
            .submit(RunControlRequest::steer("run", "overflow"), 6)
            .await,
        Err(RunControlError::QueueFull)
    ));
    assert_eq!(inbox.submit(first, 6).await.unwrap(), accepted);
}

#[derive(Debug)]
struct ReentrantObserver {
    inbox: StdMutex<std::sync::Weak<RunControlInbox>>,
    observed: StdMutex<Vec<RunControlReceiptState>>,
}
#[async_trait::async_trait]
impl HookExecutor for ReentrantObserver {
    async fn fire(&self, _: &crate::hooks::HookEvent) -> crate::hooks::HookResult {
        crate::hooks::HookResult::continue_()
    }
    async fn record_run_control(&self, _: &RunControlRequest, receipt: &RunControlReceipt) {
        if receipt.state != RunControlReceiptState::Accepted {
            let inbox = self.inbox.lock().unwrap().upgrade().unwrap();
            // Both transition locks must be released before calling observers.
            let admission = inbox
                .admission
                .try_lock()
                .expect("post-hook outside admission");
            let state = inbox.state.try_lock().expect("post-hook outside state");
            assert_eq!(state.seen[&receipt.request_id].receipt, *receipt);
            drop(state);
            drop(admission);
            assert!(!inbox.snapshot().await.active);
        }
        self.observed.lock().unwrap().push(receipt.state);
    }
}

#[tokio::test]
async fn terminal_receipt_wins_over_stale_accepted_queue_and_in_flight_copies() {
    let inbox = RunControlInbox::new("session", "run", CancellationToken::new());
    let request = RunControlRequest::steer("run", "already committed");
    inbox.submit(request.clone(), 1).await.unwrap();
    let pending = inbox.drain().await.pop().unwrap();
    let applied = inbox.mark_applied(&pending, None, 0, 2).await;
    {
        let mut state = inbox.state.lock().await;
        state.queue.push_back(pending.clone());
        state.in_flight.insert(request.request_id.clone(), pending);
    }
    let decided = inbox
        .persist_pending(true, 3, |_| panic!("must not reappend terminal input"))
        .await;
    assert!(decided.decisions.is_empty());
    assert!(!inbox.seal_and_observe_pending().await.has_accepted());
    assert_eq!(inbox.submit(request, 4).await.unwrap(), applied);
}

#[tokio::test]
async fn post_observer_can_reenter_after_the_append_and_ack_have_both_decided() {
    let hook = Arc::new(ReentrantObserver {
        inbox: StdMutex::new(std::sync::Weak::new()),
        observed: StdMutex::new(vec![]),
    });
    let inbox = RunControlInbox::new_with_hook_executor(
        "session",
        "run",
        CancellationToken::new(),
        Some(hook.clone()),
    );
    *hook.inbox.lock().unwrap() = Arc::downgrade(&inbox);
    inbox
        .submit_with_hooks(RunControlRequest::steer("run", "input"), 1)
        .await
        .unwrap();
    let decided = inbox
        .persist_pending(true, 2, |_| {
            assert!(inbox.admission.try_lock().is_err());
            assert!(inbox.state.try_lock().is_err());
            Ok(())
        })
        .await;
    assert!(decided.failure.is_none());
    assert!(!decided.pending.has_accepted());
    assert_eq!(
        *hook.observed.lock().unwrap(),
        [RunControlReceiptState::Accepted]
    );
    inbox.observe_decisions(&decided.decisions).await;
    assert_eq!(
        *hook.observed.lock().unwrap(),
        [
            RunControlReceiptState::Accepted,
            RunControlReceiptState::Applied
        ]
    );
}
