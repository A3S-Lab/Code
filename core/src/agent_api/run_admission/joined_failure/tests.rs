#![cfg(test)]
use super::*;

#[test]
fn failure_wire_closed_set_round_trips_and_refuses_unknown() {
    for value in [
        OriginalFailureWire::NotApplicable,
        OriginalFailureWire::NotAttempted,
        OriginalFailureWire::Queued,
        OriginalFailureWire::Full,
        OriginalFailureWire::Closed,
    ] {
        assert_eq!(OriginalFailureWire::parse(value.as_str()).unwrap(), value);
    }
    assert!(OriginalFailureWire::parse("unknown")
        .unwrap_err()
        .to_string()
        .contains("original failure wire"));
}

#[derive(Debug)]
struct ReceiptBarrier {
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
    errors: std::sync::atomic::AtomicUsize,
    block_event: bool,
    panic_receipt: bool,
    event_entered: tokio::sync::Semaphore,
    event_release: tokio::sync::Semaphore,
}
#[async_trait::async_trait]
impl HookExecutor for ReceiptBarrier {
    async fn fire(&self, _: &crate::hooks::HookEvent) -> crate::hooks::HookResult {
        crate::hooks::HookResult::continue_()
    }
    async fn record_run_control(
        &self,
        _: &crate::run_control::RunControlRequest,
        receipt: &crate::run_control::RunControlReceipt,
    ) {
        if receipt.state == crate::run_control::RunControlReceiptState::Settled {
            self.entered.add_permits(1);
            assert!(!self.panic_receipt, "actual Settled receipt observer panic");
            self.release.acquire().await.unwrap().forget();
        }
    }
    async fn record_agent_event(&self, event: &AgentEvent, _: &str, _: &str) {
        assert!(matches!(event, AgentEvent::Error { .. }));
        self.errors.fetch_add(1, Ordering::SeqCst);
        if self.block_event {
            self.event_entered.add_permits(1);
            self.event_release.acquire().await.unwrap().forget();
        }
    }
}

struct NoProvider;
#[async_trait::async_trait]
impl crate::llm::LlmClient for NoProvider {
    async fn complete(
        &self,
        _: &[crate::llm::Message],
        _: Option<&str>,
        _: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<crate::llm::LlmResponse> {
        panic!("component must never enter a provider");
    }
    async fn complete_streaming(
        &self,
        _: &[crate::llm::Message],
        _: Option<&str>,
        _: &[crate::llm::ToolDefinition],
        _: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
        panic!("component must never enter a provider");
    }
}

async fn transport_case(
    wire: OriginalFailureWire,
    cancelled: bool,
    prior_sealed: bool,
    panic_receipt: bool,
) {
    use crate::agent_api::execution_coordinator::ExecutionCoordinator;
    use crate::agent_api::run_lifecycle::RunControlState;
    use crate::run_control::{RunControlReceiptState, RunControlRequest};
    let workspace = tempfile::tempdir().unwrap();
    let mut config = crate::agent_api::test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        ..Default::default()
    });
    let agent = crate::Agent::from_config(config).await.unwrap();
    let hook = Arc::new(ReceiptBarrier {
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
        errors: std::sync::atomic::AtomicUsize::new(0),
        block_event: prior_sealed,
        panic_receipt,
        event_entered: tokio::sync::Semaphore::new(0),
        event_release: tokio::sync::Semaphore::new(0),
    });
    let session = agent
        .build_session(
            workspace.path().display().to_string(),
            Arc::new(NoProvider),
            &crate::SessionOptions::new()
                .with_session_id("joined-component")
                .with_hook_executor(hook.clone()),
        )
        .unwrap();
    let lease = ExecutionCoordinator::admit(&session, "joined-component")
        .await
        .unwrap();
    let (handoff, start, original) = OriginalWorkerHandoff::prepare(
        &session,
        lease,
        ExecutionMode::Streaming,
        session.session_cancel.child_token(),
    )
    .unwrap();
    let control = RunControlState::from_session(&session)
        .bind_original(original.clone())
        .unwrap();
    let store = session.run_store.clone();
    let (sender, receiver) = mpsc::channel(1);
    let mut receiver = Some(receiver);
    if wire == OriginalFailureWire::Full {
        sender
            .try_send(AgentEvent::Error {
                message: "existing buffered event".into(),
            })
            .unwrap();
    } else if wire == OriginalFailureWire::Closed {
        drop(receiver.take());
    }
    let publisher = OriginalFailurePublisher::freeze(
        store.clone(),
        Some(hook.clone()),
        "joined-component".into(),
        Some(sender),
    );
    let context = original.clone();
    let worker_hook = hook.clone();
    let worker_store = store.clone();
    let (ready, ready_rx) = oneshot::channel();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        assert!(matches!(
            control
                .reserve_run_with_id("joined-run", "component")
                .await
                .unwrap(),
            crate::run::RunReservation::Created(_)
        ));
        let inbox = RunControlInbox::new_with_hook_executor(
            "joined-component",
            "joined-run",
            context.original_cancel.clone(),
            Some(worker_hook.clone()),
        );
        context.bind_inbox_once(inbox.clone()).unwrap();
        control
            .attach_owned_run_control("joined-run", inbox.clone())
            .await
            .unwrap();
        context.record_no_committer().unwrap();
        let request = RunControlRequest::steer("joined-run", "exact unretained input");
        inbox.submit_with_hooks(request.clone(), 1).await.unwrap();
        // Both queue and retained receipt authority survive the actual abort.
        assert_eq!(inbox.drain().await.len(), 1);
        if prior_sealed {
            let sealed = context.seal_publication().unwrap();
            let event = AgentEvent::Error {
                message: "original worker failure".into(),
            };
            let mut actual = OriginalPublicationObservation {
                kind: OriginalPublicationKind::Error,
                terminal: OriginalStoreObservation::Entered,
                diagnostic: OriginalStoreObservation::NotApplicable,
                cancellation_hook: OriginalHookObservation::NotApplicable,
                event_hook: OriginalHookObservation::NotAttempted,
            };
            context
                .record_publication_progress(&sealed, &actual)
                .unwrap();
            actual.terminal = OriginalStoreObservation::Returned(Box::new(
                worker_store.record_event("joined-run", event.clone()).await,
            ));
            context
                .record_publication_progress(&sealed, &actual)
                .unwrap();
            actual.event_hook = OriginalHookObservation::Entered;
            context
                .record_publication_progress(&sealed, &actual)
                .unwrap();
            ready
                .send((context.created_handle().unwrap(), inbox, request))
                .unwrap();
            // Real original store returned, then real event hook entered and
            // remains blocked when the actual worker is aborted.
            worker_hook
                .record_agent_event(&event, "joined-run", "joined-component")
                .await;
            panic!("actual original blocked hook must never return");
        }
        ready
            .send((context.created_handle().unwrap(), inbox, request))
            .unwrap();
        std::future::pending::<()>().await;
    });
    let abort = worker.abort_handle();
    let (disposition, observed) = oneshot::channel();
    let proxy =
        handoff
            .with_failure_publisher(publisher)
            .supervise(worker, vec![abort], disposition);
    let (run, inbox, request) = tokio::time::timeout(Duration::from_secs(5), ready_rx)
        .await
        .unwrap()
        .unwrap();
    // A non-guardian cannot invoke continuation while the actual worker lives.
    assert!(original
        .continue_joined_failure(OriginalFailurePublisher::freeze(
            store.clone(),
            Some(hook.clone()),
            "joined-component".into(),
            None
        ))
        .await
        .is_err());
    assert_eq!(
        inbox.submit(request.clone(), 2).await.unwrap().state,
        RunControlReceiptState::Accepted
    );
    if prior_sealed {
        tokio::time::timeout(Duration::from_secs(5), hook.event_entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
    if cancelled {
        assert!(run.cancel().await);
    }
    assert!(session
        .run_admission
        .abort_original_stream_workers(&original));
    tokio::time::timeout(Duration::from_secs(5), hook.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    assert_eq!(
        original.observed_worker_join(),
        Some(ActualJoinObservation::Cancelled)
    );
    let receipt = inbox.submit(request.clone(), 3).await.unwrap();
    assert_eq!(receipt.state, RunControlReceiptState::Settled);
    assert_eq!(
        receipt.error.as_ref().unwrap().code,
        "STEER_PERSISTENCE_FAILED"
    );
    // EOF already available although the receipt observer is still blocked.
    if let Some(mut receiver) = receiver {
        let first = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap();
        if prior_sealed {
            assert!(first.is_none());
        } else if wire == OriginalFailureWire::Full {
            assert!(
                matches!(first, Some(AgentEvent::Error { message }) if message == "existing buffered event")
            );
        } else {
            assert!(matches!(first, Some(AgentEvent::Error { .. })));
        }
        assert!(
            tokio::time::timeout(Duration::from_secs(2), receiver.recv())
                .await
                .unwrap()
                .is_none()
        );
    }
    let events = store.events("joined-run").await;
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.event, AgentEvent::Error { .. }))
            .count(),
        1
    );
    {
        let state = original.state.lock().unwrap();
        assert!(!state.release.controls.unwrap().has_accepted());
        assert!(!state.release.publication_complete);
        assert_eq!(state.release.failure_wire, (!prior_sealed).then_some(wire));
        if prior_sealed {
            assert!(matches!(
                state.release.publication.as_ref().unwrap().terminal,
                OriginalStoreObservation::Returned(_)
            ));
            assert_eq!(
                state.release.publication.as_ref().unwrap().event_hook,
                OriginalHookObservation::Entered
            );
        }
    }
    if !prior_sealed {
        assert_eq!(
            store.snapshot("joined-run").await.unwrap().status,
            if cancelled {
                crate::run::RunStatus::Cancelled
            } else {
                crate::run::RunStatus::Failed
            }
        );
    }
    if panic_receipt {
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), observed)
                .await
                .unwrap()
                .expect("panicking observer must not unwind the original guardian"),
            OwnerDisposition::RetainedUnproved
        );
        assert!(!proxy.is_finished());
        assert!(session
            .run_admission
            .current_original()
            .is_some_and(|actual| Arc::ptr_eq(&actual, &original)));
        assert!(matches!(
            session.run_admission.try_acquire("joined-component"),
            Err(CodeError::SessionBusy { .. })
        ));
        assert_eq!(session.task_scheduler.health().await.unwrap().active, 1);
        {
            let state = original.state.lock().unwrap();
            assert!(!state.release.publication_complete);
            assert!(state.failure_continuation.as_ref().unwrap().actual.is_err());
            assert!(state.actual_retired.is_none());
        }
        assert_eq!(hook.errors.load(Ordering::SeqCst), 0);
        return;
    }
    assert!(matches!(
        session.run_admission.try_acquire("joined-component"),
        Err(CodeError::SessionBusy { .. })
    ));
    hook.release.add_permits(1);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), observed)
            .await
            .unwrap()
            .unwrap(),
        OwnerDisposition::RetainedUnproved
    );
    assert!(!proxy.is_finished());
    assert_eq!(hook.errors.load(Ordering::SeqCst), 1);
    assert_eq!(inbox.submit(request, 4).await.unwrap(), receipt);
    let retained_events = store.events("joined-run").await.len();
    // A foreign observer cannot repeat even the already joined publication.
    assert!(original
        .continue_joined_failure(OriginalFailurePublisher::freeze(
            store.clone(),
            Some(hook.clone()),
            "joined-component".into(),
            None
        ))
        .await
        .is_err());
    assert_eq!(store.events("joined-run").await.len(), retained_events);
}

#[tokio::test]
async fn queued_error_drops_sender_before_blocked_receipt_hook_and_retains_owner() {
    transport_case(OriginalFailureWire::Queued, false, false, false).await;
}
#[tokio::test]
async fn full_error_channel_is_observed_once_without_waiting_or_retry() {
    transport_case(OriginalFailureWire::Full, false, false, false).await;
}
#[tokio::test]
async fn closed_error_channel_preserves_pinned_cancellation_and_retains_owner() {
    transport_case(OriginalFailureWire::Closed, true, false, false).await;
}
#[tokio::test]
async fn actual_original_store_and_entered_hook_are_never_retried_or_replaced() {
    transport_case(OriginalFailureWire::Queued, false, true, false).await;
}

#[tokio::test]
async fn proved_no_run_drops_publisher_without_error_and_retires_original_lease() {
    use crate::agent_api::execution_coordinator::ExecutionCoordinator;
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(crate::agent_api::test_config())
        .await
        .unwrap();
    let session = agent
        .build_session(
            workspace.path().display().to_string(),
            Arc::new(NoProvider),
            &crate::SessionOptions::new().with_session_id("proved-no-run"),
        )
        .unwrap();
    let lease = ExecutionCoordinator::admit(&session, "proved-no-run")
        .await
        .unwrap();
    let (handoff, start, original) = OriginalWorkerHandoff::prepare(
        &session,
        lease,
        ExecutionMode::Streaming,
        session.session_cancel.child_token(),
    )
    .unwrap();
    let (sender, mut receiver) = mpsc::channel(1);
    let publisher = OriginalFailurePublisher::freeze(
        session.run_store.clone(),
        None,
        "proved-no-run".into(),
        Some(sender),
    );
    let cleanup = crate::agent_api::runtime_events::RunCleanupState::from_session(&session, "");
    let context = original.clone();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        context
            .record_scope_not_attempted(OriginalScopeKind::Command)
            .unwrap();
        context
            .record_scope_not_attempted(OriginalScopeKind::Fact)
            .unwrap();
        context.record_no_run_branch().unwrap();
        context.record_no_inbox().unwrap();
        context.record_no_committer().unwrap();
        context.seal_original_controls().await.unwrap();
        let sealed = context.seal_publication().unwrap();
        context
            .record_publication(
                sealed,
                OriginalPublicationObservation {
                    kind: OriginalPublicationKind::NoRun,
                    terminal: OriginalStoreObservation::NotApplicable,
                    diagnostic: OriginalStoreObservation::NotApplicable,
                    cancellation_hook: OriginalHookObservation::NotApplicable,
                    event_hook: OriginalHookObservation::NotApplicable,
                },
            )
            .unwrap();
        cleanup.finish_owned(&context).await.unwrap();
    });
    let (tx, rx) = oneshot::channel();
    let proxy = handoff
        .with_failure_publisher(publisher)
        .supervise(worker, Vec::new(), tx);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .unwrap()
            .unwrap(),
        OwnerDisposition::Released
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap()
            .is_none()
    );
    proxy.await.unwrap();
    assert!(session.run_admission.current_original().is_none());
    assert!(session.run_store.snapshot("proved-no-run").await.is_none());
    assert!(original
        .state
        .lock()
        .unwrap()
        .release
        .failure_wire
        .is_none());
    assert!(session.run_admission.try_acquire("proved-no-run").is_ok());
}

#[tokio::test]
async fn panicking_receipt_observer_cannot_release_unproved_original_owner() {
    transport_case(OriginalFailureWire::Queued, false, false, true).await;
}
