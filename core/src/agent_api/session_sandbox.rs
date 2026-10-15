//! Default process-isolation binding for local sessions.

use super::SessionOptions;
use crate::sandbox::process_host::ProcessHostBashSandbox;
use crate::sandbox::{BashSandbox, SandboxOutput};
use std::path::Path;
use std::sync::Arc;

/// Environment opt-in for process-host fallback when the native sandbox cannot
/// initialize (Harbor / Terminal-Bench containers without bubblewrap).
pub const ALLOW_PROCESS_HOST_SANDBOX_ENV: &str = "A3S_CODE_ALLOW_PROCESS_HOST_SANDBOX";

/// Bind the built-in native sandbox before capabilities are assembled so the
/// top-level Bash tool, workflows, and delegated child runs all inherit the
/// same process-isolation boundary.
///
/// A host-provided sandbox remains authoritative. Non-local workspace services
/// retain their own command runner because they do not expose a local root. If
/// the native backend cannot initialize, the default remains fail-closed
/// (`UnavailableDefaultSandbox`) unless the host opts into
/// [`SessionOptions::with_allow_process_host_sandbox`] /
/// [`ALLOW_PROCESS_HOST_SANDBOX_ENV`], which installs a process-host runner
/// suitable when an outer container already isolates the job. The fallback
/// only ever serves a deny posture: a mediated request degrades to
/// unavailable (with an error-level log) rather than silently dropping the
/// mediation the host asked for.
pub(super) fn install_default_local_sandbox(workspace: &Path, opts: &mut SessionOptions) {
    if opts.sandbox_handle.is_some() {
        return;
    }

    let local_root = match opts.workspace_services.as_ref() {
        Some(services) => match services.local_root() {
            Some(root) => root,
            None => return,
        },
        None => workspace,
    };

    let allow_process_host = allow_process_host_sandbox(opts);
    let native = match opts.sandbox_network {
        crate::sandbox::SandboxNetworkMode::Deny => {
            crate::sandbox::native::NativeBashSandbox::new(local_root)
        }
        crate::sandbox::SandboxNetworkMode::Mediated => {
            crate::sandbox::native::NativeBashSandbox::with_network_mode(
                local_root,
                crate::sandbox::SandboxNetworkMode::Mediated,
                &opts.sandbox_network_allow,
            )
        }
    };
    let sandbox =
        select_default_local_sandbox(local_root, opts.sandbox_network, allow_process_host, native);
    opts.sandbox_handle = Some(sandbox);
}

fn allow_process_host_sandbox(opts: &SessionOptions) -> bool {
    if opts.allow_process_host_sandbox {
        return true;
    }
    std::env::var_os(ALLOW_PROCESS_HOST_SANDBOX_ENV).is_some_and(|value| {
        matches!(
            value.to_string_lossy().trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

pub(crate) fn select_default_local_sandbox(
    local_root: &Path,
    requested: crate::sandbox::SandboxNetworkMode,
    allow_process_host: bool,
    native: anyhow::Result<crate::sandbox::native::NativeBashSandbox>,
) -> Arc<dyn BashSandbox> {
    match native {
        Ok(sandbox) => Arc::new(sandbox),
        Err(error) if allow_process_host && local_root.is_dir() => match requested {
            // The process-host runner has no network mediation, so it may
            // only serve a deny posture: installing it under a mediated
            // request would deliver unmediated host egress — wider than
            // the host asked for. A future posture must decide its own
            // fallback stance here, never inherit this one silently.
            crate::sandbox::SandboxNetworkMode::Deny => {
                tracing::warn!(
                    workspace = %local_root.display(),
                    error = %error,
                    "default native Bash sandbox is unavailable; using process-host sandbox because the host opted in"
                );
                Arc::new(ProcessHostBashSandbox::new(local_root.to_path_buf(), None))
            }
            crate::sandbox::SandboxNetworkMode::Mediated => {
                let message = format!(
                    "the default A3S native sandbox is unavailable for '{}': {error:#}; the \
                     process-host fallback was NOT installed because it cannot honor the \
                     requested '{}' network posture — install the native backend (e.g. \
                     bubblewrap) or drop the mediated posture",
                    local_root.display(),
                    requested.as_str(),
                );
                tracing::error!(
                    workspace = %local_root.display(),
                    error = %error,
                    posture = requested.as_str(),
                    "default native Bash sandbox is unavailable and the process-host fallback cannot honor the mediated posture; Bash remains denied"
                );
                Arc::new(UnavailableDefaultSandbox { message })
            }
        },
        Err(error) => {
            let message = format!(
                "the default A3S native sandbox is unavailable for '{}': {error:#}",
                local_root.display()
            );
            tracing::warn!(workspace = %local_root.display(), error = %error, "default native Bash sandbox is unavailable; Bash remains denied");
            Arc::new(UnavailableDefaultSandbox { message })
        }
    }
}

#[derive(Debug)]
struct UnavailableDefaultSandbox {
    message: String,
}

#[async_trait::async_trait]
impl BashSandbox for UnavailableDefaultSandbox {
    fn validate_execution_available(&self) -> anyhow::Result<()> {
        anyhow::bail!(self.message.clone())
    }

    async fn exec_command(
        &self,
        _command: &str,
        _guest_workspace: &str,
    ) -> anyhow::Result<SandboxOutput> {
        anyhow::bail!(self.message.clone())
    }

    async fn shutdown(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::sandbox::SandboxCommandRequest;

    struct CustomSandbox;

    #[async_trait::async_trait]
    impl BashSandbox for CustomSandbox {
        async fn exec_command(
            &self,
            _command: &str,
            _guest_workspace: &str,
        ) -> anyhow::Result<SandboxOutput> {
            Ok(SandboxOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            })
        }

        async fn shutdown(&self) {}
    }

    #[test]
    fn explicit_sandbox_remains_authoritative() {
        let workspace = tempfile::tempdir().unwrap();
        let expected: Arc<dyn BashSandbox> = Arc::new(CustomSandbox);
        let mut opts = SessionOptions::new().with_sandbox_handle(Arc::clone(&expected));

        install_default_local_sandbox(workspace.path(), &mut opts);

        let actual = opts.sandbox_handle.as_ref().unwrap();
        assert!(Arc::ptr_eq(actual, &expected));
    }

    #[tokio::test]
    async fn unavailable_local_workspace_gets_error_only_handle() {
        let parent = tempfile::tempdir().unwrap();
        let workspace = parent.path().join("missing");
        let mut opts = SessionOptions::new();

        install_default_local_sandbox(&workspace, &mut opts);

        let sandbox = opts.sandbox_handle.as_ref().unwrap();
        let error = match sandbox
            .exec_command("this-must-never-run", "/workspace")
            .await
        {
            Ok(_) => panic!("unavailable sandbox unexpectedly executed a command"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains("the default A3S native sandbox is unavailable"));
    }

    // Harbor / Terminal-Bench process-host bash is a Unix outer-isolation path.
    // Windows product bash uses PowerShell; Git Bash spawned from MSVC is not
    // the supported process-host contract.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn native_execution_refusal_never_selects_initialization_fallback() {
        use crate::tools::Tool;
        let workspace = tempfile::tempdir().unwrap();
        let native = crate::sandbox::native::NativeBashSandbox::new(workspace.path()).unwrap();
        let digest = native.policy_digest();
        let sandbox = select_default_local_sandbox(
            workspace.path(),
            crate::sandbox::SandboxNetworkMode::Deny,
            true,
            Ok(native),
        );
        assert_eq!(sandbox.policy_digest(), digest);
        for _ in 0..2 {
            let output = crate::tools::builtin::bash::BashTool
                .execute(
                    &serde_json::json!({"command":"printf unexpected-host-fallback > no-fallback-sentinel"}),
                    &crate::tools::ToolContext::new(workspace.path().to_path_buf())
                        .with_sandbox(Arc::clone(&sandbox)),
                )
                .await
                .expect("a pre-spawn native refusal is a settled tool result");
            assert!(!output.success);
            assert!(output
                .content
                .contains("native_process_lifetime_unqualified"));
            assert!(matches!(
                output.error_kind,
                Some(crate::tools::ToolErrorKind::Unsupported { .. })
            ));
            assert!(!workspace.path().join("no-fallback-sentinel").exists());
            assert_eq!(sandbox.policy_digest(), digest);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_host_fallback_runs_when_native_fails_and_host_opts_in() {
        let workspace = tempfile::tempdir().unwrap();
        let sandbox = select_default_local_sandbox(
            workspace.path(),
            crate::sandbox::SandboxNetworkMode::Deny,
            true,
            Err(anyhow::anyhow!("Linux native sandbox requires bubblewrap")),
        );
        let output = sandbox
            .exec(SandboxCommandRequest {
                command: "printf '%s\\n' 'process-host-ok'".into(),
                guest_workspace: workspace.path().display().to_string(),
                timeout_ms: 5_000,
                output_observer: None,
                env: None,
            })
            .await
            .expect("process-host sandbox must execute");
        assert_eq!(
            output.exit_code, 0,
            "process-host bash exit={}; stdout={:?} stderr={}",
            output.exit_code, output.stdout, output.stderr
        );
        assert!(
            output.stdout.contains("process-host-ok"),
            "stdout={:?} stderr={:?}",
            output.stdout,
            output.stderr
        );
    }

    #[tokio::test]
    async fn process_host_fallback_stays_fail_closed_without_opt_in() {
        let workspace = tempfile::tempdir().unwrap();
        let sandbox = select_default_local_sandbox(
            workspace.path(),
            crate::sandbox::SandboxNetworkMode::Deny,
            false,
            Err(anyhow::anyhow!("Linux native sandbox requires bubblewrap")),
        );
        let error = match sandbox
            .exec_command("printf should-not-run", "/workspace")
            .await
        {
            Ok(_) => panic!("without opt-in the default remains unavailable"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains("the default A3S native sandbox is unavailable"));
    }

    #[test]
    fn session_option_enables_process_host_opt_in() {
        let opts = SessionOptions::new().with_allow_process_host_sandbox(true);
        assert!(allow_process_host_sandbox(&opts));
    }

    #[test]
    fn env_opt_in_accepts_common_truthy_values() {
        let previous = std::env::var_os(ALLOW_PROCESS_HOST_SANDBOX_ENV);
        let opts = SessionOptions::new();
        for value in ["1", "true", "YES", " On "] {
            std::env::set_var(ALLOW_PROCESS_HOST_SANDBOX_ENV, value);
            assert!(
                allow_process_host_sandbox(&opts),
                "expected env value {value:?} to opt in"
            );
        }
        std::env::set_var(ALLOW_PROCESS_HOST_SANDBOX_ENV, "0");
        assert!(!allow_process_host_sandbox(&opts));
        match previous {
            Some(value) => std::env::set_var(ALLOW_PROCESS_HOST_SANDBOX_ENV, value),
            None => std::env::remove_var(ALLOW_PROCESS_HOST_SANDBOX_ENV),
        }
    }

    #[tokio::test]
    async fn unavailable_sandbox_shutdown_is_a_noop() {
        let sandbox = UnavailableDefaultSandbox {
            message: "denied".into(),
        };
        sandbox.shutdown().await;
    }

    #[tokio::test]
    async fn custom_sandbox_handle_remains_callable() {
        let workspace = tempfile::tempdir().unwrap();
        let expected: Arc<dyn BashSandbox> = Arc::new(CustomSandbox);
        let mut opts = SessionOptions::new().with_sandbox_handle(Arc::clone(&expected));
        install_default_local_sandbox(workspace.path(), &mut opts);
        let sandbox = opts.sandbox_handle.as_ref().unwrap();
        let output = sandbox
            .exec_command("unused", "/workspace")
            .await
            .expect("custom sandbox must run");
        assert_eq!(output.exit_code, 0);
        sandbox.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_host_opt_in_selects_process_host_when_native_fails() {
        let workspace = tempfile::tempdir().unwrap();
        let sandbox = select_default_local_sandbox(
            workspace.path(),
            crate::sandbox::SandboxNetworkMode::Deny,
            true,
            Err(anyhow::anyhow!("native unavailable for coverage")),
        );
        let output = sandbox
            .exec_command("printf 'selected-process-host\\n'", "/workspace")
            .await
            .expect("process-host selection must execute");
        assert_eq!(output.exit_code, 0, "stderr={}", output.stderr);
        assert!(output.stdout.contains("selected-process-host"));
        sandbox.shutdown().await;
    }

    #[tokio::test]
    async fn mediated_request_refuses_the_process_host_fallback() {
        let workspace = tempfile::tempdir().unwrap();
        // The host opted into the fallback AND asked for mediated egress;
        // the fallback cannot mediate, so the default must stay denied
        // instead of silently widening to unmediated host execution.
        let sandbox = select_default_local_sandbox(
            workspace.path(),
            crate::sandbox::SandboxNetworkMode::Mediated,
            true,
            Err(anyhow::anyhow!("Linux native sandbox requires bubblewrap")),
        );
        let error = match sandbox
            .exec_command("printf must-not-run", "/workspace")
            .await
        {
            Ok(_) => panic!("mediated request must not degrade to the process-host sandbox"),
            Err(error) => error,
        };
        let message = error.to_string();
        assert!(
            message.contains("the default A3S native sandbox is unavailable"),
            "stable prefix kept: {message}"
        );
        assert!(
            message.contains("process-host fallback was NOT installed"),
            "the refusal names the withheld fallback: {message}"
        );
        assert!(
            message.contains("mediated"),
            "the refusal names the posture that cannot be honored: {message}"
        );
    }
}
