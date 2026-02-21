//! Sandbox integration for bash tool execution.
//!
//! When [`SandboxConfig`] is provided via `SessionOptions::with_sandbox()`,
//! the `bash` tool routes commands through an A3S Box MicroVM instead of
//! `std::process::Command`. The workspace directory is mounted read-write
//! at `/workspace` inside the sandbox.
//!
//! # Feature Flag
//!
//! The concrete [`BoxSandboxHandle`] implementation requires the `sandbox`
//! Cargo feature and the `a3s-box-sdk` crate. Without the feature, setting
//! a `SandboxConfig` emits a warning but has no effect.

use async_trait::async_trait;
use std::collections::HashMap;

// ============================================================================
// SandboxConfig
// ============================================================================

/// Configuration for routing `bash` tool execution through an A3S Box sandbox.
///
/// Use [`SessionOptions::with_sandbox()`](crate::SessionOptions::with_sandbox) to activate.
/// Requires the `sandbox` Cargo feature.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// OCI image reference (e.g., `"alpine:latest"`, `"ubuntu:22.04"`).
    pub image: String,
    /// Memory limit in megabytes (default: 512).
    pub memory_mb: u32,
    /// Number of vCPUs (default: 1).
    pub cpus: u32,
    /// Enable outbound networking (default: `false` — safer for agent workflows).
    pub network: bool,
    /// Additional environment variables to inject into the sandbox.
    pub env: HashMap<String, String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            image: "alpine:latest".into(),
            memory_mb: 512,
            cpus: 1,
            network: false,
            env: HashMap::new(),
        }
    }
}

// ============================================================================
// SandboxOutput
// ============================================================================

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
/// Implement this trait to provide a custom sandbox backend.
/// The default implementation ([`BoxSandboxHandle`]) uses A3S Box.
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
// BoxSandboxHandle — feature-gated concrete implementation
// ============================================================================

#[cfg(feature = "sandbox")]
pub use box_impl::BoxSandboxHandle;

#[cfg(feature = "sandbox")]
mod box_impl {
    use super::{BashSandbox, SandboxConfig, SandboxOutput};
    use a3s_box_sdk::{BoxSdk, MountSpec, Sandbox, SandboxOptions};
    use anyhow::Context;
    use async_trait::async_trait;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[allow(clippy::large_enum_variant)]
    enum SandboxState {
        NotStarted,
        Running(Sandbox),
    }

    /// A3S Box–backed sandbox handle.
    ///
    /// Lazily boots the sandbox on the first `exec_command()` call.
    /// The session workspace is mounted read-write at `/workspace`.
    pub struct BoxSandboxHandle {
        state: Arc<Mutex<SandboxState>>,
        config: SandboxConfig,
        /// Absolute host path of the session workspace.
        workspace_host: String,
    }

    impl BoxSandboxHandle {
        /// Create a new handle. The sandbox is **not** started yet — it boots
        /// on the first `exec_command()` call.
        pub fn new(config: SandboxConfig, workspace_host: impl Into<String>) -> Self {
            Self {
                state: Arc::new(Mutex::new(SandboxState::NotStarted)),
                config,
                workspace_host: workspace_host.into(),
            }
        }

        /// Ensure the sandbox is running, booting it if necessary.
        async fn ensure_running(&self) -> anyhow::Result<()> {
            let mut state = self.state.lock().await;
            if matches!(*state, SandboxState::NotStarted) {
                tracing::info!(image = %self.config.image, "Booting A3S Box sandbox");
                let sdk = BoxSdk::new()
                    .await
                    .context("BoxSdk initialization failed")?;

                let opts = SandboxOptions {
                    image: self.config.image.clone(),
                    memory_mb: self.config.memory_mb,
                    cpus: self.config.cpus,
                    network: self.config.network,
                    env: self.config.env.clone(),
                    mounts: vec![MountSpec {
                        host_path: self.workspace_host.clone(),
                        guest_path: "/workspace".into(),
                        readonly: false,
                    }],
                    workdir: Some("/workspace".into()),
                    ..SandboxOptions::default()
                };

                let sandbox = sdk
                    .create(opts)
                    .await
                    .context("Failed to create A3S Box sandbox")?;

                tracing::info!("A3S Box sandbox ready");
                *state = SandboxState::Running(sandbox);
            }
            Ok(())
        }
    }

    #[async_trait]
    impl BashSandbox for BoxSandboxHandle {
        async fn exec_command(
            &self,
            command: &str,
            _guest_workspace: &str,
        ) -> anyhow::Result<SandboxOutput> {
            self.ensure_running().await?;

            let state = self.state.lock().await;
            let sandbox = match &*state {
                SandboxState::Running(s) => s,
                SandboxState::NotStarted => {
                    unreachable!("ensure_running guarantees Running state")
                }
            };

            let result = sandbox
                .exec("bash", &["-c", command])
                .await
                .context("Sandbox exec failed")?;

            Ok(SandboxOutput {
                stdout: result.stdout,
                stderr: result.stderr,
                exit_code: result.exit_code,
            })
        }

        async fn shutdown(&self) {
            let mut state = self.state.lock().await;
            // Take ownership of the sandbox to call stop() (which takes self).
            let old = std::mem::replace(&mut *state, SandboxState::NotStarted);
            if let SandboxState::Running(sandbox) = old {
                tracing::info!("Stopping A3S Box sandbox");
                if let Err(e) = sandbox.stop().await {
                    tracing::warn!("Sandbox stop failed (non-fatal): {}", e);
                }
            }
        }
    }

    impl Drop for BoxSandboxHandle {
        fn drop(&mut self) {
            // Best-effort async cleanup: spawn a task to stop the sandbox.
            let state = Arc::clone(&self.state);
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let mut s = state.lock().await;
                    let old = std::mem::replace(&mut *s, SandboxState::NotStarted);
                    if let SandboxState::Running(sandbox) = old {
                        let _ = sandbox.stop().await;
                    }
                });
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_sandbox_config_default() {
        let cfg = SandboxConfig::default();
        assert_eq!(cfg.image, "alpine:latest");
        assert_eq!(cfg.memory_mb, 512);
        assert_eq!(cfg.cpus, 1);
        assert!(!cfg.network);
        assert!(cfg.env.is_empty());
    }

    #[test]
    fn test_sandbox_config_custom() {
        let cfg = SandboxConfig {
            image: "ubuntu:22.04".into(),
            memory_mb: 1024,
            cpus: 2,
            network: true,
            env: [("FOO".into(), "bar".into())].into(),
        };
        assert_eq!(cfg.image, "ubuntu:22.04");
        assert_eq!(cfg.memory_mb, 1024);
        assert_eq!(cfg.cpus, 2);
        assert!(cfg.network);
        assert_eq!(cfg.env["FOO"], "bar");
    }

    #[test]
    fn test_sandbox_config_clone() {
        let cfg = SandboxConfig {
            image: "python:3.12-slim".into(),
            ..SandboxConfig::default()
        };
        let cloned = cfg.clone();
        assert_eq!(cloned.image, "python:3.12-slim");
        assert_eq!(cloned.memory_mb, cfg.memory_mb);
    }

    // -----------------------------------------------------------------------
    // Mock BashSandbox for testing the trait contract
    // -----------------------------------------------------------------------

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
        // Verify the trait object is Send + Sync so it can live in ToolContext.
        let sandbox: Arc<dyn BashSandbox> = Arc::new(MockSandbox {
            output: "ok".into(),
            exit_code: 0,
        });
        let result = sandbox.exec_command("true", "/workspace").await.unwrap();
        assert_eq!(result.exit_code, 0);
    }
}
