//! A3S native sandbox adapter.
//!
//! Isolation policy and platform enforcement live in the independent
//! `a3s-sandbox` crate. This module preserves the A3S Code `BashSandbox`
//! contract and translates output observation without duplicating security
//! behavior.

use super::{BashSandbox, SandboxCommandRequest, SandboxExecutionOutput, SandboxOutput};
use crate::workspace::{CommandOutputObserver, CommandOutputSummary};
use anyhow::Result;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use a3s_sandbox::NATIVE_SANDBOX_BACKEND;
pub(crate) use a3s_sandbox::{
    hard_link_count, hard_link_count_for_open_file, sensitive_paths,
    should_skip_workspace_scan_directory, workspace_hardlink_paths, workspace_sensitive_paths,
};

/// Fail-closed native sandbox implementation for the A3S Code bash contract.
#[derive(Debug)]
pub struct NativeBashSandbox {
    inner: a3s_sandbox::NativeSandbox,
}

impl NativeBashSandbox {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self {
            inner: a3s_sandbox::NativeSandbox::new(workspace)?,
        })
    }

    pub fn workspace(&self) -> &Path {
        self.inner.workspace()
    }

    pub fn backend(&self) -> &'static str {
        self.inner.backend()
    }

    pub async fn probe(&self) -> Result<()> {
        self.inner.probe().await
    }
}

struct OutputObserverAdapter {
    inner: Arc<dyn CommandOutputObserver>,
}

#[async_trait]
impl a3s_sandbox::OutputObserver for OutputObserverAdapter {
    async fn on_output_delta(&self, delta: &str) {
        self.inner.on_output_delta(delta).await;
    }

    async fn on_output_complete(&self, summary: &a3s_sandbox::OutputSummary) {
        self.inner
            .on_output_complete(&CommandOutputSummary {
                total_bytes: summary.total_bytes,
                captured_bytes: summary.captured_bytes,
                truncated: summary.truncated,
                timed_out: summary.timed_out,
            })
            .await;
    }
}

fn execution_output(output: a3s_sandbox::CommandOutput) -> SandboxExecutionOutput {
    SandboxExecutionOutput {
        stdout: output.stdout,
        stderr: output.stderr,
        exit_code: output.exit_code,
        timed_out: output.timed_out,
    }
}

#[async_trait]
impl BashSandbox for NativeBashSandbox {
    async fn exec_command(&self, command: &str, _guest_workspace: &str) -> Result<SandboxOutput> {
        let output = self.inner.exec_command(command).await?;
        Ok(SandboxOutput {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.exit_code,
        })
    }

    async fn exec(&self, request: SandboxCommandRequest) -> Result<SandboxExecutionOutput> {
        let output_observer: Option<Arc<dyn a3s_sandbox::OutputObserver>> = request
            .output_observer
            .map(|inner| Arc::new(OutputObserverAdapter { inner }) as Arc<_>);
        let output = self
            .inner
            .execute(a3s_sandbox::CommandRequest {
                command: request.command,
                timeout_ms: request.timeout_ms,
                output_observer,
                env: request.env,
            })
            .await?;
        Ok(execution_output(output))
    }

    async fn shutdown(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn adapter_preserves_native_backend_and_output() {
        let workspace = tempfile::tempdir().unwrap();
        let sandbox = NativeBashSandbox::new(workspace.path()).unwrap();

        assert_eq!(
            sandbox.workspace(),
            workspace.path().canonicalize().unwrap()
        );
        assert_eq!(sandbox.backend(), NATIVE_SANDBOX_BACKEND);
        sandbox.probe().await.unwrap();

        #[cfg(not(windows))]
        let command = "printf adapter-ready";
        #[cfg(windows)]
        let command = "[Console]::Out.Write('adapter-ready')";
        let output = sandbox.exec_command(command, "/workspace").await.unwrap();
        assert_eq!(output.stdout, "adapter-ready");
        assert_eq!(output.exit_code, 0);
    }
}
