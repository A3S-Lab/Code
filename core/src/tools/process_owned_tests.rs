#![cfg(test)]
#![cfg(unix)]

use super::*;
use std::path::Path;
use std::time::Duration;

fn shell(root: &Path, script: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(script).current_dir(root);
    command
}

async fn wait_for(path: &Path) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the actual process must reach the test boundary");
}

async fn assert_no_later_write(root: &Path) {
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        !root.join("leaked").exists(),
        "a settled invocation left a live writer"
    );
}

#[tokio::test]
async fn output_capture_and_input_delivery_cannot_deadlock_each_other() {
    let root = tempfile::tempdir().unwrap();
    let mut command = shell(root.path(), "dd if=/dev/zero bs=1024 count=2048 2>/dev/null | tr '\\000' o; cat > input; printf END; printf diagnostic >&2");
    let input = vec![b'i'; 128 * 1024];
    let output = execute_owned_process(
        &mut command,
        &input,
        Duration::from_secs(3),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(output.termination, OwnedProcessTermination::Completed);
    assert!(output.status.unwrap().success());
    assert_eq!(std::fs::read(root.path().join("input")).unwrap(), input);
    assert!(output.stdout.starts_with("oooo"));
    assert!(output.stdout.ends_with("END"));
    assert!(output.stdout.contains("truncated"));
    assert!(output.stdout.len() < MAX_OUTPUT_SIZE + 512);
    assert_eq!(output.stderr, "diagnostic");
}

#[tokio::test]
async fn cancellation_before_admission_does_not_start_a_process() {
    let root = tempfile::tempdir().unwrap();
    let mut command = shell(root.path(), "touch leaked");
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let output = execute_owned_process(&mut command, b"", Duration::from_secs(1), &cancellation)
        .await
        .unwrap();
    assert_eq!(output.termination, OwnedProcessTermination::Cancelled);
    assert!(output.status.is_none());
    assert!(!root.path().join("leaked").exists());
}

#[tokio::test]
async fn deadline_includes_blocked_stdin_and_closed_output_streams() {
    let root = tempfile::tempdir().unwrap();
    let mut command = shell(
        root.path(),
        "exec 1>&- 2>&-; touch started; sleep 0.40; touch leaked",
    );
    let output = execute_owned_process(
        &mut command,
        &vec![b'i'; 128 * 1024],
        Duration::from_millis(100),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(root.path().join("started").exists());
    assert_eq!(output.termination, OwnedProcessTermination::TimedOut);
    assert_no_later_write(root.path()).await;
}

#[tokio::test]
async fn cancellation_settles_a_child_and_its_background_writer() {
    let root = tempfile::tempdir().unwrap();
    let cancellation = CancellationToken::new();
    let owned_cancellation = cancellation.clone();
    let mut command = shell(
        root.path(),
        "cat >/dev/null; (touch started; sleep 0.40; touch leaked) & wait",
    );
    let invocation = tokio::spawn(async move {
        execute_owned_process(
            &mut command,
            b"{}",
            Duration::from_secs(3),
            &owned_cancellation,
        )
        .await
    });
    wait_for(&root.path().join("started")).await;
    cancellation.cancel();
    let output = invocation.await.unwrap().unwrap();
    assert_eq!(output.termination, OwnedProcessTermination::Cancelled);
    assert_no_later_write(root.path()).await;
}

#[tokio::test]
async fn dropping_an_invocation_cannot_leave_its_background_writer() {
    let root = tempfile::tempdir().unwrap();
    let mut command = shell(
        root.path(),
        "cat >/dev/null; (touch started; sleep 0.40; touch leaked) & wait",
    );
    let invocation = tokio::spawn(async move {
        execute_owned_process(
            &mut command,
            b"{}",
            Duration::from_secs(3),
            &CancellationToken::new(),
        )
        .await
    });
    wait_for(&root.path().join("started")).await;
    invocation.abort();
    assert!(invocation.await.unwrap_err().is_cancelled());
    assert_no_later_write(root.path()).await;
}

#[tokio::test]
async fn successful_exit_also_retires_background_group_members() {
    let root = tempfile::tempdir().unwrap();
    let mut command = shell(root.path(), "cat >/dev/null; (exec 1>&- 2>&-; touch started; sleep 0.40; touch leaked) & while [ ! -f started ]; do :; done; printf done");
    let output = execute_owned_process(
        &mut command,
        b"{}",
        Duration::from_secs(2),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(output.termination, OwnedProcessTermination::Completed);
    assert!(output.status.unwrap().success());
    assert_eq!(output.stdout, "done");
    assert_no_later_write(root.path()).await;
}

#[tokio::test]
async fn input_failure_still_stops_the_process() {
    let root = tempfile::tempdir().unwrap();
    let mut command = shell(
        root.path(),
        "exec 0<&-; touch started; sleep 0.40; touch leaked",
    );
    let error = execute_owned_process(
        &mut command,
        &vec![b'i'; 128 * 1024],
        Duration::from_secs(2),
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert_no_later_write(root.path()).await;
}

#[tokio::test]
async fn nonzero_exit_preserves_its_own_error_stream() {
    let root = tempfile::tempdir().unwrap();
    let mut command = shell(
        root.path(),
        "cat >/dev/null; printf result; printf failure >&2; exit 7",
    );
    let output = execute_owned_process(
        &mut command,
        b"{}",
        Duration::from_secs(2),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(output.termination, OwnedProcessTermination::Completed);
    assert_eq!(output.status.unwrap().code(), Some(7));
    assert_eq!(output.stdout, "result");
    assert_eq!(output.stderr, "failure");
}
