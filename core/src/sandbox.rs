//! Sandbox integration for bash tool execution.
//!
//! Local A3S Code sessions install a [`BashSandbox`] automatically, and the
//! `bash` built-in tool routes commands through it instead of
//! `std::process::Command`. The A3S native backend keeps the canonical host
//! workspace path while enforcing the platform isolation boundary around the
//! child process.
//!
//! [`native::NativeBashSandbox`] is the A3S-owned fail-closed implementation
//! used by default throughout A3S Code. Hosts can still supply another
//! implementation through the trait contract when they own an equivalent
//! isolation boundary. Non-local workspace backends retain their explicit
//! command-runner contract.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::workspace::CommandOutputObserver;

pub mod guest;
pub mod native;
pub mod process_host;

pub use a3s_sandbox::{
    is_protected_workspace_path, PROTECTED_WORKSPACE_DIRECTORIES, PROTECTED_WORKSPACE_FILES,
};
pub use process_host::ProcessHostBashSandbox;

pub(crate) use native::parse_network_allow_rules;

/// Closed set of sandbox network postures the product exposes.
///
/// `Deny` (the fail-closed default) keeps the baseline: sandboxed commands
/// have no network. `Mediated` arms the sandbox crate's Gate 4 stack — the
/// deny-by-default platform profile plus a loopback-only CONNECT mediator,
/// with `HTTP_PROXY`/`HTTPS_PROXY` (and `NO_PROXY=""`) forced into the
/// child environment so egress routes through mediation decisions instead
/// of escaping outright. Arming is an explicit host opt-in: it widens a
/// previously denied surface to AVAILABLE-through-mediation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxNetworkMode {
    /// No network in the sandbox (baseline, unchanged).
    #[default]
    Deny,
    /// Gate 4 mediated egress: forced proxy through the loopback mediator.
    Mediated,
}

impl SandboxNetworkMode {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "deny" => Ok(Self::Deny),
            "mediated" => Ok(Self::Mediated),
            other => anyhow::bail!(
                "sandbox network mode must be one of the closed set \
                 [deny, mediated]; got {other:?}"
            ),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Deny => "deny",
            Self::Mediated => "mediated",
        }
    }
}

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
/// Implement this trait to replace the native sandbox with a custom backend.
/// The host application constructs the implementation and passes it to the
/// session via [`crate::SessionOptions::with_sandbox_handle`].
#[async_trait]
pub trait BashSandbox: Send + Sync {
    /// Check whether this backend can admit command execution without starting
    /// a process or producing effects. A refusal is a known unexecuted call,
    /// rather than an execution whose terminal state is unknown. Implementations
    /// must still enforce the same boundary inside their execution methods.
    fn validate_execution_available(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Execute a shell command inside the sandbox.
    ///
    /// * `command` — the shell command string (passed as `bash -c <command>`).
    /// * `guest_workspace` — the workspace path expected by a custom guest
    ///   backend (for example, `"/workspace"`). The A3S native backend uses its
    ///   canonical host workspace instead.
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

    /// Whether [`Self::exec`] supports Run-owned background execution.
    ///
    /// Opting in promises streaming output, final capture accounting, and
    /// termination of the command and its descendants when the execution
    /// future is dropped. The legacy adapter cannot prove these guarantees
    /// and therefore refuses background execution by default.
    fn supports_background_execution(&self) -> bool {
        false
    }

    /// Shut down the sandbox (best-effort, infallible from caller's perspective).
    async fn shutdown(&self);

    /// Snapshot of the active policy digest for grant lineage, when the
    /// backend supports host-approved grants.
    fn policy_digest(&self) -> Option<String> {
        None
    }

    /// Apply a host-approved network grant: the only sanctioned broadening
    /// path. `expected_base_digest` pins the approval to the exact policy the
    /// user confirmed against. The default refuses: custom backends opt in
    /// explicitly.
    fn apply_network_grant(
        &self,
        _grant: a3s_sandbox::NetworkGrant,
        _expected_base_digest: &str,
    ) -> anyhow::Result<String> {
        anyhow::bail!("this sandbox backend does not support policy grants")
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_api::SessionOptions;
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

    #[test]
    fn sandbox_network_mode_parse_names_the_closed_set() {
        assert_eq!(
            SandboxNetworkMode::parse("deny").unwrap(),
            SandboxNetworkMode::Deny
        );
        assert_eq!(
            SandboxNetworkMode::parse(" Mediated ").unwrap(),
            SandboxNetworkMode::Mediated
        );
        let error = SandboxNetworkMode::parse("open").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("closed set"), "got: {message}");
        assert!(
            message.contains("deny") && message.contains("mediated"),
            "got: {message}"
        );
    }

    #[test]
    fn sandbox_network_mode_default_is_deny() {
        assert_eq!(
            SessionOptions::new().sandbox_network,
            SandboxNetworkMode::Deny
        );
        assert_eq!(
            SessionOptions::new()
                .with_sandbox_network_mode(SandboxNetworkMode::Mediated)
                .sandbox_network,
            SandboxNetworkMode::Mediated
        );
        assert_eq!(SandboxNetworkMode::Deny.as_str(), "deny");
        assert_eq!(SandboxNetworkMode::Mediated.as_str(), "mediated");
    }
}
