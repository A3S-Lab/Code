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

pub(crate) use a3s_sandbox::{
    hard_link_count, hard_link_count_for_open_file, sensitive_paths,
    should_skip_workspace_scan_directory, workspace_hardlink_paths, workspace_sensitive_paths,
};
pub use a3s_sandbox::{CapabilityReport, ExecutionUnavailable, NATIVE_SANDBOX_BACKEND};

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

    /// Build with an explicit network posture. `Deny` is byte-identical to
    /// [`Self::new`]; `Mediated` arms Gate 4 on the same baseline policy
    /// with the given origins (`host` or `host:port`, any path). An empty
    /// allowlist is refused by the sandbox crate — mediating nothing is
    /// not a posture — and unsupported platforms fail closed.
    pub fn with_network_mode(
        workspace: impl Into<PathBuf>,
        mode: super::SandboxNetworkMode,
        allow_origins: &[String],
    ) -> Result<Self> {
        let inner = match mode {
            super::SandboxNetworkMode::Deny => a3s_sandbox::NativeSandbox::new(workspace)?,
            super::SandboxNetworkMode::Mediated => {
                let mut policy = a3s_sandbox::SandboxPolicy::a3s_bash_baseline();
                policy.features.mediated_network = true;
                policy.network.allow = parse_network_allow_rules(allow_origins)?;
                a3s_sandbox::NativeSandbox::with_policy(workspace, policy)?
            }
        };
        Ok(Self { inner })
    }

    /// Whether Gate 4 mediation is armed in the active policy.
    pub fn network_mediation_armed(&self) -> bool {
        self.inner.policy().features.mediated_network
    }

    pub fn workspace(&self) -> &Path {
        self.inner.workspace()
    }

    pub fn backend(&self) -> &'static str {
        self.inner.backend()
    }

    pub async fn probe(&self) -> Result<()> {
        self.probe_capabilities().await.map(|_| ())
    }

    pub fn capability_report(&self) -> CapabilityReport {
        self.inner.capability_report()
    }

    pub async fn probe_capabilities(&self) -> Result<CapabilityReport> {
        self.inner.probe().await
    }

    fn ensure_process_lifetime_qualified(&self) -> Result<()> {
        ensure_lifetime_capabilities(&self.capability_report())
    }
}

fn ensure_lifetime_capabilities(report: &CapabilityReport) -> Result<()> {
    let reason = if !report.capabilities.process_tree_cleanup {
        ExecutionUnavailable::ProcessTreeCleanup
    } else if !report.capabilities.host_exit_cleanup {
        ExecutionUnavailable::HostExitCleanup
    } else {
        return Ok(());
    };
    Err(NativeCapabilityUnavailable {
        backend: report.backend,
        reason,
    }
    .into())
}

#[derive(Debug)]
struct NativeCapabilityUnavailable {
    backend: &'static str,
    reason: ExecutionUnavailable,
}

impl std::fmt::Display for NativeCapabilityUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.backend == "macos-seatbelt"
            && self.reason == ExecutionUnavailable::ProcessTreeCleanup
        {
            return formatter.write_str(NATIVE_PROCESS_LIFETIME_UNQUALIFIED);
        }
        write!(
            formatter,
            "native_execution_unavailable: {}: {}",
            self.backend, self.reason
        )
    }
}

impl std::error::Error for NativeCapabilityUnavailable {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.reason)
    }
}

/// Compatibility reason for unqualified macOS descendant cleanup.
pub(crate) const NATIVE_PROCESS_LIFETIME_UNQUALIFIED: &str = "native_process_lifetime_unqualified: macOS native command execution is unavailable until descendant lifetime and settlement are qualified";

/// The native backend refused before any command process existed.
///
/// A timeout, a dropped child, or any other sandbox failure is a different
/// error. Those stay unknown because the command may already have run.
#[derive(Debug)]
pub(crate) struct NativeProcessLifetimeUnqualified;

impl std::fmt::Display for NativeProcessLifetimeUnqualified {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(NATIVE_PROCESS_LIFETIME_UNQUALIFIED)
    }
}

impl std::error::Error for NativeProcessLifetimeUnqualified {}

pub(crate) fn native_process_lifetime_unqualified() -> anyhow::Error {
    NativeProcessLifetimeUnqualified.into()
}

#[cfg(test)]
pub(crate) fn is_native_process_lifetime_unqualified(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.is::<NativeProcessLifetimeUnqualified>()
            || cause
                .downcast_ref::<NativeCapabilityUnavailable>()
                .is_some_and(|failure| failure.reason == ExecutionUnavailable::ProcessTreeCleanup)
    })
}

pub(crate) fn is_native_execution_unavailable(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.is::<NativeProcessLifetimeUnqualified>() || cause.is::<NativeCapabilityUnavailable>()
    })
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
    fn validate_execution_available(&self) -> Result<()> {
        self.ensure_process_lifetime_qualified()
    }

    fn supports_background_execution(&self) -> bool {
        self.ensure_process_lifetime_qualified().is_ok()
    }

    fn policy_digest(&self) -> Option<String> {
        Some(self.inner.policy_digest())
    }

    fn apply_network_grant(
        &self,
        grant: a3s_sandbox::NetworkGrant,
        expected_base_digest: &str,
    ) -> Result<String> {
        self.inner.apply_network_grant(grant, expected_base_digest)
    }

    async fn exec_command(&self, command: &str, _guest_workspace: &str) -> Result<SandboxOutput> {
        self.ensure_process_lifetime_qualified()?;
        let output = self.inner.exec_command(command).await?;
        Ok(SandboxOutput {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.exit_code,
        })
    }

    async fn exec(&self, request: SandboxCommandRequest) -> Result<SandboxExecutionOutput> {
        self.ensure_process_lifetime_qualified()?;
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

/// Parse `host` / `host:port` origin strings into allow rules (any path).
/// Malformed entries refuse construction — a typo must not become a
/// silently wider origin.
pub(crate) fn parse_network_allow_rules(
    origins: &[String],
) -> Result<Vec<a3s_sandbox::NetworkAllowRule>> {
    origins
        .iter()
        .map(|origin| {
            let origin = origin.trim();
            let (host, port) = match origin.rsplit_once(':') {
                Some((host, port)) => {
                    let port: u16 = port.parse().map_err(|_| {
                        anyhow::anyhow!(
                            "sandbox network allow origin {origin:?} has a non-numeric port"
                        )
                    })?;
                    (host.trim().to_string(), Some(port))
                }
                None => (origin.to_string(), None),
            };
            if host.is_empty()
                || host
                    .chars()
                    .any(|character| matches!(character, '/' | ' ' | ':' | '[' | ']'))
            {
                anyhow::bail!(
                    "sandbox network allow origin {origin:?} is not a bare host[:port] \
                     (bracketed or bare IPv6 literals are not accepted yet)"
                );
            }
            if port == Some(0) {
                anyhow::bail!("sandbox network allow origin {origin:?} has port 0");
            }
            Ok(a3s_sandbox::NetworkAllowRule {
                host,
                port,
                path_prefix: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod capability_tests;
#[cfg(test)]
mod lifetime_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_applies_digest_pinned_grant_on_the_live_sandbox() {
        let workspace = tempfile::tempdir().unwrap();
        let sandbox = NativeBashSandbox::new(workspace.path()).unwrap();
        let base = sandbox
            .policy_digest()
            .expect("native backend exposes a digest");
        let new_digest = sandbox
            .apply_network_grant(
                a3s_sandbox::NetworkGrant::new("api.example.com", Some(443)).unwrap(),
                &base,
            )
            .expect("grant must apply on the deny-all baseline");
        assert_ne!(new_digest, base);
        assert!(sandbox.policy_digest().is_some());

        // Stale lineage refuses: the digest moved since the approval.
        let error = sandbox
            .apply_network_grant(
                a3s_sandbox::NetworkGrant::new("other.example.com", Some(443)).unwrap(),
                &base,
            )
            .expect_err("stale digest must refuse");
        assert!(error.to_string().contains("digest"), "{error}");
    }

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
        let result = sandbox.exec_command(command, "/workspace").await;
        if cfg!(target_os = "macos") {
            let error = result.err().expect("unqualified native lifetime refuses");
            assert!(error
                .to_string()
                .contains("native_process_lifetime_unqualified"));
            return;
        }
        let output = result.unwrap();
        assert_eq!(output.stdout, "adapter-ready");
        assert_eq!(output.exit_code, 0);
    }

    #[test]
    fn deny_mode_is_the_unchanged_baseline_policy() {
        let workspace = tempfile::tempdir().unwrap();
        let baseline =
            NativeBashSandbox::new(workspace.path()).expect("baseline sandbox constructs");
        let deny = NativeBashSandbox::with_network_mode(
            workspace.path(),
            crate::sandbox::SandboxNetworkMode::Deny,
            &[],
        )
        .expect("deny mode constructs");
        assert_eq!(
            baseline.inner.policy_digest(),
            deny.inner.policy_digest(),
            "deny must be byte-identical to the default policy"
        );
        assert!(!deny.network_mediation_armed());
    }

    #[test]
    fn mediated_mode_arms_gate4_with_the_given_allowlist() {
        let workspace = tempfile::tempdir().unwrap();
        let mediated = NativeBashSandbox::with_network_mode(
            workspace.path(),
            crate::sandbox::SandboxNetworkMode::Mediated,
            &[
                "registry.example".to_string(),
                "packages.internal:8443".to_string(),
            ],
        )
        .expect("mediated policy with an allowlist arms on this host");
        assert!(mediated.network_mediation_armed());
        let policy = mediated.inner.policy();
        let hosts: Vec<&str> = policy
            .network
            .allow
            .iter()
            .map(|r| r.host.as_str())
            .collect();
        assert_eq!(hosts, vec!["registry.example", "packages.internal"]);
        assert_eq!(policy.network.allow[1].port, Some(8443));

        // An empty allowlist is refused: mediating nothing is not a posture.
        let refusal = NativeBashSandbox::with_network_mode(
            workspace.path(),
            crate::sandbox::SandboxNetworkMode::Mediated,
            &[],
        )
        .unwrap_err();
        assert!(
            format!("{refusal:#}").contains("allow"),
            "refusal names the allowlist: {refusal:#}"
        );

        // Malformed origins refuse instead of widening into a typo —
        // including the review's findings: a bare IPv6 literal mis-splits
        // into host ":" port 1, a bracketed IPv6 keeps its brackets, and
        // port 0 is not a real destination.
        for bad in ["registry.example:notaport", "::1", "[::1]:443", "host:0"] {
            let malformed = NativeBashSandbox::with_network_mode(
                workspace.path(),
                crate::sandbox::SandboxNetworkMode::Mediated,
                &[bad.to_string()],
            )
            .unwrap_err();
            assert!(!format!("{malformed:#}").is_empty(), "{bad:?} must refuse");
        }
    }
}
