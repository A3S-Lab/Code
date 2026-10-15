#![cfg(test)]

use super::*;

/// Exercise the foreground adapter directly: background admission is already
/// refused on macOS, but that capability does not guard `BashSandbox::exec`.
/// The host releases the write only after exec has returned exit code zero.
#[tokio::test]
async fn native_foreground_completion_contains_a_setsid_descendant() {
    assert!(
        std::path::Path::new("/usr/bin/python3").is_file(),
        "foreground setsid proof requires the actual /usr/bin/python3; no implicit skip"
    );
    let workspace = tempfile::tempdir().unwrap();
    let sandbox = crate::sandbox::native::NativeBashSandbox::new(workspace.path()).unwrap();
    let nonce = format!("foreground-escape-proof-{}", uuid::Uuid::new_v4());
    let cleanup = EscapedProcessCleanup {
        task: None,
        ready_path: workspace.path().join("foreground-escape-ready.json"),
        nonce: nonce.clone(),
    };
    let script = r#"import json, os, sys, time
original_pgid = os.getpgrp()
pid = os.fork()
if pid == 0:
    os.setsid()
    descriptor = os.open(os.devnull, os.O_RDWR)
    for stream in (0, 1, 2):
        os.dup2(descriptor, stream)
    if descriptor > 2:
        os.close(descriptor)
    identity = {"pid": os.getpid(), "pgid": os.getpgrp(), "sid": os.getsid(0), "original_pgid": original_pgid, "nonce": sys.argv[1]}
    with open("foreground-escape-ready.tmp", "w") as handle:
        json.dump(identity, handle)
    os.replace("foreground-escape-ready.tmp", "foreground-escape-ready.json")
    deadline = time.monotonic() + 30
    while not os.path.exists("foreground-escape-release") and time.monotonic() < deadline:
        time.sleep(0.02)
    if os.path.exists("foreground-escape-release"):
        time.sleep(0.25)
        with open("foreground-escape-late.txt", "w") as handle:
            handle.write(sys.argv[1])
    while time.monotonic() < deadline:
        time.sleep(0.1)
    os._exit(0)
deadline = time.monotonic() + 10
while not os.path.exists("foreground-escape-ready.json") and time.monotonic() < deadline:
    time.sleep(0.02)
if not os.path.exists("foreground-escape-ready.json"):
    raise RuntimeError("escaped descendant never published readiness")
print("foreground-parent-exited-after-ready", flush=True)
"#;
    let command = format!(
        "/usr/bin/python3 -c {} {}",
        shell_words::quote(script),
        shell_words::quote(&nonce)
    );
    let execution = tokio::time::timeout(
        Duration::from_secs(15),
        sandbox.exec(SandboxCommandRequest {
            command,
            guest_workspace: "/workspace".into(),
            timeout_ms: 12_000,
            output_observer: None,
            env: None,
        }),
    )
    .await;
    let identity = std::fs::read(&cleanup.ready_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    if cfg!(target_os = "macos") {
        let error = match execution {
            Ok(Err(error)) => error,
            _ => panic!("unqualified native foreground must refuse before execution"),
        };
        assert!(error
            .to_string()
            .contains("native_process_lifetime_unqualified"));
        assert!(identity.is_none());
        assert!(!workspace
            .path()
            .join("foreground-escape-ready.tmp")
            .exists());
        assert!(!workspace.path().join("foreground-escape-late.txt").exists());
        cleanup.terminate().unwrap();
        eprintln!("native_lifetime=refused_unqualified; no native containment-support claim");
        return;
    }
    let pid = identity
        .as_ref()
        .and_then(|identity| identity["pid"].as_i64())
        .and_then(|pid| i32::try_from(pid).ok())
        .filter(|pid| *pid > 1);
    let returned_successfully = matches!(
        &execution,
        Ok(Ok(output)) if output.exit_code == 0
            && !output.timed_out
            && output.stdout.contains("foreground-parent-exited-after-ready")
    );
    let execution_evidence = match &execution {
        Ok(Ok(output)) => serde_json::json!({
            "exit_code": output.exit_code,
            "timed_out": output.timed_out,
            "stdout": output.stdout,
            "stderr": output.stderr,
        }),
        Ok(Err(error)) => serde_json::json!({ "error": error.to_string() }),
        Err(error) => serde_json::json!({ "outer_timeout": error.to_string() }),
    };
    if !returned_successfully || pid.is_none() {
        let cleaned = cleanup.terminate();
        panic!(
            "foreground exec/readiness must succeed before containment can be inferred; exec={execution_evidence}; identity={identity:?}; final_cleanup={cleaned:?}"
        );
    }
    let pid = pid.unwrap();
    let identity = identity.unwrap();
    let alive_after_exec = process_is_running(pid);
    // Neither the child nor the parent can create this release before the
    // adapter has returned. A late write proves execution beyond that return.
    let released = std::fs::write(workspace.path().join("foreground-escape-release"), &nonce);
    let late_path = workspace.path().join("foreground-escape-late.txt");
    let wrote_after_exec = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if late_path.is_file() {
                break true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or(false);
    let late_bytes = std::fs::read_to_string(&late_path).ok();
    let cleaned = cleanup.terminate();
    let cleanup_settled = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match process_is_running(pid) {
                Ok(false) => break Ok(()),
                Ok(true) => tokio::time::sleep(Duration::from_millis(20)).await,
                Err(error) => break Err(error),
            }
        }
    })
    .await;
    eprintln!(
        "foreground setsid identity={identity}; exec={execution_evidence}; alive_after_exec={alive_after_exec:?}; late_file={wrote_after_exec}; late_bytes={late_bytes:?}; final_cleanup={cleaned:?}; cleanup_settled={cleanup_settled:?}"
    );
    cleaned.unwrap();
    cleanup_settled.unwrap().unwrap();
    released.unwrap();
    assert_eq!(identity["nonce"], nonce);
    assert_eq!(identity["pgid"], pid);
    assert_eq!(identity["sid"], pid);
    assert_ne!(identity["original_pgid"], pid);
    if wrote_after_exec {
        assert_eq!(late_bytes.as_deref(), Some(nonce.as_str()));
    }
    assert!(
        !alive_after_exec.unwrap() && !wrote_after_exec,
        "HIGH: setsid descendant survived successful foreground exec or wrote after the adapter returned"
    );
}
