//! Real HVF guest settlement. Ignored so a normal lib run does not boot a VM.
//! Run it with `--ignored` only after the system image and signed shim exist.
//! A missing artifact or a guest that does not boot is a failure.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use crate::sandbox::{BashSandbox, SandboxCommandRequest};

use super::super::SessionOptions;
use super::install_default_local_sandbox;

#[tokio::test]
#[ignore = "boots a real HVF guest; run with --ignored"]
async fn admitted_commands_settle_inside_a_guest_pid_namespace() {
    let manifest = require_artifact("A3S_CODE_GUEST_SYSTEM_IMAGE");
    let shim = require_artifact("A3S_CODE_KRUN_SHIM");
    let image_sha256 = image_sha256(&manifest);
    println!("e2e image_sha256={image_sha256}");
    println!("e2e manifest={}", manifest.display());
    println!("e2e shim={}", shim.display());

    let parent = tempfile::tempdir().expect("e2e parent");
    let workspace = parent.path().join("workspace");
    std::fs::create_dir(&workspace).expect("workspace");
    set_private_dir(&workspace);
    let sibling = parent.path().join("sibling");
    std::fs::create_dir(&sibling).expect("sibling");
    std::fs::write(sibling.join("keep"), "keep").expect("sibling marker");
    let host_tmp_escape = std::env::temp_dir().join("a3s-guest-lifetime-host-escape");
    if host_tmp_escape.exists() {
        let _ = std::fs::remove_file(&host_tmp_escape);
    }

    let mut passed = 0u32;
    let failed = 0u32;

    let write = run(&workspace, "printf 'guest-wrote\\n' > host-visible.txt\ntouch ../a3s-guest-sibling-escape\ntouch /tmp/a3s-guest-lifetime-host-escape\nwget -T 5 -O net-escape http://example.com/ || true\nls /sys/class/net > net-ifaces || true\n", 120_000).await;
    match write {
        Ok(output) if output.exit_code == 0 && !output.timed_out => {
            let body =
                std::fs::read_to_string(workspace.join("host-visible.txt")).unwrap_or_default();
            let net = std::fs::read_to_string(workspace.join("net-escape")).unwrap_or_default();
            let ifaces = std::fs::read_to_string(workspace.join("net-ifaces")).unwrap_or_default();
            let sibling_escape = parent.path().join("a3s-guest-sibling-escape");
            let host_write_ok = body == "guest-wrote\n";
            let outside_ok = !sibling_escape.exists()
                && !host_tmp_escape.exists()
                && std::fs::read_to_string(sibling.join("keep"))
                    .ok()
                    .as_deref()
                    == Some("keep");
            let network_ok = !net.to_ascii_lowercase().contains("<html")
                && !net.contains("Example Domain")
                && !ifaces.lines().any(|line| {
                    let line = line.trim();
                    line.starts_with("eth") || line.starts_with("en")
                });
            println!(
                "e2e host-write file={} bytes={:?} exit={}",
                if host_write_ok { "present" } else { "mismatch" },
                body,
                output.exit_code
            );
            println!(
                "e2e outside-write sibling={} host-tmp={}",
                if outside_ok { "unchanged" } else { "changed" },
                if host_tmp_escape.exists() {
                    "present"
                } else {
                    "absent"
                }
            );
            println!(
                "e2e network payload={} ifaces={:?}",
                if network_ok { "denied" } else { "unexpected" },
                ifaces
            );
            if host_write_ok && outside_ok && network_ok {
                passed += 3;
            } else {
                panic!(
                    "e2e FAIL write/boundary stdout={} stderr={}",
                    output.stdout, output.stderr
                );
            }
        }
        Ok(output) => panic!(
            "e2e FAIL write exit={} timed_out={} stdout={} stderr={}",
            output.exit_code, output.timed_out, output.stdout, output.stderr
        ),
        Err(error) => panic!("e2e FAIL write guest did not start: {error:#}"),
    }

    let setsid = run(
        &workspace,
        "touch ready-setsid\nsetsid sh -c 'sh -c \"sleep 2; touch late-setsid\" &' &\nexit 0\n",
        120_000,
    )
    .await;
    match setsid {
        Ok(output) if output.exit_code == 0 && !output.timed_out => {
            let ready = workspace.join("ready-setsid");
            if !ready.is_file() {
                panic!("e2e FAIL setsid missing readiness stdout={}", output.stdout);
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
            let late = workspace.join("late-setsid").exists();
            let alive = generation_alive(&shim, &workspace);
            println!(
                "e2e setsid ready=present late={} descendant={} exit=0",
                if late { "present" } else { "absent" },
                if alive { "alive" } else { "not-alive" }
            );
            if late || alive {
                panic!(
                    "e2e FAIL setsid late={late} alive={alive} stdout={}",
                    output.stdout
                );
            }
            passed += 1;
        }
        Ok(output) => panic!(
            "e2e FAIL setsid exit={} timed_out={} stdout={} stderr={}",
            output.exit_code, output.timed_out, output.stdout, output.stderr
        ),
        Err(error) => panic!("e2e FAIL setsid: {error:#}"),
    }

    let timeout = settle_after_ready(
        &workspace,
        &shim,
        "touch ready-timeout\nsleep 300\ntouch late-timeout\n",
        120_000,
        Settle::Timeout,
    )
    .await;
    match timeout {
        Ok(()) => passed += 1,
        Err(error) => panic!("e2e FAIL timeout: {error}"),
    }

    let cancel = settle_after_ready(
        &workspace,
        &shim,
        "touch ready-cancel\nsleep 300\ntouch late-cancel\n",
        180_000,
        Settle::Cancel,
    )
    .await;
    match cancel {
        Ok(()) => passed += 1,
        Err(error) => panic!("e2e FAIL cancel: {error}"),
    }

    let indeterminate = settle_after_ready(
        &workspace,
        &shim,
        "touch ready-indeterminate\nsleep 300\ntouch late-indeterminate\n",
        180_000,
        Settle::Shutdown,
    )
    .await;
    match indeterminate {
        Ok(()) => passed += 1,
        Err(error) => panic!("e2e FAIL indeterminate: {error}"),
    }

    println!("e2e summary passed={passed} failed={failed} image_sha256={image_sha256}");
    assert_eq!(failed, 0);
}

enum Settle {
    Timeout,
    Cancel,
    Shutdown,
}

async fn settle_after_ready(
    workspace: &Path,
    shim: &Path,
    command: &str,
    timeout_ms: u64,
    kind: Settle,
) -> Result<(), String> {
    let (ready_name, late_name, label) = match kind {
        Settle::Timeout => ("ready-timeout", "late-timeout", "timeout"),
        Settle::Cancel => ("ready-cancel", "late-cancel", "cancel"),
        Settle::Shutdown => ("ready-indeterminate", "late-indeterminate", "indeterminate"),
    };
    let ready = workspace.join(ready_name);
    let late = workspace.join(late_name);
    let _ = std::fs::remove_file(&ready);
    let _ = std::fs::remove_file(&late);
    // One sandbox owns the in-flight guest. Shutdown must signal that same
    // instance; a second install would stop an empty sandbox.
    let sandbox = installed(workspace);
    let running = Arc::clone(&sandbox);
    let command = command.to_string();
    let workspace_buf = workspace.to_path_buf();
    let task = tokio::spawn(async move {
        running
            .exec(SandboxCommandRequest {
                command,
                guest_workspace: workspace_buf.display().to_string(),
                timeout_ms,
                output_observer: None,
                env: None,
            })
            .await
    });
    if !wait_for_file(&ready, Duration::from_secs(100)).await {
        task.abort();
        let _ = task.await;
        return Err(format!(
            "{label} readiness was not observed; missing readiness is a failure"
        ));
    }
    match kind {
        Settle::Timeout => {
            let result = task
                .await
                .map_err(|error| format!("{label} task failed: {error}"))?;
            let output = result.map_err(|error| format!("{label} returned an error: {error:#}"))?;
            tokio::time::sleep(Duration::from_secs(1)).await;
            let alive = generation_alive(shim, workspace);
            let late_exists = late.exists();
            println!(
                "e2e timeout ready=present late={} descendant={} timed_out={} exit={}",
                if late_exists { "present" } else { "absent" },
                if alive { "alive" } else { "not-alive" },
                output.timed_out,
                output.exit_code
            );
            if !output.timed_out || output.exit_code == 0 || late_exists || alive {
                return Err(format!(
                    "timeout was command success or left a descendant: exit={} timed_out={} late={late_exists} alive={alive} stdout={}",
                    output.exit_code, output.timed_out, output.stdout
                ));
            }
        }
        Settle::Cancel => {
            task.abort();
            let joined = task.await;
            tokio::time::sleep(Duration::from_secs(1)).await;
            let alive = generation_alive(shim, workspace);
            let late_exists = late.exists();
            let cancelled = match joined {
                Err(_) => true,
                Ok(Err(_)) => true,
                Ok(Ok(output)) => output.exit_code != 0 || output.timed_out,
            };
            println!(
                "e2e cancel ready=present late={} descendant={} cancelled={}",
                if late_exists { "present" } else { "absent" },
                if alive { "alive" } else { "not-alive" },
                cancelled
            );
            if !cancelled || late_exists || alive {
                return Err(format!(
                    "cancellation left a descendant or reported success: late={late_exists} alive={alive}"
                ));
            }
        }
        Settle::Shutdown => {
            sandbox.shutdown().await;
            let result = task
                .await
                .map_err(|error| format!("{label} task failed: {error}"))?;
            tokio::time::sleep(Duration::from_secs(1)).await;
            let alive = generation_alive(shim, workspace);
            let late_exists = late.exists();
            let indeterminate = match &result {
                Err(error) => {
                    let message = error.to_string();
                    println!("e2e indeterminate result=error detail={message}");
                    message.contains("indeterminate")
                }
                Ok(output) => {
                    println!(
                        "e2e indeterminate result=ok exit={} timed_out={}",
                        output.exit_code, output.timed_out
                    );
                    false
                }
            };
            println!(
                "e2e indeterminate late={} descendant={}",
                if late_exists { "present" } else { "absent" },
                if alive { "alive" } else { "not-alive" }
            );
            if !indeterminate || late_exists || alive {
                return Err(
                    "torn-down guest was recorded as command success or left a descendant".into(),
                );
            }
        }
    }
    Ok(())
}

fn installed(workspace: &Path) -> Arc<dyn BashSandbox> {
    let mut opts = SessionOptions::new();
    install_default_local_sandbox(workspace, &mut opts);
    opts.sandbox_handle
        .expect("install_default_local_sandbox installs a sandbox")
}

async fn run(
    workspace: &Path,
    command: &str,
    timeout_ms: u64,
) -> anyhow::Result<crate::sandbox::SandboxExecutionOutput> {
    let sandbox = installed(workspace);
    println!("e2e command={command:?} timeout_ms={timeout_ms}");
    sandbox
        .exec(SandboxCommandRequest {
            command: command.to_string(),
            guest_workspace: workspace.display().to_string(),
            timeout_ms,
            output_observer: None,
            env: None,
        })
        .await
}

async fn wait_for_file(path: &Path, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if path.is_file() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    path.is_file()
}

fn generation_alive(shim: &Path, workspace: &Path) -> bool {
    let Ok(output) = Command::new("/bin/ps")
        .args(["-ax", "-o", "command="])
        .output()
    else {
        return true;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let shim = shim.to_string_lossy().into_owned();
    let workspace = workspace.to_string_lossy().into_owned();
    text.lines()
        .any(|line| line.contains(&shim) && line.contains(&workspace))
}

fn require_artifact(key: &str) -> PathBuf {
    let value =
        std::env::var(key).unwrap_or_else(|_| panic!("{key} is required for the HVF guest e2e"));
    let path = PathBuf::from(&value);
    assert!(path.is_file(), "{key} does not name a file: {value}");
    path
}

fn image_sha256(manifest: &Path) -> String {
    let bytes = std::fs::read(manifest).expect("read system image manifest");
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).expect("parse system image manifest");
    value
        .get("image")
        .and_then(|image| image.get("sha256"))
        .and_then(|sha| sha.as_str())
        .unwrap_or("missing")
        .to_string()
}

fn set_private_dir(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .expect("workspace must be mode 0700 for the HVF share");
    }
}
