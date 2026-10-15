#![cfg(test)]

use super::*;
use crate::agent::AgentEvent;
use crate::run_control::{RunControlReceiptState, SteerRequest};

async fn scenario(panics: bool, pending_steer: bool) {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Blocking).await;
    let capture = super::super::super::agent_loop_runtime::capture_owned_preparation(
        &fixture.session,
        original.original_cancel.clone(),
        false,
    )
    .unwrap();
    let control =
        super::super::super::run_lifecycle::RunControlState::from_session(&fixture.session)
            .bind_original(original.clone())
            .unwrap();
    let store = fixture.session.run_store.clone();
    let admission = fixture.session.run_admission.clone();
    let context = original.clone();
    let (ready, ready_rx) = oneshot::channel();
    let (fail, failed) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let run_id = format!("committer-{panics}-{pending_steer}");
    let owned_run_id = run_id.clone();
    let publisher =
        OriginalFailurePublisher::freeze(store.clone(), None, "original-t002".into(), None);
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        let install = context.reserve_committer_install().unwrap();
        let (started, started_rx) = oneshot::channel();
        let committer = tokio::spawn(async move {
            started.send(()).unwrap();
            failed.await.unwrap();
            assert!(!panics, "actual original committer panic");
            std::future::pending::<()>().await;
        });
        let committer_id = committer.id();
        let abort = committer.abort_handle();
        install.install(committer); // Same unique handle, before another await.
        started_rx.await.unwrap();
        let (run, inbox) = actual_owned_run(&control, &context, &owned_run_id).await;
        context
            .record_scope_not_attempted(OriginalScopeKind::Command)
            .unwrap();
        context
            .mark_scope_attempted(OriginalScopeKind::Fact)
            .unwrap();
        let super::super::super::agent_loop_runtime::OwnedPreparationCapture {
            fact,
            boundary,
            cancellation,
            workspace,
            ..
        } = capture;
        let (_, guard, _) =
            super::super::super::agent_loop_runtime::admit_captured_runtime_projection(
                fact,
                &boundary,
                cancellation,
                &workspace,
            )
            .await
            .unwrap();
        context
            .record_scope_returned(OriginalScopeKind::Fact, &guard)
            .unwrap();
        let actual_close = guard.close().await;
        assert!(actual_close.as_ref().unwrap().is_clean());
        context
            .record_scope_close(OriginalScopeKind::Fact, &actual_close)
            .unwrap();
        ready
            .send((run, inbox.clone(), abort, committer_id))
            .unwrap();
        released.await.unwrap();
        let join = context.wait_committer().await.unwrap();
        assert_eq!(
            join,
            ActualCommitterJoin::Joined(if panics {
                ActualJoinObservation::Panicked
            } else {
                ActualJoinObservation::Cancelled
            })
        );
        context.seal_original_controls().await.unwrap();
        let sealed = context.seal_publication().unwrap();
        let mut publication = OriginalPublicationObservation {
            kind: OriginalPublicationKind::Error,
            terminal: OriginalStoreObservation::Entered,
            diagnostic: OriginalStoreObservation::NotApplicable,
            cancellation_hook: OriginalHookObservation::NotApplicable,
            event_hook: OriginalHookObservation::NotApplicable,
        };
        context
            .record_publication_progress(&sealed, &publication)
            .unwrap();
        publication.terminal = OriginalStoreObservation::Returned(Box::new(
            store
                .record_event(
                    &owned_run_id,
                    AgentEvent::Error {
                        message: format!("actual original committer did not complete: {join:?}"),
                    },
                )
                .await,
        ));
        context.record_publication(sealed, publication).unwrap();
        // With no controls, every other pre-cleanup proof is positive. With
        // an Accepted Steer, only the same joined guardian may fail it later.
        assert!(context
            .retire_original_slots("original-t002", &admission, &owned_run_id)
            .await
            .is_err());
    });
    let (sent, observed) = oneshot::channel();
    let proxy = handoff
        .with_failure_publisher(publisher)
        .supervise(worker, Vec::new(), sent);
    let (run, inbox, abort, committer_id) = tokio::time::timeout(Duration::from_secs(5), ready_rx)
        .await
        .unwrap()
        .unwrap();
    let mut request =
        SteerRequest::new("  original unretained committer input\n").with_run_id(run.id());
    request.request_id = Some("original-committer-control".into());
    if pending_steer {
        assert_eq!(
            fixture.session.steer(request.clone()).await.unwrap().state,
            RunControlReceiptState::Accepted
        );
    }
    if panics {
        fail.send(()).unwrap();
    } else {
        abort.abort();
        drop(fail);
    }
    release.send(()).unwrap();
    assert_eq!(
        disposition(observed).await,
        OwnerDisposition::RetainedUnproved
    );
    assert_eq!(
        original.observed_worker_join(),
        Some(ActualJoinObservation::Completed)
    );
    {
        let state = original.committer.state.lock().await;
        let OriginalCommitterState::Joined(Err(error)) = &*state else {
            panic!("actual failed committer handle/result was lost: {state:?}");
        };
        assert_eq!(error.id(), committer_id);
        assert_eq!(error.is_panic(), panics);
        assert_eq!(error.is_cancelled(), !panics);
    }
    if pending_steer {
        let settled = fixture.session.steer(request.clone()).await.unwrap();
        assert_eq!(settled.state, RunControlReceiptState::Settled);
        assert_eq!(
            settled.error.as_ref().unwrap().code,
            "STEER_PERSISTENCE_FAILED"
        );
        assert_eq!(fixture.session.steer(request).await.unwrap(), settled);
    }
    assert!(!original
        .state
        .lock()
        .unwrap()
        .release
        .controls
        .unwrap()
        .has_accepted());
    assert!(!original.state.lock().unwrap().release.slots_retired);
    assert_eq!(
        fixture.session.current_run_id.lock().await.as_deref(),
        Some(run.id())
    );
    assert!(Arc::ptr_eq(
        fixture
            .session
            .active_run_control
            .lock()
            .await
            .as_ref()
            .unwrap(),
        &inbox
    ));
    assert!(Arc::ptr_eq(
        &fixture.session.run_admission.current_original().unwrap(),
        &original
    ));
    let events = fixture.session.run_store.events(run.id()).await;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, AgentEvent::Error { .. }))
            .count(),
        1
    );
    assert!(!events
        .iter()
        .any(|event| matches!(event.event, AgentEvent::End { .. })));
    assert_eq!(
        fixture
            .session
            .run_store
            .snapshot(run.id())
            .await
            .unwrap()
            .status,
        crate::run::RunStatus::Failed
    );
    assert!(
        !fixture
            .session
            .cancel_and_settle(Duration::from_millis(20), Duration::from_millis(20))
            .await
    );
    assert!(
        ExecutionCoordinator::admit(&fixture.session, "foreign replacement must refuse")
            .await
            .is_err()
    );
    let health = health(&fixture).await;
    assert_eq!((health.active, health.released), (1, 0));
    assert!(!proxy.is_finished());
    assert_eq!(fixture.model.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn actual_committer_errors_retain_original_capacity_and_fail_unretained_steer() {
    for (panics, pending_steer) in [(false, false), (false, true), (true, false), (true, true)] {
        tokio::time::timeout(Duration::from_secs(10), scenario(panics, pending_steer))
            .await
            .unwrap();
    }
}
