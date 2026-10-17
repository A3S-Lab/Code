use super::*;

#[test]
fn lease_rejects_overlap_and_releases_on_drop() {
    let admission = Arc::new(RunAdmission::default());
    let lease = admission.try_acquire("session-1").unwrap();

    assert!(matches!(
        admission.try_acquire("session-1"),
        Err(CodeError::SessionBusy { ref session_id }) if session_id == "session-1"
    ));

    drop(lease);
    assert!(admission.try_acquire("session-1").is_ok());
}

#[tokio::test]
async fn stream_guard_survives_public_handle_abort() {
    let admission = Arc::new(RunAdmission::default());
    let lease = admission.try_acquire("session-1").unwrap();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let inner = tokio::spawn(async move {
        let _ = release_rx.await;
    });
    let inner_abort = inner.abort_handle();

    let public_handle = guard_stream_handle(inner, vec![inner_abort], lease);
    public_handle.abort();
    tokio::task::yield_now().await;

    assert!(matches!(
        admission.try_acquire("session-1"),
        Err(CodeError::SessionBusy { .. })
    ));

    let _ = release_tx.send(());
    let mut released = None;
    for _ in 0..50 {
        match admission.try_acquire("session-1") {
            Ok(lease) => {
                released = Some(lease);
                break;
            }
            Err(CodeError::SessionBusy { .. }) => tokio::task::yield_now().await,
            Err(other) => panic!("unexpected admission error: {other:?}"),
        }
    }
    assert!(
        released.is_some(),
        "guardian must release after inner cleanup"
    );
}

#[tokio::test]
async fn registered_stream_worker_can_be_force_settled() {
    let admission = Arc::new(RunAdmission::default());
    let lease = admission.try_acquire("session-1").unwrap();
    let worker = tokio::spawn(std::future::pending::<()>());
    let worker_abort = worker.abort_handle();
    let (cleanup_tx, cleanup_rx) = tokio::sync::oneshot::channel();
    let supervisor = tokio::spawn(async move {
        let error = worker.await.expect_err("worker must be aborted");
        assert!(error.is_cancelled());
        let _ = cleanup_tx.send(());
    });
    let proxy = guard_stream_handle(supervisor, vec![worker_abort], lease);

    assert!(admission.abort_stream_workers());
    cleanup_rx
        .await
        .expect("the lifecycle supervisor must survive worker abortion");
    assert!(admission.wait_until_idle(Duration::from_secs(1)).await);
    assert!(proxy.await.is_ok());
    assert!(admission.try_acquire("session-1").is_ok());
}
