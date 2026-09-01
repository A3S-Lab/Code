//! Sandbox integration for bash tool execution.
//!
//! When a [`BashSandbox`] is provided via
//! [`ToolContext::with_sandbox`](crate::tools::ToolContext::with_sandbox), the
//! `bash` built-in tool routes commands through that sandbox instead of
//! `std::process::Command`. The workspace directory is mounted read-write
//! at `/workspace` inside the sandbox.
//!
//! [`native::NativeBashSandbox`] is the A3S-owned fail-closed implementation
//! used by the CLI. Hosts can still supply another implementation through the
//! trait contract when they own an equivalent isolation boundary.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::workspace::CommandOutputObserver;

pub mod native;

pub use a3s_sandbox::{
    is_protected_workspace_path, PROTECTED_WORKSPACE_DIRECTORIES, PROTECTED_WORKSPACE_FILES,
};

/// Output from running a command inside a sandbox.
pub struct SandboxOutput {
    /// Standard output bytes decoded as UTF-8.
    pub stdout: String,
    /// Standard error bytes decoded as UTF-8.
    pub stderr: String,
    /// Process exit code (0 = success).
    pub exit_code: i32,
}

/// Complete request passed to sandbox implementations that support the
/// execution controls used by the built-in `bash` tool.
///
/// The legacy [`BashSandbox::exec_command`] method remains the minimum
/// compatibility contract. New implementations should override
/// [`BashSandbox::exec`] so command timeouts, streaming output, and explicit
/// host-provided environment values are preserved inside the sandbox.
#[derive(Clone)]
pub struct SandboxCommandRequest {
    pub command: String,
    pub guest_workspace: String,
    pub timeout_ms: u64,
    pub output_observer: Option<Arc<dyn CommandOutputObserver>>,
    pub env: Option<Arc<HashMap<String, String>>>,
}

impl std::fmt::Debug for SandboxCommandRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxCommandRequest")
            .field("command", &self.command)
            .field("guest_workspace", &self.guest_workspace)
            .field("timeout_ms", &self.timeout_ms)
            .field("output_observer", &self.output_observer.is_some())
            .field("env", &self.env.as_ref().map(|env| env.len()))
            .finish()
    }
}

/// Output from the extended sandbox execution contract.
pub struct SandboxExecutionOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
}

impl From<SandboxOutput> for SandboxExecutionOutput {
    fn from(output: SandboxOutput) -> Self {
        Self {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.exit_code,
            timed_out: false,
        }
    }
}

// ============================================================================
// BashSandbox trait
// ============================================================================

/// Abstraction over sandbox bash execution used by the `bash` built-in tool.
///
/// Implement this trait to provide a custom sandbox backend. The host
/// application constructs the implementation and passes it to the session
/// via [`ToolContext::with_sandbox`](crate::tools::ToolContext::with_sandbox).
#[async_trait]
pub trait BashSandbox: Send + Sync {
    /// Execute a shell command inside the sandbox.
    ///
    /// * `command` — the shell command string (passed as `bash -c <command>`).
    /// * `guest_workspace` — the guest path where the workspace is mounted
    ///   (e.g., `"/workspace"`).
    async fn exec_command(
        &self,
        command: &str,
        guest_workspace: &str,
    ) -> anyhow::Result<SandboxOutput>;

    /// Execute a command with the complete host tool contract.
    ///
    /// Existing implementations inherit a compatibility adapter that delegates
    /// to [`Self::exec_command`]. Sandboxes that spawn a real process should
    /// override this method so timeout and output-stream semantics are not
    /// silently lost. The built-in `bash` tool also enforces `timeout_ms` by
    /// dropping this future at the deadline, so implementations must terminate
    /// or otherwise contain child processes when their execution future is
    /// cancelled.
    async fn exec(&self, request: SandboxCommandRequest) -> anyhow::Result<SandboxExecutionOutput> {
        self.exec_command(&request.command, &request.guest_workspace)
            .await
            .map(Into::into)
    }

    /// Shut down the sandbox (best-effort, infallible from caller's perspective).
    async fn shutdown(&self);
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct MockSandbox {
        output: String,
        exit_code: i32,
    }

    #[async_trait]
    impl BashSandbox for MockSandbox {
        async fn exec_command(
            &self,
            _command: &str,
            _guest_workspace: &str,
        ) -> anyhow::Result<SandboxOutput> {
            Ok(SandboxOutput {
                stdout: self.output.clone(),
                stderr: String::new(),
                exit_code: self.exit_code,
            })
        }

        async fn shutdown(&self) {}
    }

    #[tokio::test]
    async fn test_mock_sandbox_success() {
        let sandbox = MockSandbox {
            output: "hello sandbox\n".into(),
            exit_code: 0,
        };
        let result = sandbox
            .exec_command("echo hello sandbox", "/workspace")
            .await
            .unwrap();
        assert_eq!(result.stdout, "hello sandbox\n");
        assert_eq!(result.exit_code, 0);
        assert!(result.stderr.is_empty());
    }

    #[tokio::test]
    async fn test_mock_sandbox_nonzero_exit() {
        let sandbox = MockSandbox {
            output: String::new(),
            exit_code: 127,
        };
        let result = sandbox
            .exec_command("nonexistent_cmd", "/workspace")
            .await
            .unwrap();
        assert_eq!(result.exit_code, 127);
    }

    #[tokio::test]
    async fn test_bash_sandbox_is_arc_send_sync() {
        let sandbox: Arc<dyn BashSandbox> = Arc::new(MockSandbox {
            output: "ok".into(),
            exit_code: 0,
        });
        let result = sandbox.exec_command("true", "/workspace").await.unwrap();
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn protected_workspace_paths_cover_control_metadata_cross_platform() {
        for path in [
            ".git/config",
            "./.a3s/permissions.acl",
            ".AGENTS/worker.acl",
            ".codex\\config",
            ".Claude/settings.json",
            ".vscode/tasks.json",
            ".idea/workspace.xml",
            ".gitmodules",
            ".MCP.JSON",
        ] {
            assert!(
                is_protected_workspace_path(path),
                "{path} should require explicit host authorization"
            );
        }
        for path in [
            "src/lib.rs",
            "nested/.git/config",
            "AGENTS.md",
            "../.git/config",
        ] {
            assert!(
                !is_protected_workspace_path(path),
                "{path} should be handled by another boundary or remain ordinary"
            );
        }
    }
}
