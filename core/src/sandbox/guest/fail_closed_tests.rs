#![cfg(test)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use super::super::{BashSandbox, SandboxCommandRequest};
use super::{GuestLifetimeSandbox, GUEST_COMMAND_SCHEMA};
use crate::sandbox::native::{
    is_native_process_lifetime_unqualified, NATIVE_PROCESS_LIFETIME_UNQUALIFIED,
};
use crate::workspace::{CommandOutputObserver, CommandOutputSummary};

struct Observer(AtomicUsize);

#[test]
fn guest_control_script_never_overwrites_existing_entries() {
    let root = tempfile::tempdir().unwrap();
    let outside = root.path().join("keep");
    std::fs::write(&outside, "keep").unwrap();
    for kind in ["regular", "symlink", "hardlink"] {
        let workspace = root.path().join(kind);
        std::fs::create_dir(&workspace).unwrap();
        let script = workspace.join(super::GUEST_SCRIPT_NAME);
        match kind {
            "regular" => std::fs::write(&script, "keep").unwrap(),
            "symlink" => std::os::unix::fs::symlink(&outside, &script).unwrap(),
            "hardlink" => std::fs::hard_link(&outside, &script).unwrap(),
            _ => unreachable!(),
        }
        assert!(
            super::write_guest_script(&script, "overwrite", None).is_err(),
            "the {kind} entry must refuse the command before its script is written"
        );
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep");
        assert_eq!(std::fs::read_to_string(&script).unwrap(), "keep");
    }
}

#[async_trait]
impl CommandOutputObserver for Observer {
    async fn on_output_delta(&self, _delta: &str) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_output_complete(&self, _summary: &CommandOutputSummary) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn executable_script(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).expect("write fake shim");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .expect("mark fake shim executable");
    }
    path
}

#[tokio::test]
async fn expired_timeout_does_not_start_a_command() {
    let root = tempfile::tempdir().unwrap();
    let manifest = root.path().join("image.json");
    std::fs::write(&manifest, "{}").unwrap();
    let shim = executable_script(root.path(), "shim", "#!/bin/sh\nexit 2\n");
    let workspace = tempfile::tempdir().unwrap();
    let sandbox = GuestLifetimeSandbox::from_artifacts(workspace.path(), &manifest, &shim)
        .expect("artifacts exist");
    let sentinel = workspace.path().join("sentinel");
    let observer = Arc::new(Observer(AtomicUsize::new(0)));
    let output = sandbox
        .exec(SandboxCommandRequest {
            command: "touch sentinel".into(),
            guest_workspace: workspace.path().display().to_string(),
            timeout_ms: 0,
            output_observer: Some(observer.clone()),
            env: None,
        })
        .await;
    match output {
        Ok(output) => {
            assert!(output.timed_out);
            assert_eq!(output.exit_code, 124);
        }
        Err(error) => assert!(is_native_process_lifetime_unqualified(&error), "{error}"),
    }
    assert!(!sentinel.exists());
    assert!(!workspace.path().join(".a3s-guest-command").exists());
}

#[tokio::test]
async fn a_shim_that_does_not_enter_is_not_command_success() {
    let root = tempfile::tempdir().unwrap();
    let manifest = root.path().join("image.json");
    std::fs::write(&manifest, "{}").unwrap();
    let shim = executable_script(
        root.path(),
        "shim",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' '{{\"schema_version\":\"{GUEST_COMMAND_SCHEMA}\",\"settlement\":\"settled\",\"exit_code\":0,\"vm_entered\":false}}'\n"
        ),
    );
    let workspace = tempfile::tempdir().unwrap();
    let sandbox = GuestLifetimeSandbox::from_artifacts(workspace.path(), &manifest, &shim)
        .expect("artifacts exist");
    let observer = Arc::new(Observer(AtomicUsize::new(0)));
    let error = sandbox
        .exec(SandboxCommandRequest {
            command: "touch sentinel".into(),
            guest_workspace: "/workspace".into(),
            timeout_ms: 5_000,
            output_observer: Some(observer.clone()),
            env: None,
        })
        .await;
    let Err(error) = error else {
        panic!("vm_entered false must not be command success");
    };
    let message = error.to_string();
    assert!(
        message.contains("indeterminate") || message.contains(NATIVE_PROCESS_LIFETIME_UNQUALIFIED),
        "{message}"
    );
    assert_eq!(observer.0.load(Ordering::SeqCst), 0);
    assert!(!workspace.path().join("sentinel").exists());
}

#[tokio::test]
async fn a_settled_guest_exit_is_command_success() {
    let root = tempfile::tempdir().unwrap();
    let manifest = root.path().join("image.json");
    std::fs::write(&manifest, "{}").unwrap();
    let shim = executable_script(
        root.path(),
        "shim",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' '{{\"schema_version\":\"{GUEST_COMMAND_SCHEMA}\",\"settlement\":\"settled\",\"exit_code\":0,\"vm_entered\":true}}'\n"
        ),
    );
    let workspace = tempfile::tempdir().unwrap();
    let sandbox = GuestLifetimeSandbox::from_artifacts(workspace.path(), &manifest, &shim)
        .expect("artifacts exist");
    let observer = Arc::new(Observer(AtomicUsize::new(0)));
    let output = sandbox
        .exec(SandboxCommandRequest {
            command: "printf ok".into(),
            guest_workspace: "/workspace".into(),
            timeout_ms: 5_000,
            output_observer: Some(observer.clone()),
            env: None,
        })
        .await;
    let output = match output {
        Ok(output) => output,
        Err(error) => panic!("a settled entered exit must be command success: {error:#}"),
    };
    assert_eq!(output.exit_code, 0);
    assert!(!output.timed_out);
    assert!(observer.0.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn missing_probe_artifacts_use_the_stable_refusal() {
    let previous_image = std::env::var_os(super::GUEST_SYSTEM_IMAGE_ENV);
    let previous_shim = std::env::var_os(super::GUEST_KRUN_SHIM_ENV);
    std::env::remove_var(super::GUEST_SYSTEM_IMAGE_ENV);
    std::env::remove_var(super::GUEST_KRUN_SHIM_ENV);
    let workspace = tempfile::tempdir().unwrap();
    let Err(error) = GuestLifetimeSandbox::discover(workspace.path()) else {
        panic!("probe must fail");
    };
    restore_env(super::GUEST_SYSTEM_IMAGE_ENV, previous_image);
    restore_env(super::GUEST_KRUN_SHIM_ENV, previous_shim);
    assert_eq!(error.to_string(), NATIVE_PROCESS_LIFETIME_UNQUALIFIED);
    assert!(is_native_process_lifetime_unqualified(&error));
}

fn restore_env(key: &str, previous: Option<std::ffi::OsString>) {
    match previous {
        Some(value) => std::env::set_var(key, value),
        None => std::env::remove_var(key),
    }
}

fn pid_alive(pid: u32) -> bool {
    let rc = unsafe { libc::kill(pid as i32, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Dropping an in-flight `exec` is what `tokio::time::timeout` does in the
/// bash tool. That drop kills one generation. The same sandbox must still
/// run the next command, and `shutdown` stays sticky after that.
#[tokio::test]
async fn dropped_exec_cancels_one_generation_and_the_next_command_runs() {
    let root = tempfile::tempdir().unwrap();
    let manifest = root.path().join("image.json");
    std::fs::write(&manifest, "{}").unwrap();
    let settled = format!(
        "{{\"schema_version\":\"{GUEST_COMMAND_SCHEMA}\",\"settlement\":\"settled\",\"exit_code\":17,\"vm_entered\":true}}"
    );
    let shim = executable_script(
        root.path(),
        "shim",
        &format!(
            r#"#!/bin/sh
share=
while [ $# -gt 0 ]; do
  if [ "$1" = "--runtime-share" ]; then
    share=$2
    shift 2
  else
    shift
  fi
done
if [ -z "$share" ]; then
  exit 2
fi
script="$share/.a3s-guest-command"
if grep -q generation-hold "$script"; then
  echo $$ > "$share/hold.pid"
  (sleep 0.6; touch "$share/late") &
  touch "$share/ready"
  sleep 30
  exit 0
fi
if grep -q generation-next "$script"; then
  touch "$share/second-ran"
  printf '%s\n' '{settled}'
  exit 0
fi
touch "$share/after-shutdown-ran"
printf '%s\n' '{settled}'
exit 0
"#,
            settled = settled
        ),
    );
    let workspace = tempfile::tempdir().unwrap();
    let sandbox = Arc::new(
        GuestLifetimeSandbox::from_artifacts(workspace.path(), &manifest, &shim)
            .expect("artifacts exist"),
    );
    let ready = workspace.path().join("ready");
    let late = workspace.path().join("late");
    let running = Arc::clone(&sandbox);
    let guest_workspace = workspace.path().display().to_string();
    // JoinHandle drop detaches the task. abort drops the exec future, which
    // is what tokio::time::timeout does to sandbox.exec in the bash tool.
    let hold = tokio::spawn(async move {
        running
            .exec(SandboxCommandRequest {
                command: "generation-hold".into(),
                guest_workspace,
                timeout_ms: 30_000,
                output_observer: None,
                env: None,
            })
            .await
    });
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if ready.exists() {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("readiness was not observed; missing readiness is a failure");
        }
        if hold.is_finished() {
            panic!("hold command finished before cancel");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let pid_text = std::fs::read_to_string(workspace.path().join("hold.pid")).expect("hold pid");
    let pid: u32 = pid_text.trim().parse().expect("hold pid integer");
    assert!(pid_alive(pid), "hold generation was not running");
    hold.abort();
    match hold.await {
        Err(error) if error.is_cancelled() => {}
        Err(error) => panic!("hold task ended without cancellation: {error}"),
        Ok(Ok(output)) => panic!("hold command returned exit {}", output.exit_code),
        Ok(Err(error)) => panic!("hold command failed before cancel: {error:#}"),
    }
    assert!(!pid_alive(pid), "dropped exec left the generation alive");
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    assert!(!late.exists(), "dropped exec performed a late write");

    let observer = Arc::new(Observer(AtomicUsize::new(0)));
    let output = sandbox
        .exec(SandboxCommandRequest {
            command: "generation-next".into(),
            guest_workspace: workspace.path().display().to_string(),
            timeout_ms: 5_000,
            output_observer: Some(observer.clone()),
            env: None,
        })
        .await;
    let output = match output {
        Ok(output) => output,
        Err(error) => panic!("the next command on the same sandbox must run: {error:#}"),
    };
    assert_eq!(output.exit_code, 17);
    assert!(!output.timed_out);
    assert!(workspace.path().join("second-ran").exists());
    assert!(observer.0.load(Ordering::SeqCst) > 0);

    sandbox.shutdown().await;
    let after = sandbox
        .exec(SandboxCommandRequest {
            command: "generation-after-shutdown".into(),
            guest_workspace: workspace.path().display().to_string(),
            timeout_ms: 5_000,
            output_observer: None,
            env: None,
        })
        .await;
    let Err(error) = after else {
        panic!("shutdown must stay sticky");
    };
    let message = error.to_string();
    assert!(message.contains("sandbox is shut down"), "{message}");
    assert!(!workspace.path().join("after-shutdown-ran").exists());
}
