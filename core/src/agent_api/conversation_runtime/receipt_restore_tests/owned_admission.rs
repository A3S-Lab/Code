#![cfg(test)]

use super::*;
use run_admission::{ExecutionMode, OriginalScopeKind, OwnerDisposition};
use std::future::Future;
use std::sync::atomic::Ordering;
use std::time::Duration;

// The scenario runs inside the same start-gated original worker that reserves
// the Run, constructs its actual scope/inbox/committer and publishes failure.
// Passing a Session or a legacy PinnedFact into that worker would bypass the
// ownership boundary these fixtures are intended to exercise.
pub(super) async fn scenario<F, Fut, T>(
    session: &AgentSession,
    run_id: &'static str,
    action: F,
) -> (T, OwnerDisposition)
where
    F: FnOnce(OwnedPinnedFact, Option<crate::cognitive_context::CognitivePackageBindingV1>) -> Fut
        + Send
        + 'static,
    Fut: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let lease = ExecutionCoordinator::admit(session, "owned recovery fixture")
        .await
        .unwrap();
    let mut captured = OwnedConversationCapture::capture(session, false).unwrap();
    let cognitive = captured.preparation.cognitive_binding.clone();
    let cancellation = captured.preparation.cancellation.clone();
    let (handoff, start, original) = ExecutionCoordinator::prepare_original_handoff(
        session,
        lease,
        ExecutionMode::Detached,
        cancellation,
    )
    .unwrap();
    captured.controls = captured.controls.bind_original(original.clone()).unwrap();
    let context = original.clone();
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        context
            .record_scope_not_attempted(OriginalScopeKind::Command)
            .unwrap();
        let reserved = captured
            .controls
            .reserve_run_with_id(run_id, "before model recovery fixture")
            .await
            .unwrap();
        assert!(matches!(reserved, crate::run::RunReservation::Created(_)));
        let run = context.created_handle().unwrap();
        let pinned = prepare_owned_fact(
            captured,
            context,
            "before model recovery fixture",
            Some(run),
            None,
        )
        .await
        .unwrap();
        assert!(result_tx.send(action(pinned, cognitive).await).is_ok());
    });
    let (disposition_tx, disposition_rx) = tokio::sync::oneshot::channel();
    let proxy = handoff.supervise(worker, Vec::new(), disposition_tx);
    let result = tokio::time::timeout(Duration::from_secs(10), result_rx)
        .await
        .unwrap()
        .unwrap();
    let disposition = tokio::time::timeout(Duration::from_secs(5), disposition_rx)
        .await
        .unwrap()
        .unwrap();
    match disposition {
        OwnerDisposition::Released => {
            proxy.await.unwrap();
            assert!(session.run_admission.current_original().is_none());
            assert!(session.current_run_id.lock().await.is_none());
            assert!(session.cancel_token.lock().await.is_none());
            assert!(session.active_run_control.lock().await.is_none());
            assert_eq!(session.task_scheduler.health().await.unwrap().active, 0);
        }
        OwnerDisposition::RetainedUnproved => {
            assert!(!proxy.is_finished());
            assert!(Arc::ptr_eq(
                &original,
                &session.run_admission.current_original().unwrap()
            ));
            assert_eq!(session.task_scheduler.health().await.unwrap().active, 1);
            drop(proxy); // Observation loss does not release the original.
        }
        other => panic!("entered canonical worker has unexpected disposition: {other:?}"),
    }
    (result, disposition)
}

pub(super) async fn binding_identity() {
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(DetachedAdmissionClient::new(false));
    let session = detached_session(workspace.path(), client.clone(), true).await;
    let active = session.active_run_control.clone();
    let model = client.clone();
    let (_, disposition) = scenario(
        &session,
        "detached-binding",
        move |pinned, cognitive| async move {
            let surface = pinned.parts.surface.as_ref().unwrap();
            let expected = surface.agent.checkpoint_capability_binding().unwrap();
            assert_eq!(pinned.snapshot.capability_binding.as_ref(), Some(expected));
            assert_eq!(pinned.snapshot.cognitive_package_binding, cognitive);
            assert!(pinned.snapshot.cognitive_package_binding.is_some());
            assert_eq!(surface.run_id.as_deref(), Some(pinned.run.id()));
            assert_eq!(
                surface.agent.checkpoint_run_id.as_deref(),
                Some(pinned.run.id())
            );
            let checkpoint = surface.checkpoint.as_ref().unwrap();
            assert_eq!(checkpoint.run_id, pinned.run.id());
            assert_eq!(checkpoint.capability_binding.as_ref(), Some(expected));
            let inbox = surface.run_control.as_ref().unwrap();
            assert!(Arc::ptr_eq(inbox, active.lock().await.as_ref().unwrap()));
            let control = inbox.snapshot().await;
            assert_eq!(control.run_id, pinned.run.id());
            assert!(control.active && control.turn_id.is_some());
            assert_eq!(model.calls.load(Ordering::SeqCst), 0);
            fail_owned_prepared(pinned, CodeError::Session("binding test cleanup".into())).await
        },
    )
    .await;
    assert_eq!(disposition, OwnerDisposition::Released);
    assert_eq!(
        session
            .run_snapshot("detached-binding")
            .await
            .unwrap()
            .status,
        crate::run::RunStatus::Failed
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    session.close().await;
}

pub(super) async fn recovery_seed_failure() {
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(DetachedAdmissionClient::new(false));
    let session = detached_session(workspace.path(), client.clone(), false).await;
    let log_dir = crate::session_folder::effect_log_dir(workspace.path(), session.session_id());
    let fault_path = log_dir.join(format!(
        "{}.jsonl",
        crate::fact_control::thread_for_session(session.session_id())
    ));
    let restored_path = fault_path.clone();
    let closed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = closed.clone();
    let (_, disposition) = scenario(
        &session,
        "detached-seed-failure",
        move |pinned, _| async move {
            pinned
                .guard
                .run_scope()
                .register_effect(AdmissionCloseEffect {
                    closed: observed.clone(),
                    fail: false,
                })
                .unwrap();
            let inbox = pinned
                .parts
                .surface
                .as_ref()
                .unwrap()
                .run_control
                .clone()
                .unwrap();
            // A real FileLog I/O fault, including on privileged test hosts.
            assert!(!fault_path.exists());
            std::fs::create_dir(&fault_path).unwrap();
            let scope_cancel = pinned.guard.run_scope().cancellation();
            let primary = seed_exact_recovery_parts(&pinned.parts, &[Message::user("recover")])
                .unwrap_err()
                .to_string();
            let error = fail_owned_prepared(pinned, CodeError::Session(primary.clone())).await;
            assert!(error.to_string().contains(&primary));
            assert_eq!(observed.load(Ordering::SeqCst), 1);
            assert!(scope_cancel.is_cancelled());
            assert!(!inbox.snapshot().await.active);
        },
    )
    .await;
    assert_eq!(disposition, OwnerDisposition::Released);
    assert_eq!(closed.load(Ordering::SeqCst), 1);
    assert_eq!(
        session
            .run_snapshot("detached-seed-failure")
            .await
            .unwrap()
            .status,
        crate::run::RunStatus::Failed
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    std::fs::remove_dir(restored_path).unwrap();
    let AgentRunSpawn::Started { worker, .. } = session
        .spawn_run_with_id("after-seed-failure", "fresh turn")
        .await
        .unwrap()
    else {
        panic!("fresh Run replayed")
    };
    tokio::time::timeout(Duration::from_secs(10), worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        session
            .run_snapshot("after-seed-failure")
            .await
            .unwrap()
            .status,
        crate::run::RunStatus::Completed
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), 1);
    session.close().await;
}

pub(super) async fn binding_conflict_retains_failed_close() {
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(DetachedAdmissionClient::new(false));
    let session = detached_session(workspace.path(), client.clone(), false).await;
    let closed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = closed.clone();
    let (expected, disposition) = scenario(
        &session,
        "detached-binding-conflict",
        move |pinned, _| async move {
            let expected = pinned.snapshot.capability_binding.clone().unwrap();
            let mut other = serde_json::to_value(&expected).unwrap();
            other["catalogDigest"] = json!(format!("sha256:{}", "f".repeat(64)));
            let other: crate::capability::RunCapabilityBindingV1 =
                serde_json::from_value(other).unwrap();
            other.validate().unwrap();
            pinned
                .guard
                .run_scope()
                .register_effect(AdmissionCloseEffect {
                    closed: observed.clone(),
                    fail: true,
                })
                .unwrap();
            let primary = pinned
                .controls
                .bind_capability_generation(pinned.run.id(), other)
                .await
                .unwrap_err();
            let primary_text = primary.to_string();
            let error = fail_owned_prepared(pinned, primary).await;
            assert!(error.to_string().contains(&primary_text));
            assert!(error
                .to_string()
                .contains("Capability Run close was incomplete"));
            assert!(
                error.to_string().contains("child scopes failed: 1"),
                "{error}"
            );
            assert!(error
                .to_string()
                .contains("original cleanup remains unproved"));
            assert_eq!(observed.load(Ordering::SeqCst), 1);
            expected
        },
    )
    .await;
    assert_eq!(disposition, OwnerDisposition::RetainedUnproved);
    let snapshot = session
        .run_snapshot("detached-binding-conflict")
        .await
        .unwrap();
    assert_eq!(snapshot.status, crate::run::RunStatus::Failed);
    assert_eq!(snapshot.capability_binding, Some(expected));
    assert_eq!(
        session.current_run_id.lock().await.as_deref(),
        Some(snapshot.id.as_str())
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    assert!(matches!(
        session
            .spawn_run_with_id("after-incomplete-close", "must refuse")
            .await,
        Err(CodeError::SessionBusy { .. })
    ));
    let facts = crate::fact_control::read_workspace_facts(
        workspace.path(),
        &crate::fact_control::thread_for_session(session.session_id()),
    )
    .unwrap();
    assert!(!facts
        .iter()
        .any(|fact| fact.kind == "tool.prepared" || fact.kind == "tool.result"));
}
