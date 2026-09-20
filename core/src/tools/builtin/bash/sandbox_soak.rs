//! S-SB-01: repeated sandboxed bash stays inside the sandbox, and sandbox-init
//! failures do not fall back to a host command.

use super::super::*;
use crate::sandbox::{BashSandbox, SandboxOutput};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const ECHOES: usize = 100;
const INIT_FAILURES: usize = 20;

struct CountingSandbox {
    calls: AtomicUsize,
}

#[async_trait]
impl BashSandbox for CountingSandbox {
    async fn exec_command(
        &self,
        _command: &str,
        _guest_workspace: &str,
    ) -> anyhow::Result<SandboxOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(SandboxOutput {
            stdout: "SB-OK\n".into(),
            stderr: String::new(),
            exit_code: 0,
        })
    }

    async fn shutdown(&self) {}
}

struct UnavailableSandbox;

#[async_trait]
impl BashSandbox for UnavailableSandbox {
    async fn exec_command(
        &self,
        _command: &str,
        _guest_workspace: &str,
    ) -> anyhow::Result<SandboxOutput> {
        anyhow::bail!(
            "the default A3S native sandbox is unavailable for this workspace: init failed"
        )
    }

    async fn shutdown(&self) {}
}

#[tokio::test]
#[ignore = "S-SB-01 sandboxed bash soak; run with --ignored"]
async fn soak_sandboxed_bash_does_not_fall_back_to_the_host() {
    let workspace = tempfile::tempdir().unwrap();
    let marker = workspace.path().join("host-leak.txt");
    let command = "echo HOST-LEAK > host-leak.txt";
    let sandbox = Arc::new(CountingSandbox {
        calls: AtomicUsize::new(0),
    });
    let tool = BashTool;
    let context = ToolContext::new(workspace.path().to_path_buf())
        .with_sandbox(Arc::clone(&sandbox) as Arc<dyn BashSandbox>);

    for index in 0..ECHOES {
        let result = tool
            .execute(&serde_json::json!({ "command": command }), &context)
            .await
            .expect("bash returns a tool result");
        assert!(
            result.success,
            "cycle {index} left the sandbox: {}",
            result.content
        );
        assert_eq!(
            result
                .metadata
                .as_ref()
                .and_then(|meta| meta.get("sandboxed")),
            Some(&serde_json::json!(true)),
            "cycle {index} was not marked sandboxed"
        );
    }
    assert_eq!(sandbox.calls.load(Ordering::SeqCst), ECHOES);
    assert!(
        !marker.exists(),
        "a sandboxed echo must not create the host marker"
    );

    let failing = Arc::new(UnavailableSandbox);
    let failing_context = ToolContext::new(workspace.path().to_path_buf())
        .with_sandbox(failing as Arc<dyn BashSandbox>);
    for index in 0..INIT_FAILURES {
        let error = tool
            .execute(&serde_json::json!({ "command": command }), &failing_context)
            .await
            .expect_err("sandbox init failure must not return a host result");
        let message = error.to_string();
        assert!(
            message.contains("unavailable"),
            "cycle {index} did not fail closed: {message}"
        );
        assert!(!marker.exists(), "init failure wrote the host marker");
    }
    assert_eq!(
        sandbox.calls.load(Ordering::SeqCst),
        ECHOES,
        "init failures must not be replayed through the working sandbox"
    );
}
