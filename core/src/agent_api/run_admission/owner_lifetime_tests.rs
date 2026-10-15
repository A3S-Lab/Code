#![cfg(test)]

mod protocol_cancel_tests;

use super::*;
use crate::agent_api::execution_coordinator::ExecutionCoordinator;
use crate::agent_api::{Agent, AgentSession, SessionOptions};
use crate::task_scheduler::{TaskSchedulerConfig, TaskSchedulerError, TaskSchedulerHealthSnapshot};
use std::sync::atomic::AtomicUsize;

mod checkpoint_failure_tests;
mod committer_failure_tests;
mod lifecycle_boundary_tests;

#[derive(Default)]
struct NoModelClient(AtomicUsize);

#[async_trait::async_trait]
impl crate::llm::LlmClient for NoModelClient {
    async fn complete(
        &self,
        _messages: &[crate::llm::Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<crate::llm::LlmResponse> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("T002 component must not execute a model")
    }

    async fn complete_streaming(
        &self,
        _messages: &[crate::llm::Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        _cancellation: CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("T002 component must not execute a model")
    }
}

struct Fixture {
    _workspace: tempfile::TempDir,
    _agent: Agent,
    session: AgentSession,
    model: Arc<NoModelClient>,
}

async fn fixture() -> Fixture {
    let workspace = tempfile::tempdir().unwrap();
    let mut config = crate::agent_api::test_config();
    config.task_scheduler = TaskSchedulerConfig {
        max_active: 1,
        aging_interval_ms: 60_000,
    };
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        prune_policy: None,
        ..Default::default()
    });
    let agent = Agent::from_config(config).await.unwrap();
    let model = Arc::new(NoModelClient::default());
    let options = SessionOptions::new()
        .with_session_id("original-t002")
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()));
    let session = agent
        .build_session(
            workspace.path().display().to_string(),
            model.clone(),
            &options,
        )
        .unwrap();
    Fixture {
        _workspace: workspace,
        _agent: agent,
        session,
        model,
    }
}

async fn prepare(
    fixture: &Fixture,
    mode: ExecutionMode,
) -> (
    OriginalWorkerHandoff,
    OwnedWorkerStart,
    Arc<OriginalExecutionContext>,
    u64,
) {
    let lease = ExecutionCoordinator::admit(&fixture.session, "owner-component")
        .await
        .unwrap();
    let lease_id = lease.task_lease.as_ref().unwrap().id();
    let (handoff, start, original) = ExecutionCoordinator::prepare_original_handoff(
        &fixture.session,
        lease,
        mode,
        fixture.session.session_cancel.child_token(),
    )
    .unwrap();
    (handoff, start, original, lease_id)
}

async fn disposition(receiver: oneshot::Receiver<OwnerDisposition>) -> OwnerDisposition {
    tokio::time::timeout(Duration::from_secs(5), receiver)
        .await
        .unwrap()
        .unwrap()
}

async fn health(fixture: &Fixture) -> TaskSchedulerHealthSnapshot {
    fixture.session.task_scheduler.health().await.unwrap()
}

#[test]
fn owner_closed_sets_round_trip_and_refuse_unknown() {
    for (value, wire) in [
        (ExecutionMode::Blocking, "blocking"),
        (ExecutionMode::Streaming, "streaming"),
        (ExecutionMode::Detached, "detached"),
    ] {
        assert_eq!(value.as_str(), wire);
        assert_eq!(ExecutionMode::parse(wire).unwrap(), value);
    }
    for (value, wire) in [
        (EffectPhase::NeverStarted, "never_started"),
        (EffectPhase::MayHaveEffects, "may_have_effects"),
        (EffectPhase::WorkerFinished, "worker_finished"),
    ] {
        assert_eq!(value.as_str(), wire);
        assert_eq!(EffectPhase::parse(wire).unwrap(), value);
    }
    for (value, wire) in [
        (OwnerDisposition::NeverStarted, "never_started"),
        (OwnerDisposition::Released, "released"),
        (OwnerDisposition::RetainedUnproved, "retained_unproved"),
    ] {
        assert_eq!(value.as_str(), wire);
        assert_eq!(OwnerDisposition::parse(wire).unwrap(), value);
    }
    for (value, wire) in [
        (OriginalScopeKind::Command, "command"),
        (OriginalScopeKind::Fact, "fact"),
    ] {
        assert_eq!(value.as_str(), wire);
        assert_eq!(OriginalScopeKind::parse(wire).unwrap(), value);
    }
    for (value, wire) in [
        (OriginalCancelOrigin::Host, "host"),
        (OriginalCancelOrigin::Close, "close"),
        (OriginalCancelOrigin::Admission, "admission"),
    ] {
        assert_eq!(value.as_str(), wire);
        assert_eq!(OriginalCancelOrigin::parse(wire).unwrap(), value);
    }
    for (value, wire) in [
        (OriginalPublicationKind::End, "end"),
        (OriginalPublicationKind::Error, "error"),
        (OriginalPublicationKind::Cancelled, "cancelled"),
        (OriginalPublicationKind::AdmissionFailed, "admission_failed"),
        (OriginalPublicationKind::NoRun, "no_run"),
    ] {
        assert_eq!(value.as_str(), wire);
        assert_eq!(OriginalPublicationKind::parse(wire).unwrap(), value);
    }
    assert!(ExecutionMode::parse("unknown")
        .unwrap_err()
        .to_string()
        .contains("execution mode"));
    assert!(EffectPhase::parse("unknown")
        .unwrap_err()
        .to_string()
        .contains("effect phase"));
    assert!(OwnerDisposition::parse("unknown")
        .unwrap_err()
        .to_string()
        .contains("owner disposition"));
    assert!(OriginalScopeKind::parse("unknown")
        .unwrap_err()
        .to_string()
        .contains("original scope kind"));
    assert!(OriginalCancelOrigin::parse("unknown")
        .unwrap_err()
        .to_string()
        .contains("original cancel origin"));
    assert!(OriginalPublicationKind::parse("unknown")
        .unwrap_err()
        .to_string()
        .contains("original publication kind"));
}

#[tokio::test]
async fn owned_handoff_starts_only_after_guardian_has_actual_lease() {
    let fixture = fixture().await;
    let (handoff, start, original, lease_id) = prepare(&fixture, ExecutionMode::Blocking).await;
    let (started_tx, mut started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        started_tx.send(tokio::task::id()).unwrap();
        let _ = release_rx.await;
    });
    let worker_id = worker.id();
    assert!(matches!(
        started_rx.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(handoff.lease.task_lease.as_ref().unwrap().id(), lease_id);
    let before = health(&fixture).await;
    assert_eq!((before.admitted, before.active, before.released), (1, 1, 0));
    let abort = worker.abort_handle();
    let (disposition_tx, disposition_rx) = oneshot::channel();
    let proxy = handoff.supervise(worker, vec![abort], disposition_tx);
    assert_eq!(started_rx.await.unwrap(), worker_id);
    assert!(
        !fixture.session.run_admission.abort_stream_workers(),
        "blocking ownership must not register force-abort policy"
    );
    assert_eq!(original.state.lock().unwrap().worker_id, Some(worker_id));
    assert!(Arc::ptr_eq(
        &fixture.session.run_admission.current_original().unwrap(),
        &original
    ));
    assert!(fixture
        .session
        .run_admission
        .try_acquire("original-t002")
        .is_err());
    release_tx.send(()).unwrap();
    assert_eq!(
        disposition(disposition_rx).await,
        OwnerDisposition::RetainedUnproved
    );
    assert!(!proxy.is_finished());
    assert_eq!(fixture.model.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn closed_start_receiver_joins_and_releases_original_lease() {
    let fixture = fixture().await;
    let (handoff, start, original, lease_id) = prepare(&fixture, ExecutionMode::Blocking).await;
    let OriginalWorkerHandoff {
        runtime,
        lease,
        pin,
        original: registered,
        start: signal,
        failure,
    } = handoff;
    assert_eq!(lease.task_lease.as_ref().unwrap().id(), lease_id);
    drop(signal);
    let worker = runtime.spawn(async move {
        assert!(!start.wait().await.unwrap());
    });
    let worker_id = worker.id();
    let (disposition_tx, disposition_rx) = oneshot::channel();
    let proxy = spawn_guardian(
        Some(&runtime),
        worker,
        Vec::new(),
        lease,
        GuardianRegistration::Owned {
            pin,
            original: registered,
            disposition: disposition_tx,
            failure,
        },
    );
    assert_eq!(
        disposition(disposition_rx).await,
        OwnerDisposition::NeverStarted
    );
    proxy.await.unwrap();
    let released = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value = health(&fixture).await;
            if value.released == 1 {
                break value;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        (released.active, released.admitted, released.released),
        (0, 1, 1)
    );
    {
        let state = original.state.lock().unwrap();
        assert_eq!(state.worker_id, Some(worker_id));
        assert!(state.worker_join.as_ref().unwrap().is_ok());
        assert!(matches!(state.start, StartObservation::ReceiverClosed));
        assert!(fixture.session.run_admission.current_original().is_none());
    }
    assert!(fixture.session.run_store.list().await.is_empty());

    // A second actual admitted preflight is dropped before any worker spawn.
    // Its positive non-executing RAII is separate from the joined start-closed case.
    let (preflight, never_spawned_start, _, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    drop(never_spawned_start);
    drop(preflight);
    let released = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value = health(&fixture).await;
            if value.released == 2 {
                break value;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        (released.active, released.admitted, released.released),
        (0, 2, 2)
    );
    assert!(fixture.session.run_admission.current_original().is_none());
    assert!(fixture.session.run_store.list().await.is_empty());
    assert_eq!(fixture.model.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn foreign_start_signals_refuse_effects_and_join_each_original_worker() {
    let first = fixture().await;
    let second = fixture().await;
    let before_first = health(&first).await;
    let before_second = health(&second).await;
    let (mut first_handoff, first_start, first_original, first_lease) =
        prepare(&first, ExecutionMode::Blocking).await;
    let (mut second_handoff, second_start, second_original, second_lease) =
        prepare(&second, ExecutionMode::Blocking).await;
    assert!(!Arc::ptr_eq(&first_original, &second_original));
    assert_eq!(
        first_handoff.lease.task_lease.as_ref().unwrap().id(),
        first_lease
    );
    assert_eq!(
        second_handoff.lease.task_lease.as_ref().unwrap().id(),
        second_lease
    );
    std::mem::swap(&mut first_handoff.start, &mut second_handoff.start);
    let first_worker = tokio::spawn(async move {
        assert!(!first_start.wait().await.unwrap());
    });
    let second_worker = tokio::spawn(async move {
        assert!(!second_start.wait().await.unwrap());
    });
    let first_id = first_worker.id();
    let second_id = second_worker.id();
    let (first_tx, first_rx) = oneshot::channel();
    let (second_tx, second_rx) = oneshot::channel();
    let first_proxy = first_handoff.supervise(first_worker, Vec::new(), first_tx);
    let second_proxy = second_handoff.supervise(second_worker, Vec::new(), second_tx);
    assert_eq!(disposition(first_rx).await, OwnerDisposition::NeverStarted);
    assert_eq!(disposition(second_rx).await, OwnerDisposition::NeverStarted);
    first_proxy.await.unwrap();
    second_proxy.await.unwrap();
    for (fixture, original, worker_id, before) in [
        (&first, &first_original, first_id, before_first),
        (&second, &second_original, second_id, before_second),
    ] {
        let actual = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let actual = health(fixture).await;
                if actual.released == before.released + 1 {
                    break actual;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(actual.active, before.active);
        assert_eq!(actual.admitted, before.admitted + 1);
        {
            let state = original.state.lock().unwrap();
            assert_eq!(state.worker_id, Some(worker_id));
            assert!(state.worker_join.as_ref().unwrap().is_ok());
            assert!(matches!(state.start, StartObservation::ReceiverClosed));
            assert_eq!(state.actual_retired, Some(OwnerDisposition::NeverStarted));
        }
        assert!(fixture.session.run_admission.current_original().is_none());
        assert!(fixture.session.run_store.list().await.is_empty());
        assert_eq!(fixture.model.0.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn public_observer_abort_does_not_release_original_lease() {
    let fixture = fixture().await;
    let (handoff, start, original, lease_id) = prepare(&fixture, ExecutionMode::Streaming).await;
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        started_tx.send(()).unwrap();
        let _ = release_rx.await;
    });
    assert_eq!(handoff.lease.task_lease.as_ref().unwrap().id(), lease_id);
    let (tx, rx) = oneshot::channel();
    let proxy = handoff.supervise(worker, Vec::new(), tx);
    started_rx.await.unwrap();
    proxy.abort();
    assert!(proxy.await.unwrap_err().is_cancelled());
    let held = health(&fixture).await;
    assert_eq!((held.active, held.released), (1, 0));
    assert!(fixture
        .session
        .run_admission
        .try_acquire("original-t002")
        .is_err());
    release_tx.send(()).unwrap();
    assert_eq!(disposition(rx).await, OwnerDisposition::RetainedUnproved);
    assert!(original
        .state
        .lock()
        .unwrap()
        .worker_join
        .as_ref()
        .unwrap()
        .is_ok());
    assert_eq!(
        (
            health(&fixture).await.active,
            health(&fixture).await.released
        ),
        (1, 0)
    );
}

#[tokio::test]
async fn joined_effectful_worker_without_proof_retains_real_capacity() {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
    });
    let (tx, rx) = oneshot::channel();
    let _proxy = handoff.supervise(worker, Vec::new(), tx);
    assert_eq!(disposition(rx).await, OwnerDisposition::RetainedUnproved);
    assert!(original
        .state
        .lock()
        .unwrap()
        .worker_join
        .as_ref()
        .unwrap()
        .is_ok());
    let scheduler = Arc::clone(&fixture.session.task_scheduler);
    let cancel = CancellationToken::new();
    let queued = tokio::spawn({
        let cancel = cancel.clone();
        async move {
            scheduler
                .acquire(
                    crate::task_scheduler::TaskPriority::Foreground,
                    "blocked",
                    &cancel,
                )
                .await
        }
    });
    let saturated = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value = health(&fixture).await;
            if value.pending == 1 {
                break value;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        (saturated.max_active, saturated.active, saturated.released),
        (1, 1, 0)
    );
    assert!(!queued.is_finished());
    cancel.cancel();
    assert!(matches!(
        queued.await.unwrap(),
        Err(TaskSchedulerError::Cancelled)
    ));
    assert_eq!(
        (
            health(&fixture).await.active,
            health(&fixture).await.released
        ),
        (1, 0)
    );
}

#[tokio::test]
async fn actual_worker_join_error_retains_original_lease() {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Streaming).await;
    let (started_tx, started_rx) = oneshot::channel();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        started_tx.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    let abort = worker.abort_handle();
    let (tx, rx) = oneshot::channel();
    let _proxy = handoff.supervise(worker, vec![abort], tx);
    started_rx.await.unwrap();
    assert!(fixture.session.run_admission.abort_stream_workers());
    assert_eq!(disposition(rx).await, OwnerDisposition::RetainedUnproved);
    assert!(original
        .state
        .lock()
        .unwrap()
        .worker_join
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap_err()
        .is_cancelled());
    assert_eq!(
        (
            health(&fixture).await.active,
            health(&fixture).await.released
        ),
        (1, 0)
    );
}

#[tokio::test]
async fn last_session_drop_keeps_original_close_identity_alive() {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    let weak = Arc::downgrade(&fixture.session.close_handle);
    let scheduler = Arc::clone(&fixture.session.task_scheduler);
    let admission = Arc::clone(&fixture.session.run_admission);
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        started_tx.send(()).unwrap();
        let _ = release_rx.await;
    });
    let (tx, rx) = oneshot::channel();
    let _proxy = handoff.supervise(worker, Vec::new(), tx);
    started_rx.await.unwrap();
    let Fixture {
        session,
        _agent,
        _workspace,
        model,
    } = fixture;
    drop(session);
    assert!(weak.upgrade().is_some());
    assert!(Arc::ptr_eq(
        &admission.current_original().unwrap(),
        &original
    ));
    let held = scheduler.health().await.unwrap();
    assert_eq!((held.active, held.released), (1, 0));
    release_tx.send(()).unwrap();
    assert_eq!(disposition(rx).await, OwnerDisposition::RetainedUnproved);
    assert!(weak.upgrade().is_some());
    assert_eq!(model.0.load(Ordering::SeqCst), 0);
    // No Agent.close/registry/replacement operation is used as cleanup proof.
}

#[tokio::test]
async fn borrowed_committer_wait_cancellation_retains_actual_handle() {
    let cell = OriginalCommitterCell::default();
    let install = cell.reserve_install().unwrap();
    let (release_tx, release_rx) = oneshot::channel();
    let actual = tokio::spawn(async move {
        let _ = release_rx.await;
    });
    let actual_id = actual.id();
    install.install(actual);
    let mut waiting = Box::pin(cell.wait());
    tokio::select! {
        biased;
        actual = &mut waiting => panic!("committer returned before its real barrier: {actual:?}"),
        _ = tokio::task::yield_now() => {}
    }
    drop(waiting);
    match &*cell.state.try_lock().unwrap() {
        OriginalCommitterState::Running(handle) => assert_eq!(handle.id(), actual_id),
        other => panic!("cancelled borrowed wait lost original handle: {other:?}"),
    }
    release_tx.send(()).unwrap();
    assert_eq!(
        cell.wait().await.unwrap(),
        ActualCommitterJoin::Joined(ActualJoinObservation::Completed)
    );
    assert!(matches!(
        &*cell.state.lock().await,
        OriginalCommitterState::Joined(Ok(()))
    ));
}

#[tokio::test]
async fn actual_committer_join_error_is_not_success() {
    let cell = OriginalCommitterCell::default();
    let install = cell.reserve_install().unwrap();
    let actual = tokio::spawn(std::future::pending::<()>());
    let actual_id = actual.id();
    actual.abort();
    install.install(actual);
    assert_eq!(
        cell.wait().await.unwrap(),
        ActualCommitterJoin::Joined(ActualJoinObservation::Cancelled)
    );
    match &*cell.state.lock().await {
        OriginalCommitterState::Joined(Err(error)) => {
            assert!(error.is_cancelled());
            assert_eq!(error.id(), actual_id);
        }
        other => panic!("actual JoinError not retained: {other:?}"),
    };
}

#[tokio::test]
async fn unconstructed_committer_is_unproved_and_repeated_install_refuses() {
    let cell = OriginalCommitterCell::default();
    assert!(cell.wait().await.is_err());
    let install = cell.reserve_install().unwrap();
    assert!(cell.reserve_install().is_err());
    let actual = tokio::spawn(std::future::pending::<()>());
    let abort = actual.abort_handle();
    install.install(actual);
    assert!(cell.reserve_install().is_err());
    assert!(cell.positively_disabled().is_err());
    abort.abort();
    assert_eq!(
        cell.wait().await.unwrap(),
        ActualCommitterJoin::Joined(ActualJoinObservation::Cancelled)
    );
    let disabled = OriginalCommitterCell::default();
    disabled.positively_disabled().unwrap();
    assert_eq!(
        disabled.wait().await.unwrap(),
        ActualCommitterJoin::PositivelyDisabled
    );
    assert!(disabled.reserve_install().is_err());
}

// Same concrete model-cell component: no production activation or release proof.
#[tokio::test]
async fn actual_model_handle_survives_cancelled_borrow_and_guardian_waits_worker_first() {
    let fixture = fixture().await;
    let (handoff, start, original, _lease_id) = prepare(&fixture, ExecutionMode::Blocking).await;
    let context = original.clone();
    let (installed, installed_rx) = oneshot::channel();
    let (finish_child, finished_child) = oneshot::channel();
    let (finish_worker, finished_worker) = oneshot::channel();
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
            finished_child.await.unwrap();
            Err(a3s_effect::ActorError::Handler {
                key: "infer".into(),
                message: "actual child error without provider entry".into(),
            })
        });
        let child_id = child.id();
        reservation.install_and_start(
            child,
            CancellationToken::new(),
            Arc::new(crate::agent::FactModelAccountingObservation::default()),
            start_child,
        );
        // Poll the actual borrowed handle once, then cancel only that waiter.
        let mut waiter = Box::pin(port.wait_same_original_call());
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(
                std::future::Future::poll(waiter.as_mut(), cx).is_pending()
            ))
            .await
        );
        drop(waiter);
        installed.send(child_id).unwrap();
        finished_worker.await.unwrap();
    });
    let worker_id = worker.id();
    let (sent, mut receipt) = oneshot::channel();
    let proxy = handoff.supervise(worker, Vec::new(), sent);
    let actual_child_id = installed_rx.await.unwrap();
    {
        let cell = original.model_call.state.lock().await;
        assert!(
            matches!(&*cell, OriginalModelState::Running { actual, .. } if actual.id() == actual_child_id)
        );
    }
    assert!(original.state.lock().unwrap().worker_join.is_none());
    assert_eq!(original.state.lock().unwrap().worker_id, Some(worker_id));
    assert_eq!(health(&fixture).await.active, 1);
    assert!(
        ExecutionCoordinator::admit(&fixture.session, "second while actual child pending")
            .await
            .is_err()
    );
    finish_worker.send(()).unwrap();
    // Locking the cell only after its prior guard has dropped does not take
    // the handle. Guardian's actual model wait may now hold that same mutex.
    assert!(matches!(
        receipt.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    finish_child.send(()).unwrap();
    assert_eq!(
        disposition(receipt).await,
        OwnerDisposition::RetainedUnproved
    );
    let cell = original.model_call.state.lock().await;
    assert!(
        matches!(&*cell, OriginalModelState::Ready(ModelReady { actual: Ok(Err(a3s_effect::ActorError::Handler { message, .. })), observed: true, .. }) if message == "actual child error without provider entry")
    );
    drop(cell);
    assert_eq!(health(&fixture).await.active, 1);
    assert_eq!(health(&fixture).await.released, 0);
    assert!(!proxy.is_finished());
    assert_eq!(fixture.model.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn actual_model_panic_is_retained_and_never_reusable_or_success() {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    let context = original.clone();
    let (observed, observation) = oneshot::channel();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        let port = context.model_call_port();
        let reservation = port
            .reserve_same_original_call("original-t002")
            .await
            .unwrap();
        let (start_child, started_child) = oneshot::channel();
        let actual = tokio::spawn(async move {
            started_child.await.unwrap();
            std::future::poll_fn(
                |_| -> std::task::Poll<
                    std::result::Result<crate::agent::FactModelCallOutcome, a3s_effect::ActorError>,
                > {
                    panic!("actual model child panic");
                },
            )
            .await
        });
        reservation.install_and_start(
            actual,
            CancellationToken::new(),
            Arc::new(crate::agent::FactModelAccountingObservation::default()),
            start_child,
        );
        assert!(
            matches!(port.wait_same_original_call().await, Err(a3s_effect::ActorError::Defect(message)) if message.contains("panicked"))
        );
        assert!(port
            .reserve_same_original_call("original-t002")
            .await
            .is_err());
        observed.send(()).unwrap();
    });
    let (sent, receipt) = oneshot::channel();
    let proxy = handoff.supervise(worker, Vec::new(), sent);
    observation.await.unwrap();
    assert_eq!(
        disposition(receipt).await,
        OwnerDisposition::RetainedUnproved
    );
    let cell = original.model_call.state.lock().await;
    assert!(
        matches!(&*cell, OriginalModelState::Ready(ModelReady { actual: Err(error), observed: false, .. }) if error.is_panic())
    );
    drop(cell);
    assert_eq!(health(&fixture).await.active, 1);
    assert_eq!(health(&fixture).await.released, 0);
    assert!(!proxy.is_finished());
}

#[tokio::test]
async fn unspawned_original_reservation_restores_actual_vacant_and_wrong_worker_cannot_mint() {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    assert!(original
        .model_call_port()
        .reserve_same_original_call("original-t002")
        .await
        .is_err());
    let context = original.clone();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        let port = context.model_call_port();
        let reservation = port
            .reserve_same_original_call("original-t002")
            .await
            .unwrap();
        drop(reservation);
        assert!(context.model_call.is_vacant());
        assert_eq!(context.model_call.wait().await, ModelCallJoin::NeverEntered);
    });
    let (sent, receipt) = oneshot::channel();
    let proxy = handoff.supervise(worker, Vec::new(), sent);
    assert_eq!(
        disposition(receipt).await,
        OwnerDisposition::RetainedUnproved
    );
    assert!(original.model_call.is_vacant());
    assert_eq!(health(&fixture).await.active, 1);
    assert!(!proxy.is_finished());
    assert_eq!(fixture.model.0.load(Ordering::SeqCst), 0);
}

async fn actual_owned_run(
    control: &super::super::run_lifecycle::RunControlState,
    original: &Arc<OriginalExecutionContext>,
    run_id: &str,
) -> (
    crate::run::RunHandle,
    Arc<crate::run_control::RunControlInbox>,
) {
    let actual = control
        .reserve_run_with_id(run_id, "original fixture")
        .await
        .unwrap();
    assert!(matches!(actual, crate::run::RunReservation::Created(_)));
    let token = original.original_cancel.clone();
    original.validate_original_token(&token).unwrap();
    let inbox = crate::run_control::RunControlInbox::new("original-t002", run_id, token);
    original.bind_inbox_once(inbox.clone()).unwrap();
    control
        .attach_owned_run_control(run_id, inbox.clone())
        .await
        .unwrap();
    (original.created_handle().unwrap(), inbox)
}

async fn actual_success_publication(
    original: &Arc<OriginalExecutionContext>,
    store: &crate::run::InMemoryRunStore,
) {
    let sealed = original.seal_publication().unwrap();
    let target = original.publication_target(&sealed).unwrap();
    let run_id = target.run_id.unwrap();
    let mut observation = OriginalPublicationObservation {
        kind: if target.cancelled {
            OriginalPublicationKind::Cancelled
        } else {
            OriginalPublicationKind::End
        },
        terminal: OriginalStoreObservation::Entered,
        diagnostic: OriginalStoreObservation::NotApplicable,
        cancellation_hook: OriginalHookObservation::NotApplicable,
        event_hook: OriginalHookObservation::NotApplicable,
    };
    original
        .record_publication_progress(&sealed, &observation)
        .unwrap();
    let actual = if target.cancelled {
        store.mark_cancelled(&run_id).await
    } else {
        store
            .record_event(
                &run_id,
                crate::agent::AgentEvent::End {
                    text: "actual completion".into(),
                    usage: Default::default(),
                    verification_summary: Box::new(
                        crate::verification::VerificationSummary::from_reports(&[]),
                    ),
                    completion_terminal: Default::default(),
                    forced_stop: None,
                    meta: None,
                },
            )
            .await
    };
    observation.terminal = OriginalStoreObservation::Returned(Box::new(actual));
    original.record_publication(sealed, observation).unwrap();
}

fn actual_no_scope_constructors(original: &OriginalExecutionContext) {
    // These component workers contain neither capability-admit constructor.
    original
        .record_scope_not_attempted(OriginalScopeKind::Command)
        .unwrap();
    original
        .record_scope_not_attempted(OriginalScopeKind::Fact)
        .unwrap();
}

#[tokio::test]
async fn original_request_waiter_drop_is_inert_and_accepted_request_has_no_store_tail() {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    let control = super::super::run_lifecycle::RunControlState::from_session(&fixture.session)
        .bind_original(original.clone())
        .unwrap();
    let cleanup = super::super::runtime_events::RunCleanupState::from_session(
        &fixture.session,
        "cancel-original",
    );
    let store = fixture.session.run_store.clone();
    let context = original.clone();
    let (ready_tx, ready_rx) = oneshot::channel();
    let (finish_tx, finish_rx) = oneshot::channel();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        let (run, _) = actual_owned_run(&control, &context, "cancel-original").await;
        ready_tx.send(run).unwrap();
        finish_rx.await.unwrap();
        actual_no_scope_constructors(&context);
        context.record_no_committer().unwrap();
        context.seal_original_controls().await.unwrap();
        actual_success_publication(&context, &store).await;
        cleanup.finish_owned(&context).await.unwrap();
    });
    let (tx, rx) = oneshot::channel();
    let proxy = handoff.supervise(worker, Vec::new(), tx);
    let run = ready_rx.await.unwrap();
    let held = fixture.session.active_run_control.lock().await;
    let mut waiting = Box::pin(run.cancel());
    assert!(matches!(
        futures::poll!(&mut waiting),
        std::task::Poll::Pending
    ));
    drop(waiting);
    assert!(!original.original_cancel.is_cancelled());
    assert!(original
        .state
        .lock()
        .unwrap()
        .release
        .cancel_origin
        .is_none());
    assert_eq!(run.status().await, Some(crate::run::RunStatus::Created));
    drop(held);
    assert!(run.cancel().await);
    assert!(run.cancel().await); // request already accepted, no duplicate writer
    assert!(original.original_cancel.is_cancelled());
    assert_eq!(run.status().await, Some(crate::run::RunStatus::Created));
    assert!(run.events().await.is_empty());
    assert_eq!(
        (
            health(&fixture).await.active,
            health(&fixture).await.released
        ),
        (1, 0)
    );
    finish_tx.send(()).unwrap();
    assert_eq!(disposition(rx).await, OwnerDisposition::Released);
    proxy.await.unwrap();
    assert_eq!(run.status().await, Some(crate::run::RunStatus::Cancelled));
    assert!(!run.cancel().await);
    assert!(fixture.session.current_run_id.lock().await.is_none());
    assert!(fixture.session.cancel_token.lock().await.is_none());
    assert!(fixture.session.active_run_control.lock().await.is_none());
    assert!(fixture.session.run_admission.current_original().is_none());
}

#[tokio::test]
async fn same_id_foreign_inbox_and_token_refuse_cancel_and_cleanup_without_mutation() {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    let control = super::super::run_lifecycle::RunControlState::from_session(&fixture.session)
        .bind_original(original.clone())
        .unwrap();
    let cleanup =
        super::super::runtime_events::RunCleanupState::from_session(&fixture.session, "same-label");
    let store = fixture.session.run_store.clone();
    let tools = fixture.session.active_tools.clone();
    let context = original.clone();
    let (ready_tx, ready_rx) = oneshot::channel();
    let (finish_tx, finish_rx) = oneshot::channel();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        let (run, inbox) = actual_owned_run(&control, &context, "same-label").await;
        ready_tx.send(run).unwrap();
        finish_rx.await.unwrap();
        actual_no_scope_constructors(&context);
        context.record_no_committer().unwrap();
        context.seal_original_controls().await.unwrap();
        actual_success_publication(&context, &store).await;
        let foreign = crate::run_control::RunControlInbox::new(
            "original-t002",
            "same-label",
            CancellationToken::new(),
        );
        *context.active_slot.lock().await = Some(foreign.clone());
        tools.write().await.insert(
            "original-tool".into(),
            super::super::runtime_events::ActiveToolState {
                tool_name: "actual".into(),
                started_at_ms: 1,
            },
        );
        assert!(cleanup.finish_owned(&context).await.is_err());
        assert!(Arc::ptr_eq(
            context.active_slot.lock().await.as_ref().unwrap(),
            &foreign
        ));
        assert_eq!(tools.read().await.len(), 1);
        assert_eq!(
            context.current_slot.lock().await.as_deref(),
            Some("same-label")
        );
        *context.active_slot.lock().await = Some(inbox);
        *context.cancel_slot.lock().await = Some(CancellationToken::new());
        assert!(cleanup.finish_owned(&context).await.is_err());
        assert_eq!(tools.read().await.len(), 1);
        *context.cancel_slot.lock().await = Some(context.original_cancel.clone());
        cleanup.finish_owned(&context).await.unwrap();
        assert!(tools.read().await.is_empty());
    });
    let (tx, rx) = oneshot::channel();
    let proxy = handoff.supervise(worker, Vec::new(), tx);
    let run = ready_rx.await.unwrap();
    let actual = fixture
        .session
        .active_run_control
        .lock()
        .await
        .clone()
        .unwrap();
    let foreign = crate::run_control::RunControlInbox::new(
        "original-t002",
        "same-label",
        CancellationToken::new(),
    );
    *fixture.session.active_run_control.lock().await = Some(foreign.clone());
    assert!(!run.cancel().await);
    assert!(!original.original_cancel.is_cancelled());
    assert!(Arc::ptr_eq(
        fixture
            .session
            .active_run_control
            .lock()
            .await
            .as_ref()
            .unwrap(),
        &foreign
    ));
    *fixture.session.active_run_control.lock().await = Some(actual);
    *fixture.session.cancel_token.lock().await = Some(CancellationToken::new());
    assert!(!run.cancel().await);
    assert!(!original.original_cancel.is_cancelled());
    *fixture.session.cancel_token.lock().await = Some(original.original_cancel.clone());
    finish_tx.send(()).unwrap();
    assert_eq!(disposition(rx).await, OwnerDisposition::Released);
    proxy.await.unwrap();
}

#[tokio::test]
async fn publication_seal_rejects_late_requests_and_real_committer_blocks_capacity_release() {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    let control = super::super::run_lifecycle::RunControlState::from_session(&fixture.session)
        .bind_original(original.clone())
        .unwrap();
    let cleanup = super::super::runtime_events::RunCleanupState::from_session(
        &fixture.session,
        "sealed-original",
    );
    let store = fixture.session.run_store.clone();
    let context = original.clone();
    let (published_tx, published_rx) = oneshot::channel();
    let (join_tx, join_rx) = oneshot::channel();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        let (run, _) = actual_owned_run(&control, &context, "sealed-original").await;
        actual_no_scope_constructors(&context);
        let install = context.reserve_committer_install().unwrap();
        let actual = tokio::spawn(async move {
            join_rx.await.unwrap();
        });
        install.install(actual);
        context.seal_original_controls().await.unwrap();
        actual_success_publication(&context, &store).await;
        assert!(cleanup.finish_owned(&context).await.is_err());
        published_tx.send(run).unwrap();
        assert!(matches!(
            context.wait_committer().await.unwrap(),
            ActualCommitterJoin::Joined(ActualJoinObservation::Completed)
        ));
        cleanup.finish_owned(&context).await.unwrap();
    });
    let (tx, rx) = oneshot::channel();
    let proxy = handoff.supervise(worker, Vec::new(), tx);
    let run = published_rx.await.unwrap();
    assert!(!run.cancel().await);
    assert!(!original.request_session_close().await);
    assert_eq!(run.status().await, Some(crate::run::RunStatus::Completed));
    assert_eq!(
        (
            health(&fixture).await.active,
            health(&fixture).await.released
        ),
        (1, 0)
    );
    assert!(fixture.session.run_admission.current_original().is_some());
    assert!(!proxy.is_finished());
    join_tx.send(()).unwrap();
    assert_eq!(disposition(rx).await, OwnerDisposition::Released);
    proxy.await.unwrap();
}

#[tokio::test]
async fn earlier_cancelled_ack_cannot_mask_dropped_second_error_publication() {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    let control = super::super::run_lifecycle::RunControlState::from_session(&fixture.session)
        .bind_original(original.clone())
        .unwrap();
    let store = fixture.session.run_store.clone();
    let context = original.clone();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        actual_owned_run(&control, &context, "partial-original").await;
        actual_no_scope_constructors(&context);
        context.record_no_committer().unwrap();
        context.seal_original_controls().await.unwrap();
        context.record_admission_cancel().unwrap();
        let sealed = context.seal_publication().unwrap();
        let mut actual = OriginalPublicationObservation {
            kind: OriginalPublicationKind::Error,
            terminal: OriginalStoreObservation::Entered,
            diagnostic: OriginalStoreObservation::NotAttempted,
            cancellation_hook: OriginalHookObservation::NotApplicable,
            event_hook: OriginalHookObservation::NotApplicable,
        };
        context
            .record_publication_progress(&sealed, &actual)
            .unwrap();
        actual.terminal = OriginalStoreObservation::Returned(Box::new(
            store.mark_cancelled("partial-original").await,
        ));
        context
            .record_publication_progress(&sealed, &actual)
            .unwrap();
        let previous = actual.clone();
        actual.terminal = OriginalStoreObservation::NotAttempted;
        assert!(context
            .record_publication_progress(&sealed, &actual)
            .is_err());
        actual = previous;
        actual.diagnostic = OriginalStoreObservation::Entered;
        context
            .record_publication_progress(&sealed, &actual)
            .unwrap();
        // Real second-store future is constructed but never polled/returned.
        // Entered means the caller attempted the operation; no callee effect is inferred.
        let pending = store.record_event(
            "partial-original",
            crate::agent::AgentEvent::Error {
                message: "actual later error".into(),
            },
        );
        drop(pending);
        context.record_publication(sealed, actual).unwrap();
    });
    let (tx, rx) = oneshot::channel();
    let _proxy = handoff.supervise(worker, Vec::new(), tx);
    assert_eq!(disposition(rx).await, OwnerDisposition::RetainedUnproved);
    {
        let state = original.state.lock().unwrap();
        let actual = state.release.publication.as_ref().unwrap();
        assert!(
            matches!(&actual.terminal, OriginalStoreObservation::Returned(ack) if ack.as_ref().as_ref().is_some_and(|ack| ack.status == crate::run::RunStatus::Cancelled))
        );
        assert!(matches!(
            actual.diagnostic,
            OriginalStoreObservation::Entered
        ));
    }
    assert_eq!(
        fixture
            .session
            .run_store
            .snapshot("partial-original")
            .await
            .unwrap()
            .status,
        crate::run::RunStatus::Cancelled
    );
    assert_eq!(
        (
            health(&fixture).await.active,
            health(&fixture).await.released
        ),
        (1, 0)
    );
}

#[tokio::test]
async fn same_worker_two_actual_scope_closes_and_positive_no_run_release() {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    let capture = super::super::agent_loop_runtime::capture_owned_preparation(
        &fixture.session,
        original.original_cancel.clone(),
        true,
    )
    .unwrap();
    let cleanup = super::super::runtime_events::RunCleanupState::from_session(&fixture.session, "");
    let context = original.clone();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        assert!(context.seal_publication().is_err()); // default missing target is not NoRun
        let super::super::agent_loop_runtime::OwnedPreparationCapture {
            command,
            fact,
            boundary,
            cancellation,
            workspace,
            ..
        } = capture;
        context
            .mark_scope_attempted(OriginalScopeKind::Command)
            .unwrap();
        let (_, command, _) = super::super::agent_loop_runtime::admit_captured_runtime_projection(
            command.unwrap(),
            &boundary,
            cancellation.clone(),
            &workspace,
        )
        .await
        .unwrap();
        context
            .record_scope_returned(OriginalScopeKind::Command, &command)
            .unwrap();
        context
            .mark_scope_attempted(OriginalScopeKind::Fact)
            .unwrap();
        let (_, fact, _) = super::super::agent_loop_runtime::admit_captured_runtime_projection(
            fact,
            &boundary,
            cancellation,
            &workspace,
        )
        .await
        .unwrap();
        context
            .record_scope_returned(OriginalScopeKind::Fact, &fact)
            .unwrap();
        let command_return = command.close().await;
        assert!(command_return.as_ref().unwrap().is_clean());
        context
            .record_scope_close(OriginalScopeKind::Command, &command_return)
            .unwrap();
        let fact_return = fact.close().await;
        assert!(fact_return.as_ref().unwrap().is_clean());
        context
            .record_scope_close(OriginalScopeKind::Fact, &fact_return)
            .unwrap();
        assert!(context
            .record_scope_not_attempted(OriginalScopeKind::Fact)
            .is_err());
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
    let proxy = handoff.supervise(worker, Vec::new(), tx);
    assert_eq!(disposition(rx).await, OwnerDisposition::Released);
    proxy.await.unwrap();
    assert_eq!(
        fixture
            .session
            .run_admission
            .wait_for_original_release(&original, Duration::from_secs(1))
            .await,
        Some(OwnerDisposition::Released)
    );
    assert!(fixture.session.run_store.list().await.is_empty());
    assert!(fixture.session.run_admission.current_original().is_none());
    let fence = fixture
        .session
        .run_admission
        .try_acquire("original-t002")
        .unwrap();
    assert!(fence.is_nonexecuting_fence_for(&fixture.session.run_admission));
    assert!(!fence.is_nonexecuting_fence_for(&Arc::new(RunAdmission::default())));
    assert!(!fixture
        .session
        .run_admission
        .abort_original_stream_workers(&original));
    // A's actual retirement remains observable while B's nonexecuting fence is active.
    assert_eq!(
        fixture
            .session
            .run_admission
            .wait_for_original_release(&original, Duration::from_secs(1))
            .await,
        Some(OwnerDisposition::Released)
    );
    drop(fence);
}

#[tokio::test]
async fn original_accepted_control_debt_and_attempted_scope_cannot_qualify_release() {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    let control = super::super::run_lifecycle::RunControlState::from_session(&fixture.session)
        .bind_original(original.clone())
        .unwrap();
    let cleanup = super::super::runtime_events::RunCleanupState::from_session(
        &fixture.session,
        "accepted-original",
    );
    let store = fixture.session.run_store.clone();
    let context = original.clone();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        let (_, inbox) = actual_owned_run(&control, &context, "accepted-original").await;
        let request = crate::run_control::RunControlRequest::steer(
            "accepted-original",
            "typed fixture control",
        )
        .with_session_id("original-t002");
        let accepted = inbox.submit(request, 1).await.unwrap();
        assert_eq!(
            accepted.state,
            crate::run_control::RunControlReceiptState::Accepted
        );
        context
            .mark_scope_attempted(OriginalScopeKind::Command)
            .unwrap();
        assert!(context
            .record_scope_not_attempted(OriginalScopeKind::Command)
            .is_err());
        context
            .record_scope_not_attempted(OriginalScopeKind::Fact)
            .unwrap();
        context.record_no_committer().unwrap();
        context.seal_original_controls().await.unwrap();
        assert!(context
            .state
            .lock()
            .unwrap()
            .release
            .controls
            .unwrap()
            .has_accepted());
        actual_success_publication(&context, &store).await;
        assert!(cleanup.finish_owned(&context).await.is_err());
        assert_eq!(inbox.snapshot().await.queued_controls, 1); // refused cleanup never acknowledges debt
    });
    let (tx, rx) = oneshot::channel();
    let _proxy = handoff.supervise(worker, Vec::new(), tx);
    assert_eq!(disposition(rx).await, OwnerDisposition::RetainedUnproved);
    assert_eq!(
        original.observed_worker_join(),
        Some(ActualJoinObservation::Completed)
    );
    assert!(fixture
        .session
        .run_admission
        .wait_for_original_release(&original, Duration::from_millis(1))
        .await
        .is_none());
    assert_eq!(
        (
            health(&fixture).await.active,
            health(&fixture).await.released
        ),
        (1, 0)
    );
}

#[derive(Debug, Default)]
struct ActualPublicationHook {
    cancelled: AtomicUsize,
    entered: AtomicUsize,
    returned: AtomicUsize,
    release: Notify,
}

#[async_trait::async_trait]
impl crate::hooks::HookExecutor for ActualPublicationHook {
    async fn fire(&self, _: &crate::hooks::HookEvent) -> crate::hooks::HookResult {
        crate::hooks::HookResult::continue_()
    }
    async fn record_run_cancelled(&self, _: &str, _: &str, _: Option<&str>) {
        self.cancelled.fetch_add(1, Ordering::SeqCst);
    }
    async fn record_agent_event(&self, _: &crate::agent::AgentEvent, _: &str, _: &str) {
        self.entered.fetch_add(1, Ordering::SeqCst);
        self.release.notified().await;
        self.returned.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn real_cancelled_and_error_acks_keep_dropped_event_hook_unproved() {
    use crate::hooks::HookExecutor;
    let mut fixture = fixture().await;
    let hook = Arc::new(ActualPublicationHook::default());
    fixture.session.hook_executor = Some(hook.clone());
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    let control = super::super::run_lifecycle::RunControlState::from_session(&fixture.session)
        .bind_original(original.clone())
        .unwrap();
    let cleanup = super::super::runtime_events::RunCleanupState::from_session(
        &fixture.session,
        "hook-original",
    );
    let store = fixture.session.run_store.clone();
    let context = original.clone();
    let actual_hook = hook.clone();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        assert!(matches!(
            control
                .reserve_run_with_id("hook-original", "original fixture")
                .await
                .unwrap(),
            crate::run::RunReservation::Created(_)
        ));
        let token = context.original_cancel.clone();
        context.validate_original_token(&token).unwrap();
        let inbox = crate::run_control::RunControlInbox::new_with_hook_executor(
            "original-t002",
            "hook-original",
            token,
            Some(actual_hook.clone()),
        );
        context.bind_inbox_once(inbox.clone()).unwrap();
        control
            .attach_owned_run_control("hook-original", inbox)
            .await
            .unwrap();
        let run = context.created_handle().unwrap();
        assert!(run.cancel().await);
        actual_no_scope_constructors(&context);
        context.record_no_committer().unwrap();
        context.seal_original_controls().await.unwrap();
        let sealed = context.seal_publication().unwrap();
        let mut actual = OriginalPublicationObservation {
            kind: OriginalPublicationKind::Error,
            terminal: OriginalStoreObservation::Entered,
            diagnostic: OriginalStoreObservation::NotAttempted,
            cancellation_hook: OriginalHookObservation::NotAttempted,
            event_hook: OriginalHookObservation::NotAttempted,
        };
        context
            .record_publication_progress(&sealed, &actual)
            .unwrap();
        actual.terminal = OriginalStoreObservation::Returned(Box::new(
            store.mark_cancelled("hook-original").await,
        ));
        context
            .record_publication_progress(&sealed, &actual)
            .unwrap();
        actual.cancellation_hook = OriginalHookObservation::Entered;
        context
            .record_publication_progress(&sealed, &actual)
            .unwrap();
        actual_hook
            .record_run_cancelled("hook-original", "original-t002", Some("cancelled by host"))
            .await;
        actual.cancellation_hook = OriginalHookObservation::Returned;
        actual.diagnostic = OriginalStoreObservation::Entered;
        context
            .record_publication_progress(&sealed, &actual)
            .unwrap();
        let event = crate::agent::AgentEvent::Error {
            message: "actual canonical failure".into(),
        };
        actual.diagnostic = OriginalStoreObservation::Returned(Box::new(
            store.record_event("hook-original", event.clone()).await,
        ));
        actual.event_hook = OriginalHookObservation::Entered;
        context
            .record_publication_progress(&sealed, &actual)
            .unwrap();
        let mut pending =
            Box::pin(actual_hook.record_agent_event(&event, "hook-original", "original-t002"));
        assert!(matches!(
            futures::poll!(&mut pending),
            std::task::Poll::Pending
        ));
        assert_eq!(actual_hook.entered.load(Ordering::SeqCst), 1);
        assert_eq!(actual_hook.returned.load(Ordering::SeqCst), 0);
        drop(pending); // actual callback entered but never returned
        context.record_publication(sealed, actual).unwrap();
        assert!(cleanup.finish_owned(&context).await.is_err());
    });
    let (tx, rx) = oneshot::channel();
    let _proxy = handoff.supervise(worker, Vec::new(), tx);
    assert_eq!(disposition(rx).await, OwnerDisposition::RetainedUnproved);
    assert_eq!(hook.cancelled.load(Ordering::SeqCst), 1);
    assert_eq!(
        original.observed_worker_join(),
        Some(ActualJoinObservation::Completed)
    );
    assert_eq!(hook.entered.load(Ordering::SeqCst), 1);
    assert_eq!(hook.returned.load(Ordering::SeqCst), 0);
    {
        let state = original.state.lock().unwrap();
        let actual = state.release.publication.as_ref().unwrap();
        assert!(matches!(
            &actual.terminal,
            OriginalStoreObservation::Returned(ack) if ack.is_some()
        ));
        assert!(matches!(
            &actual.diagnostic,
            OriginalStoreObservation::Returned(ack) if ack.is_some()
        ));
        assert_eq!(actual.cancellation_hook, OriginalHookObservation::Returned);
        assert_eq!(actual.event_hook, OriginalHookObservation::Entered);
        assert!(state.actual_retired.is_none());
    }
    assert_eq!(
        (
            health(&fixture).await.active,
            health(&fixture).await.released
        ),
        (1, 0)
    );
}

#[tokio::test]
async fn positive_worker_with_poisoned_retirement_lock_keeps_actual_lease_and_registration() {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    let cleanup = super::super::runtime_events::RunCleanupState::from_session(&fixture.session, "");
    let admission = fixture.session.run_admission.clone();
    let context = original.clone();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        actual_no_scope_constructors(&context);
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
        // Actual poisoned original lock after positive worker work, before guardian retirement.
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _actual = admission.stream_aborts.lock().unwrap();
            panic!("intentional real retirement-lock poison");
        }))
        .is_err());
    });
    let (tx, rx) = oneshot::channel();
    let proxy = handoff.supervise(worker, Vec::new(), tx);
    assert_eq!(disposition(rx).await, OwnerDisposition::RetainedUnproved);
    assert!(!proxy.is_finished());
    assert!(Arc::ptr_eq(
        &fixture.session.run_admission.current_original().unwrap(),
        &original
    ));
    assert!(original
        .state
        .lock()
        .unwrap()
        .worker_join
        .as_ref()
        .unwrap()
        .is_ok());
    assert!(original.state.lock().unwrap().actual_retired.is_none());
    assert_eq!(
        (
            health(&fixture).await.active,
            health(&fixture).await.released
        ),
        (1, 0)
    );
    assert!(fixture
        .session
        .run_admission
        .wait_for_original_release(&original, Duration::from_millis(1))
        .await
        .is_none());
}
