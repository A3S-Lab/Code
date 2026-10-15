#![cfg(test)]

use super::*;

mod foreground;

/// A panic must not leave the adversarial child running. Before signalling,
/// verify both the test nonce in its argv and its newly-created process group.
struct EscapedProcessCleanup {
    task: Option<Arc<BackgroundTask>>,
    ready_path: std::path::PathBuf,
    nonce: String,
}

impl EscapedProcessCleanup {
    fn terminate(&self) -> std::io::Result<()> {
        if let Some(task) = &self.task {
            task.kill_switch.cancel();
        }
        let Ok(bytes) = std::fs::read(&self.ready_path) else {
            return Ok(());
        };
        let identity: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
        let pid = identity["pid"]
            .as_i64()
            .and_then(|pid| i32::try_from(pid).ok())
            .filter(|pid| *pid > 1)
            .ok_or_else(|| std::io::Error::other("escape test omitted its child PID"))?;
        if !process_is_running(pid)? {
            return Ok(());
        }
        let process = std::process::Command::new("/bin/ps")
            .args(["-ww", "-p", &pid.to_string(), "-o", "command="])
            .output()?;
        if !process.status.success() {
            return Ok(());
        }
        if !String::from_utf8_lossy(&process.stdout).contains(&self.nonce) {
            return Err(std::io::Error::other(
                "escape test refuses to signal a PID whose argv lost its unique nonce",
            ));
        }
        // The PID came from this test's readiness record and argv still
        // carries its unique nonce. Kill only its own new process group.
        let result = unsafe {
            if libc::getpgid(pid) == pid {
                libc::kill(-pid, libc::SIGKILL)
            } else {
                libc::kill(pid, libc::SIGKILL)
            }
        };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }
}

impl Drop for EscapedProcessCleanup {
    fn drop(&mut self) {
        if let Err(error) = self.terminate() {
            eprintln!("escape test final cleanup failed: {error}");
        }
    }
}

fn process_is_running(pid: i32) -> std::io::Result<bool> {
    // A zombie cannot execute the delayed write. Keep the independent PID
    // oracle, but distinguish that state from a live escaped process.
    let result = unsafe { libc::kill(pid, 0) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(false)
        } else {
            Err(error)
        };
    }
    let state = std::process::Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output()?;
    Ok(state.status.success()
        && !String::from_utf8_lossy(&state.stdout)
            .trim_start()
            .starts_with('Z'))
}

#[tokio::test]
async fn native_run_settlement_contains_a_setsid_descendant() {
    assert!(
        std::path::Path::new("/usr/bin/python3").is_file(),
        "setsid containment proof requires the actual /usr/bin/python3; no implicit skip"
    );
    let owned = TestRun::native();
    let tool = BackgroundTool::default();
    let nonce = format!("background-escape-proof-{}", uuid::Uuid::new_v4());
    let script = r#"import json, os, sys, time
original_pgid = os.getpgrp()
pid = os.fork()
if pid == 0:
    os.setsid()
    identity = {"pid": os.getpid(), "pgid": os.getpgrp(), "sid": os.getsid(0), "original_pgid": original_pgid, "nonce": sys.argv[1]}
    with open("escape-ready.tmp", "w") as handle:
        json.dump(identity, handle)
    os.replace("escape-ready.tmp", "escape-ready.json")
    print("escaped-child-ready", flush=True)
    deadline = time.monotonic() + 30
    while not os.path.exists("escape-release") and time.monotonic() < deadline:
        time.sleep(0.02)
    if os.path.exists("escape-release"):
        time.sleep(0.25)
        with open("escape-late.txt", "w") as handle:
            handle.write(sys.argv[1])
    while time.monotonic() < deadline:
        time.sleep(0.1)
    os._exit(0)
time.sleep(30)
"#;
    let command = format!(
        "/usr/bin/python3 -c {} {}",
        shell_words::quote(script),
        shell_words::quote(&nonce)
    );
    let started = start(&tool, &owned.ctx, "setsid-escape", &command).await;
    if !started.success {
        assert_native_start_is_unsupported(&owned, &tool, &started).await;
        assert!(!owned.workspace.path().join("escape-ready.json").exists());
        assert!(!owned.workspace.path().join("escape-ready.tmp").exists());
        assert!(!owned.workspace.path().join("escape-late.txt").exists());
        assert!(tool.runs.0.lock().unwrap().is_empty());
        eprintln!("setsid qualification=unsupported; native macOS background refused before task, fact, or process admission; no containment-support claim");
        return;
    }
    assert!(started.success, "{}", started.content);
    let task = task(&tool, &owned.ctx, "setsid-escape");
    let cleanup = EscapedProcessCleanup {
        task: Some(Arc::clone(&task)),
        ready_path: owned.workspace.path().join("escape-ready.json"),
        nonce: nonce.clone(),
    };
    let ready = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(bytes) = std::fs::read(&cleanup.ready_path) {
                if let Ok(identity) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    break identity;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    if ready.is_err() {
        cleanup.terminate().unwrap();
        let _ = owned.turn.close().await;
        let _ = owned.run.close().await;
        let _ = owned.session.close().await;
        panic!(
            "positive setsid/fork readiness never arrived; no containment result may be inferred"
        );
    }
    let identity = ready.unwrap();
    let pid = i32::try_from(identity["pid"].as_i64().unwrap()).unwrap();
    let turn_closed = owned.turn.close().await;
    let run_closed = tokio::time::timeout(Duration::from_secs(5), owned.run.close()).await;
    let alive_after_close = process_is_running(pid);
    // The host releases the write only AFTER Run.close returned. A late
    // file is therefore an independent proof of post-settlement execution.
    let released = std::fs::write(owned.workspace.path().join("escape-release"), &nonce);
    let late_path = owned.workspace.path().join("escape-late.txt");
    let wrote_after_close = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if late_path.is_file() {
                break true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or(false);
    let terminal = owned
        .facts()
        .into_iter()
        .find(|fact| fact.payload["event"] == "exited");
    let cleaned = cleanup.terminate();
    let session_closed = owned.session.close().await;
    eprintln!(
        "setsid identity={identity}; alive_after_run_close={alive_after_close:?}; late_file={wrote_after_close}; run_close={run_closed:?}; terminal={:?}; final_cleanup={cleaned:?}",
        terminal.as_ref().map(|fact| &fact.payload)
    );
    cleaned.unwrap();
    released.unwrap();
    assert_eq!(identity["nonce"], nonce);
    assert_eq!(identity["pgid"], pid);
    assert_eq!(identity["sid"], pid);
    assert_ne!(identity["original_pgid"], pid);
    assert!(turn_closed.unwrap().is_clean());
    assert!(run_closed.unwrap().unwrap().is_clean());
    assert!(session_closed.unwrap().is_clean());
    assert!(
        !alive_after_close.unwrap() && !wrote_after_close,
        "HIGH: setsid descendant survived Run.close or wrote after the terminal claimed settlement"
    );
}
