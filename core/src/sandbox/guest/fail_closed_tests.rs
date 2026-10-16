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
