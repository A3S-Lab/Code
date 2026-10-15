//! A shell that never starts is a known refusal. It must not book an
//! unresolved external operation, and it must not freeze a later tool.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use super::*;

struct SpawnRefusingSandbox;

#[async_trait]
impl crate::sandbox::BashSandbox for SpawnRefusingSandbox {
    async fn exec_command(
        &self,
        _command: &str,
        _guest_workspace: &str,
    ) -> anyhow::Result<crate::sandbox::SandboxOutput> {
        Err(crate::sandbox::process_host::process_host_spawn_refused(
            std::io::Error::new(std::io::ErrorKind::NotFound, "bash"),
        ))
    }

    async fn shutdown(&self) {}
}

struct RunningSandbox;

#[async_trait]
impl crate::sandbox::BashSandbox for RunningSandbox {
    async fn exec_command(
        &self,
        _command: &str,
        _guest_workspace: &str,
    ) -> anyhow::Result<crate::sandbox::SandboxOutput> {
        Err(anyhow::anyhow!(
            "capture process-host bash output: pipe closed after the child started"
        ))
    }

    async fn shutdown(&self) {}
}

#[tokio::test]
async fn a_shell_that_never_starts_does_not_block_a_later_tool() {
    let root = tempfile::tempdir().unwrap();
    let (tools, _) = executor_tools_with(root.path(), Some(Arc::new(SpawnRefusingSandbox)));
    let mut refused = request("bash-missing", json!({"command": "echo hi"}));
    refused.call.name = "bash".into();
    let prepared = prepare_tools(
        Arc::clone(&tools),
        refused,
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    let refused_result = execute_prepared(Arc::clone(&tools), prepared).await;
    assert_ne!(
        refused_result.outcome,
        ToolExecutionOutcome::Unknown,
        "a shell that was never spawned has no external effect to reconcile"
    );
    assert!(refused_result
        .effects
        .unresolved_external_operations
        .is_empty());
    assert!(!tools
        .ledger
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .has_uncertain_tool_invocations());

    let later = prepare_tools(
        Arc::clone(&tools),
        request("later-write", json!({"write": true})),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    let later_result = execute_prepared(Arc::clone(&tools), later).await;
    assert_eq!(later_result.outcome, ToolExecutionOutcome::Succeeded);
    assert_eq!(
        std::fs::read(root.path().join("changed.txt")).unwrap(),
        b"exact bytes"
    );
}

#[tokio::test]
async fn a_capture_failure_after_the_child_starts_stays_unknown() {
    let root = tempfile::tempdir().unwrap();
    let (tools, _) = executor_tools_with(root.path(), Some(Arc::new(RunningSandbox)));
    let mut started = request("bash-capture", json!({"command": "echo hi"}));
    started.call.name = "bash".into();
    let prepared = prepare_tools(
        Arc::clone(&tools),
        started,
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    let result = execute_prepared(tools, prepared).await;
    assert_eq!(result.outcome, ToolExecutionOutcome::Unknown);
    assert!(!result.effects.unresolved_external_operations.is_empty());
}
