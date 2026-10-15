#![cfg(test)]

use super::*;

fn admission(workspace: &std::path::Path, id: &str) -> Arc<RunAdmission> {
    Arc::new(RunAdmission::for_session(workspace, id))
}

#[test]
fn independent_instances_refuse_the_same_source_and_rollback_failed_admission() {
    let workspace = tempfile::tempdir().unwrap();
    let first = admission(workspace.path(), "same-session");
    let second = admission(workspace.path(), "same-session");
    let lease = first.try_acquire("same-session").unwrap();
    assert!(lease.log_owner.is_some());
    assert!(matches!(
        second.try_acquire("same-session"),
        Err(CodeError::SessionBusy { .. })
    ));
    assert!(!second.active.load(Ordering::Acquire));
    drop(lease);
    assert!(second.try_acquire("same-session").is_ok());
}

#[test]
fn separate_original_threads_and_workspaces_keep_their_original_independence() {
    let workspace = tempfile::tempdir().unwrap();
    let other_workspace = tempfile::tempdir().unwrap();
    let first = admission(workspace.path(), "first");
    let second = admission(workspace.path(), "second");
    let other = admission(other_workspace.path(), "first");
    let leases = (
        first.try_acquire("first").unwrap(),
        second.try_acquire("second").unwrap(),
        other.try_acquire("first").unwrap(),
    );
    drop(leases);
}

#[test]
fn a_foreign_admission_label_cannot_change_the_captured_source() {
    let workspace = tempfile::tempdir().unwrap();
    let original = admission(workspace.path(), "original");
    assert!(original.try_acquire("foreign").is_err());
    assert!(!workspace.path().join(".a3s/effect-log").exists());
    assert!(!original.active.load(Ordering::Acquire));
    assert!(original.try_acquire("original").is_ok());
}

#[test]
fn the_original_log_handle_is_reused_across_lease_retirement() {
    let workspace = tempfile::tempdir().unwrap();
    let original = admission(workspace.path(), "original");
    assert!(original.fact_log_slot().lock().unwrap().is_none());
    let lease = original.try_acquire("original").unwrap();
    let first = original
        .fact_log_slot()
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .clone();
    drop(lease);
    let next = original.try_acquire("original").unwrap();
    let second = original
        .fact_log_slot()
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .clone();
    assert!(Arc::ptr_eq(&first, &second));
    let writer = first.try_writer_fence().unwrap();
    assert!(writer.is_for(&second).unwrap());
    drop((writer, next));
}

#[tokio::test]
async fn losing_the_public_stream_proxy_retains_the_file_owner_until_actual_join() {
    let workspace = tempfile::tempdir().unwrap();
    let first = admission(workspace.path(), "same-session");
    let second = admission(workspace.path(), "same-session");
    let lease = first.try_acquire("same-session").unwrap();
    let (release, pending) = oneshot::channel();
    let worker = tokio::spawn(async {
        let _ = pending.await;
    });
    let abort = worker.abort_handle();
    let public = guard_stream_handle(worker, vec![abort], lease);
    public.abort();
    tokio::task::yield_now().await;
    assert!(matches!(
        second.try_acquire("same-session"),
        Err(CodeError::SessionBusy { .. })
    ));
    release.send(()).unwrap();
    assert!(first.wait_until_idle(Duration::from_secs(2)).await);
    assert!(second.try_acquire("same-session").is_ok());
}

fn child_probe(workspace: &std::path::Path, busy: bool) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "agent_api::run_admission::log_owner_tests::child_process_probe",
            "--ignored",
            "--nocapture",
        ])
        .env("A3S_CODE_OWNER_TEST_WORKSPACE", workspace)
        .env("A3S_CODE_OWNER_TEST_BUSY", busy.to_string())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn an_actual_child_process_uses_the_same_original_admission_boundary() {
    let workspace = tempfile::tempdir().unwrap();
    let original = admission(workspace.path(), "same-session");
    let lease = original.try_acquire("same-session").unwrap();
    child_probe(workspace.path(), true);
    drop(lease);
    child_probe(workspace.path(), false);
}

#[test]
#[ignore = "invoked by the bounded parent process test with an explicit temporary workspace"]
fn child_process_probe() {
    let workspace = std::env::var_os("A3S_CODE_OWNER_TEST_WORKSPACE").unwrap();
    let busy: bool = std::env::var("A3S_CODE_OWNER_TEST_BUSY")
        .unwrap()
        .parse()
        .unwrap();
    let original = admission(std::path::Path::new(&workspace), "same-session");
    let result = original.try_acquire("same-session");
    if busy {
        assert!(matches!(result, Err(CodeError::SessionBusy { .. })));
    } else {
        assert!(result.is_ok());
    }
}
