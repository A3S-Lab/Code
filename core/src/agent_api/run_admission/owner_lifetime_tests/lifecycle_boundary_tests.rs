#![cfg(test)]

use super::*;
use crate::hooks::{HookExecutor, HookOutcome};
use crate::run_control::{
    InterruptRequest, RunControlError, RunControlReceiptState, RunControlRequest, SteerRequest,
};
use tokio::sync::Semaphore;

#[tokio::test]
async fn goal_run_proof_requires_actual_created_run_log_worker_and_unsealed_owner() {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    let control =
        super::super::super::run_lifecycle::RunControlState::from_session(&fixture.session)
            .bind_original(original.clone())
            .unwrap();
    let actual_log = original.original_fact_log.as_ref().unwrap().clone();
    assert!(!original.model_call_port().admits_original_run(
        &actual_log,
        "original-t002",
        "goal-original"
    ));
    let context = original.clone();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        let port = context.model_call_port();
        assert!(!port.admits_original_run(&actual_log, "original-t002", "goal-original"));
        let (run, _) = actual_owned_run(&control, &context, "goal-original").await;
        assert!(port.admits_original_run(&actual_log, "original-t002", run.id()));
        assert!(!port.admits_original_run(&actual_log, "original-t002", "foreign-run"));
        assert!(!port.admits_original_run(&actual_log, "foreign-session", run.id()));
        let foreign = tempfile::tempdir().unwrap();
        let foreign_log = Arc::new(a3s_effect::FileLog::open(foreign.path()).unwrap());
        assert!(!port.admits_original_run(&foreign_log, "original-t002", run.id()));
        context.seal_original_controls().await.unwrap();
        let _sealed = context.seal_publication().unwrap();
        assert!(!port.admits_original_run(&actual_log, "original-t002", run.id()));
    });
    let (sent, receipt) = oneshot::channel();
    let proxy = handoff.supervise(worker, Vec::new(), sent);
    // This boundary fixture deliberately constructs no execution/closeout proof.
    assert_eq!(
        disposition(receipt).await,
        OwnerDisposition::RetainedUnproved
    );
    assert!(!proxy.is_finished());
    assert_eq!(fixture.model.0.load(Ordering::SeqCst), 0);
}

#[derive(Debug)]
struct CancellationHook {
    entered: Semaphore,
    token: CancellationToken,
    block_before: bool,
}

#[async_trait::async_trait]
impl HookExecutor for CancellationHook {
    async fn fire(&self, _: &crate::hooks::HookEvent) -> crate::hooks::HookResult {
        crate::hooks::HookResult::continue_()
    }
    async fn before_run_control(&self, _: &RunControlRequest) -> HookOutcome {
        if self.block_before {
            self.entered.add_permits(1);
            self.token.cancelled().await;
        }
        HookOutcome::Skip
    }
    async fn record_run_control(
        &self,
        _: &RunControlRequest,
        receipt: &crate::run_control::RunControlReceipt,
    ) {
        if !self.block_before && receipt.state == RunControlReceiptState::Accepted {
            self.entered.add_permits(1);
            self.token.cancelled().await;
        }
    }
}

async fn cancellation_must_unblock_original_hook(block_before: bool, close: bool, interrupt: bool) {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    let control =
        super::super::super::run_lifecycle::RunControlState::from_session(&fixture.session)
            .bind_original(original.clone())
            .unwrap();
    let hook = Arc::new(CancellationHook {
        entered: Semaphore::new(0),
        token: original.original_cancel.clone(),
        block_before,
    });
    let worker_hook = hook.clone();
    let context = original.clone();
    let (ready_tx, ready_rx) = oneshot::channel();
    let (finish_tx, finish_rx) = oneshot::channel();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        control
            .reserve_run_with_id("seal-before-cancel", "original fixture")
            .await
            .unwrap();
        let inbox = crate::run_control::RunControlInbox::new_with_hook_executor(
            "original-t002",
            "seal-before-cancel",
            context.original_cancel.clone(),
            Some(worker_hook),
        );
        context.bind_inbox_once(inbox.clone()).unwrap();
        control
            .attach_owned_run_control("seal-before-cancel", inbox.clone())
            .await
            .unwrap();
        ready_tx
            .send((context.created_handle().unwrap(), inbox))
            .unwrap();
        finish_rx.await.unwrap();
        // No model/capability/committer constructors: retained owner is expected.
    });
    let (tx, rx) = oneshot::channel();
    let _proxy = handoff.supervise(worker, Vec::new(), tx);
    let (run, inbox) = ready_rx.await.unwrap();
    let limit = Duration::from_millis(250);
    if interrupt {
        let receipt =
            tokio::time::timeout(limit, fixture.session.interrupt(InterruptRequest::new()))
                .await
                .expect("accepted Interrupt must cancel before a token-waiting receipt observer")
                .unwrap();
        assert_eq!(receipt.state, RunControlReceiptState::Accepted);
        hook.entered.acquire().await.unwrap().forget();
        assert_eq!(inbox.snapshot().await.queued_controls, 1);
    } else {
        let mut request = SteerRequest::new("retain exactly");
        request.request_id = Some("original-hook-steer".into());
        let mut steering = Box::pin(fixture.session.steer(request.clone()));
        assert!(futures::poll!(&mut steering).is_pending());
        hook.entered.acquire().await.unwrap().forget();
        assert!(!original.original_cancel.is_cancelled());
        let accepted_cancel = if close {
            tokio::time::timeout(limit, original.request_session_close()).await
        } else {
            tokio::time::timeout(limit, run.cancel()).await
        }
        .expect("host cancellation must not wait for its original token-waiting hook");
        assert!(accepted_cancel);
        assert!(!inbox.snapshot().await.active);
        if block_before {
            assert!(matches!(
                steering.await,
                Err(CodeError::RunControl(RunControlError::Closed))
            ));
            assert_eq!(inbox.snapshot().await.last_sequence, 0);
        } else {
            let accepted = steering.await.unwrap();
            assert_eq!(accepted.state, RunControlReceiptState::Accepted);
            assert_eq!(fixture.session.steer(request).await.unwrap(), accepted);
            assert_eq!(inbox.snapshot().await.queued_controls, 1);
        }
    }
    assert!(original.original_cancel.is_cancelled());
    assert!(!inbox.snapshot().await.active);
    assert!(fixture.session.active_run_control.try_lock().is_ok());
    assert!(fixture.session.current_run_id.try_lock().is_ok());
    assert!(fixture.session.cancel_token.try_lock().is_ok());
    assert_eq!(run.status().await, Some(crate::run::RunStatus::Created));
    assert!(run.events().await.is_empty());
    finish_tx.send(()).unwrap();
    assert_eq!(disposition(rx).await, OwnerDisposition::RetainedUnproved);
    assert_eq!(
        original.observed_worker_join(),
        Some(ActualJoinObservation::Completed)
    );
    assert_eq!(health(&fixture).await.active, 1);
    assert_eq!(fixture.model.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn actual_host_cancel_unblocks_original_policy_waiting_for_cancel() {
    cancellation_must_unblock_original_hook(true, false, false).await;
}
#[tokio::test]
async fn actual_host_cancel_unblocks_original_receipt_waiting_for_cancel() {
    cancellation_must_unblock_original_hook(false, false, false).await;
}
#[tokio::test]
async fn actual_close_request_unblocks_original_receipt_waiting_for_cancel() {
    cancellation_must_unblock_original_hook(false, true, false).await;
}
#[tokio::test]
async fn actual_interrupt_cancels_before_original_receipt_observer() {
    cancellation_must_unblock_original_hook(false, false, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_host_cancel_revalidates_foreign_same_id_after_state_lock_wait() {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    let control =
        super::super::super::run_lifecycle::RunControlState::from_session(&fixture.session)
            .bind_original(original.clone())
            .unwrap();
    let context = original.clone();
    let (ready_tx, ready_rx) = oneshot::channel();
    let (finish_tx, finish_rx) = oneshot::channel();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        ready_tx
            .send(actual_owned_run(&control, &context, "foreign-during-seal").await)
            .unwrap();
        finish_rx.await.unwrap();
    });
    let (tx, rx) = oneshot::channel();
    let _proxy = handoff.supervise(worker, Vec::new(), tx);
    let (run, inbox) = ready_rx.await.unwrap();
    fixture
        .session
        .steer(SteerRequest::new("exact input"))
        .await
        .unwrap();
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let actual = inbox.clone();
    let persistence = tokio::spawn(async move {
        let mut entered = Some(entered_tx);
        actual
            .persist_pending(false, 10, move |_| {
                entered.take().unwrap().send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
            .await
    });
    entered_rx.await.unwrap();
    let mut cancelling = Box::pin(run.cancel());
    assert!(futures::poll!(&mut cancelling).is_pending());
    assert!(fixture.session.active_run_control.try_lock().is_ok());
    assert!(fixture.session.current_run_id.try_lock().is_ok());
    assert!(fixture.session.cancel_token.try_lock().is_ok());
    let foreign = crate::run_control::RunControlInbox::new(
        "original-t002",
        "foreign-during-seal",
        CancellationToken::new(),
    );
    *fixture.session.active_run_control.lock().await = Some(foreign.clone());
    release_tx.send(()).unwrap();
    assert!(persistence.await.unwrap().failure.is_none());
    assert!(!cancelling.await);
    assert!(!original.original_cancel.is_cancelled());
    assert!(!inbox.snapshot().await.active);
    assert!(foreign.snapshot().await.active);
    assert!(!foreign.is_cancelled());
    assert!(fixture.session.run_control_snapshot().await.is_none());
    finish_tx.send(()).unwrap();
    assert_eq!(disposition(rx).await, OwnerDisposition::RetainedUnproved);
    assert_eq!(
        original.observed_worker_join(),
        Some(ActualJoinObservation::Completed)
    );
}

#[tokio::test]
async fn public_exact_retries_use_retained_inbox_after_exact_slot_removal() {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    let control =
        super::super::super::run_lifecycle::RunControlState::from_session(&fixture.session)
            .bind_original(original.clone())
            .unwrap();
    let store = fixture.session.run_store.clone();
    let admission = fixture.session.run_admission.clone();
    let context = original.clone();
    let (ready_tx, ready_rx) = oneshot::channel();
    let (settle_tx, settle_rx) = oneshot::channel();
    let (retired_tx, retired_rx) = oneshot::channel();
    let (finish_tx, finish_rx) = oneshot::channel();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        let (run, inbox) = actual_owned_run(&control, &context, "retained-retry").await;
        inbox.update_turn_id("actual-turn".into()).await;
        ready_tx.send((run, inbox.clone())).unwrap();
        settle_rx.await.unwrap();
        // Only a component transaction is qualified here, not input durability.
        let decided = inbox.persist_pending(true, 10, |_| Ok(())).await;
        assert!(decided.failure.is_none());
        assert!(!decided.pending.has_accepted());
        actual_no_scope_constructors(&context);
        context.record_no_committer().unwrap();
        context.seal_original_controls().await.unwrap();
        actual_success_publication(&context, &store).await;
        context
            .retire_original_slots("original-t002", &admission, "retained-retry")
            .await
            .unwrap();
        retired_tx.send(()).unwrap();
        finish_rx.await.unwrap();
        context.record_tools_cleaned().unwrap();
    });
    let (tx, rx) = oneshot::channel();
    let proxy = handoff.supervise(worker, Vec::new(), tx);
    let (run, inbox) = ready_rx.await.unwrap();
    assert!(matches!(
        fixture
            .session
            .steer(SteerRequest::new("wrong Run").with_run_id("foreign-run"))
            .await,
        Err(CodeError::RunControl(RunControlError::RunMismatch { .. }))
    ));
    assert!(matches!(
        fixture
            .session
            .steer(SteerRequest::new("stale turn").with_expected_turn("foreign-turn", 0))
            .await,
        Err(CodeError::RunControl(RunControlError::StaleTurn { .. }))
    ));
    let mut expired = SteerRequest::new("expired");
    expired.deadline_ms = Some(0);
    assert!(matches!(
        fixture.session.steer(expired).await,
        Err(CodeError::RunControl(RunControlError::DeadlineExceeded))
    ));
    assert_eq!(inbox.snapshot().await.last_sequence, 0);
    let mut steer = SteerRequest::new("original request").with_run_id(run.id());
    steer.request_id = Some("exact-steer".into());
    let interrupt = InterruptRequest {
        run_id: Some(run.id().into()),
        request_id: Some("exact-interrupt".into()),
        ..Default::default()
    };
    assert_eq!(
        fixture.session.steer(steer.clone()).await.unwrap().state,
        RunControlReceiptState::Accepted
    );
    assert_eq!(
        fixture
            .session
            .interrupt(interrupt.clone())
            .await
            .unwrap()
            .state,
        RunControlReceiptState::Accepted
    );
    assert!(!inbox.snapshot().await.active); // Interrupt seals before cancelling.
    settle_tx.send(()).unwrap();
    retired_rx.await.unwrap();
    assert!(fixture.session.current_run_id.lock().await.is_none());
    assert!(fixture.session.cancel_token.lock().await.is_none());
    assert!(fixture.session.active_run_control.lock().await.is_none());
    assert!(fixture.session.current_run().await.is_none());
    assert!(!fixture.session.cancel().await);
    assert!(
        !fixture
            .session
            .cancel_and_settle(Duration::from_millis(1), Duration::from_millis(1))
            .await
    );
    let expected_steer = inbox
        .submit(steer.clone().into_protocol("original-t002", run.id()), 11)
        .await
        .unwrap();
    let expected_interrupt = inbox
        .submit(
            interrupt.clone().into_protocol("original-t002", run.id()),
            11,
        )
        .await
        .unwrap();
    assert_eq!(expected_steer.state, RunControlReceiptState::Applied);
    assert_eq!(expected_interrupt.state, RunControlReceiptState::Applied);
    assert_eq!(
        fixture.session.steer(steer.clone()).await.unwrap(),
        expected_steer
    );
    assert_eq!(
        fixture.session.interrupt(interrupt.clone()).await.unwrap(),
        expected_interrupt
    );
    assert!(!fixture.session.run_control_snapshot().await.unwrap().active);
    let mut conflicting = steer.clone();
    conflicting.input = "changed input".into();
    assert!(matches!(
        fixture.session.steer(conflicting).await,
        Err(CodeError::RunControl(
            RunControlError::DuplicateRequest { .. }
        ))
    ));
    assert!(matches!(
        fixture.session.steer(SteerRequest::new("new input")).await,
        Err(CodeError::RunControl(RunControlError::Closed))
    ));
    assert!(matches!(
        fixture.session.interrupt(InterruptRequest::default()).await,
        Err(CodeError::RunControl(RunControlError::Closed))
    ));
    assert_eq!(health(&fixture).await.active, 1);
    assert_eq!(fixture.model.0.load(Ordering::SeqCst), 0);
    assert!(!proxy.is_finished());
    let foreign = crate::run_control::RunControlInbox::new(
        "original-t002",
        "retained-retry",
        CancellationToken::new(),
    );
    *fixture.session.active_run_control.lock().await = Some(foreign.clone());
    assert!(matches!(
        fixture.session.steer(steer.clone()).await,
        Err(CodeError::RunControl(RunControlError::NoActiveRun))
    ));
    assert_eq!(foreign.snapshot().await.last_sequence, 0);
    *fixture.session.active_run_control.lock().await = None;
    assert_eq!(
        fixture.session.steer(steer.clone()).await.unwrap(),
        expected_steer
    );
    finish_tx.send(()).unwrap();
    assert_eq!(disposition(rx).await, OwnerDisposition::Released);
    proxy.await.unwrap();
    assert!(matches!(
        fixture.session.steer(steer).await,
        Err(CodeError::RunControl(RunControlError::NoActiveRun))
    ));
    assert!(matches!(
        fixture.session.interrupt(interrupt).await,
        Err(CodeError::RunControl(RunControlError::NoActiveRun))
    ));
    assert_eq!(fixture.model.0.load(Ordering::SeqCst), 0);
}

#[derive(Clone, Copy)]
enum MissingProof {
    Controls,
    Capability,
    Committer,
    Model,
}

async fn retirement_refuses_missing_proof(missing: MissingProof) {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    let control =
        super::super::super::run_lifecycle::RunControlState::from_session(&fixture.session)
            .bind_original(original.clone())
            .unwrap();
    let admission = fixture.session.run_admission.clone();
    let store = fixture.session.run_store.clone();
    let context = original.clone();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        let (_, inbox) = actual_owned_run(&control, &context, "incomplete-cleanup").await;
        if matches!(missing, MissingProof::Controls) {
            inbox
                .submit(
                    RunControlRequest::steer("incomplete-cleanup", "owned debt"),
                    1,
                )
                .await
                .unwrap();
        }
        if matches!(missing, MissingProof::Capability) {
            context
                .mark_scope_attempted(OriginalScopeKind::Command)
                .unwrap();
            context
                .record_scope_not_attempted(OriginalScopeKind::Fact)
                .unwrap();
        } else {
            actual_no_scope_constructors(&context);
        }
        let held_committer = if matches!(missing, MissingProof::Committer) {
            let (tx, rx) = oneshot::channel::<()>();
            let install = context.reserve_committer_install().unwrap();
            install.install(tokio::spawn(async move {
                let _ = rx.await;
            }));
            Some(tx)
        } else {
            context.record_no_committer().unwrap();
            None
        };
        let port = OriginalModelCallPort {
            original: Arc::downgrade(&context),
        };
        let model_reservation = if matches!(missing, MissingProof::Model) {
            Some(
                port.reserve_same_original_call("original-t002")
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        context.seal_original_controls().await.unwrap();
        actual_success_publication(&context, &store).await;
        assert!(
            context
                .retire_original_slots("original-t002", &admission, "incomplete-cleanup")
                .await
                .is_err(),
            "missing original proof must refuse slot mutation"
        );
        assert!(Arc::ptr_eq(
            context.active_slot.lock().await.as_ref().unwrap(),
            &inbox
        ));
        assert_eq!(
            context.current_slot.lock().await.as_deref(),
            Some("incomplete-cleanup")
        );
        assert_eq!(
            context.cancel_slot.lock().await.as_ref(),
            Some(&context.original_cancel)
        );
        assert!(!context.state.lock().unwrap().release.slots_retired);
        drop(model_reservation);
        drop(held_committer);
    });
    let (tx, rx) = oneshot::channel();
    let _proxy = handoff.supervise(worker, Vec::new(), tx);
    assert_eq!(disposition(rx).await, OwnerDisposition::RetainedUnproved);
    assert_eq!(
        original.observed_worker_join(),
        Some(ActualJoinObservation::Completed)
    );
    assert_eq!(health(&fixture).await.active, 1);
    assert_eq!(fixture.model.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn exact_slot_removal_refuses_original_accepted_controls() {
    retirement_refuses_missing_proof(MissingProof::Controls).await;
}
#[tokio::test]
async fn exact_slot_removal_refuses_missing_canonical_capability_close() {
    retirement_refuses_missing_proof(MissingProof::Capability).await;
}
#[tokio::test]
async fn exact_slot_removal_refuses_unjoined_original_committer() {
    retirement_refuses_missing_proof(MissingProof::Committer).await;
}
#[tokio::test]
async fn exact_slot_removal_refuses_unresolved_original_model_accounting() {
    retirement_refuses_missing_proof(MissingProof::Model).await;
}

#[tokio::test]
async fn retry_reservation_joins_same_child_and_survives_dropped_waiter() {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    let context = original.clone();
    let (installed, installed_rx) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        let port = context.model_call_port();
        let reservation = port
            .reserve_same_original_call("original-t002")
            .await
            .unwrap();
        let (start_child, started_child) = oneshot::channel();
        let child = tokio::spawn(async move {
            started_child.await.unwrap();
            released.await.unwrap();
            Err(a3s_effect::ActorError::Handler {
                key: "infer".into(),
                message: "same actual child settled".into(),
            })
        });
        let child_id = child.id();
        let attempt = CancellationToken::new();
        reservation.install_and_start(
            child,
            attempt.clone(),
            Arc::new(crate::agent::FactModelAccountingObservation::default()),
            start_child,
        );
        attempt.cancel();
        let mut retry = Box::pin(port.reserve_same_original_call("original-t002"));
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(
                std::future::Future::poll(retry.as_mut(), cx).is_pending()
            ))
            .await
        );
        drop(retry);
        {
            let state = context.model_call.state.lock().await;
            assert!(matches!(&*state, OriginalModelState::Running { actual, .. }
                if actual.id() == child_id));
        }
        installed.send(child_id).unwrap();
        // A second waiter must observe the same child before reserving reuse.
        let reservation = port
            .reserve_same_original_call("original-t002")
            .await
            .unwrap();
        drop(reservation);
        let state = context.model_call.state.lock().await;
        assert!(matches!(&*state, OriginalModelState::Ready(ModelReady {
            actual: Ok(Err(a3s_effect::ActorError::Handler { message, .. })),
            observed: true, attempt_cancelled: true, ..
        }) if message == "same actual child settled"));
    });
    let (tx, rx) = oneshot::channel();
    let proxy = handoff.supervise(worker, Vec::new(), tx);
    installed_rx.await.unwrap();
    assert_eq!(health(&fixture).await.active, 1);
    assert!(
        ExecutionCoordinator::admit(&fixture.session, "pending original retry")
            .await
            .is_err()
    );
    release.send(()).unwrap();
    // This component has no capability/publication proof: joining the child
    // alone must never manufacture a full release certificate.
    assert_eq!(disposition(rx).await, OwnerDisposition::RetainedUnproved);
    assert_eq!(
        (
            health(&fixture).await.active,
            health(&fixture).await.released
        ),
        (1, 0)
    );
    assert!(!proxy.is_finished());
    assert_eq!(fixture.model.0.load(Ordering::SeqCst), 0);
}
