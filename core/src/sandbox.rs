//! Sandbox integration for bash tool execution.
//!
//! When a [`BashSandbox`] is provided via [`ToolContext::with_sandbox`], the
//! `bash` built-in tool routes commands through that sandbox instead of
//! `std::process::Command`. The workspace directory is mounted read-write
//! at `/workspace` inside the sandbox.
//!
//! The concrete sandbox implementation is supplied by the host application
//! (e.g., SafeClaw can provide an A3S Box–backed implementation after the
//! user installs `a3s-box`). This crate defines only the trait contract.

use async_trait::async_trait;
/// Output from running a command inside a sandbox.
pub struct SandboxOutput {
    /// Standard output bytes decoded as UTF-8.
    pub stdout: String,
    /// Standard error bytes decoded as UTF-8.
    pub stderr: String,
    /// Process exit code (0 = success).
    pub exit_code: i32,
}

// ============================================================================
// BashSandbox trait
// ============================================================================

/// Abstraction over sandbox bash execution used by the `bash` built-in tool.
///
/// Implement this trait to provide a custom sandbox backend. The host
/// application constructs the implementation and passes it to the session
/// via [`ToolContext::with_sandbox`].
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
}
