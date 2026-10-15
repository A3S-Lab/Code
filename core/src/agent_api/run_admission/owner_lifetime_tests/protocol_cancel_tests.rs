#![cfg(test)]

use super::*;
use crate::{
    AgentProtocolCommandV1, AgentProtocolHost, AgentProtocolHostError, AgentProtocolRunCancelV1,
    AgentProtocolRunIdentityV1, AgentProtocolRunStateV1,
};

async fn original_worker_barrier(pending_deadline: bool) {
    let fixture = fixture().await;
    let (handoff, start, original, _) = prepare(&fixture, ExecutionMode::Detached).await;
    let control =
        super::super::super::run_lifecycle::RunControlState::from_session(&fixture.session)
            .bind_original(original.clone())
            .unwrap();
    let run_id = "protocol-original-barrier";
    let cleanup = super::super::super::runtime_events::RunCleanupState::from_session(
        &fixture.session,
        run_id,
    );
    let store = fixture.session.run_store.clone();
    let context = original.clone();
    let (ready_tx, ready_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        let (run, _) = actual_owned_run(&control, &context, run_id).await;
        ready_tx.send(run).unwrap();
        // The canonical original worker is blocked here before its own
        // terminal publication. Only this exact worker can pass the barrier.
        release_rx.await.unwrap();
        actual_no_scope_constructors(&context);
        context.record_no_committer().unwrap();
        context.seal_original_controls().await.unwrap();
        actual_success_publication(&context, &store).await;
        cleanup.finish_owned(&context).await.unwrap();
    });
    let (disposition_tx, disposition_rx) = oneshot::channel();
    let proxy = handoff.supervise(worker, Vec::new(), disposition_tx);
    let run = ready_rx.await.unwrap();
    let scheduler = fixture.session.task_scheduler.clone();
    let session = Arc::new(fixture.session);
    let host =
        AgentProtocolHost::new(format!("sha256:{}", "c".repeat(64)), session.clone()).unwrap();
    let command = AgentProtocolCommandV1::Cancel {
        request: AgentProtocolRunCancelV1 {
            schema: AgentProtocolRunCancelV1::SCHEMA.into(),
            request_id: "original-cancel".into(),
            identity: AgentProtocolRunIdentityV1 {
                schema: AgentProtocolRunIdentityV1::SCHEMA.into(),
                protocol: crate::AGENT_PROTOCOL_V1.into(),
                agent_release_identity: host.agent_release_identity().into(),
                session_id: session.session_id().into(),
                run_id: run.id().into(),
            },
            reason: "cancel the original".into(),
        },
    };
    if pending_deadline {
        let error = host
            .execute(&command)
            .await
            .expect_err("original worker remains blocked");
        assert!(matches!(
            &error,
            AgentProtocolHostError::Code(crate::CodeError::Internal(_))
        ));
        assert_eq!(
            error.runtime_code(),
            "a3s.code.agent_protocol.run_cancellation_pending"
        );
        assert_eq!(error.code(), "INTERNAL_ERROR");
        assert!(original.original_cancel.is_cancelled());
        assert_eq!(run.status().await, Some(crate::RunStatus::Created));
        assert_eq!(scheduler.health().await.unwrap().active, 1);
        assert!(session.current_run().await.is_some());
        release_tx.send(()).unwrap();
    } else {
        let mut cancelling = Box::pin(host.execute(&command));
        assert!(futures::poll!(&mut cancelling).is_pending());
        tokio::select! {
            result = &mut cancelling => panic!("premature cancellation result: {result:?}"),
            _ = original.original_cancel.cancelled() => {},
        }
        assert_eq!(run.status().await, Some(crate::RunStatus::Created));
        assert_eq!(scheduler.health().await.unwrap().active, 1);
        assert!(futures::poll!(&mut cancelling).is_pending());
        release_tx.send(()).unwrap();
        let receipt = cancelling.await.unwrap();
        receipt.validate_for(&command).unwrap();
        assert_eq!(receipt.state, AgentProtocolRunStateV1::Cancelled);
        assert!(!receipt.replayed);
    }
    assert_eq!(
        disposition(disposition_rx).await,
        OwnerDisposition::Released
    );
    proxy.await.unwrap();
    assert_eq!(run.status().await, Some(crate::RunStatus::Cancelled));
    assert_eq!(scheduler.health().await.unwrap().active, 0);
    let replay = host.execute(&command).await.unwrap();
    assert!(replay.replayed);
    replay.validate_for(&command).unwrap();
    assert_eq!(replay.state, AgentProtocolRunStateV1::Cancelled);
    assert_eq!(fixture.model.0.load(Ordering::SeqCst), 0);
    session.close().await;
}

#[tokio::test]
async fn accepted_cancel_waits_for_actual_original_terminal_publication() {
    original_worker_barrier(false).await;
}

#[tokio::test]
async fn cancellation_deadline_keeps_original_owner_and_returns_no_terminal_receipt() {
    original_worker_barrier(true).await;
}
