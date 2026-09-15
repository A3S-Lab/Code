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
/// suitable when an outer container already isolates the job.
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
    let sandbox = select_default_local_sandbox(
        local_root,
        allow_process_host,
        crate::sandbox::native::NativeBashSandbox::new(local_root),
    );
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

fn select_default_local_sandbox(
    local_root: &Path,
    allow_process_host: bool,
    native: anyhow::Result<crate::sandbox::native::NativeBashSandbox>,
) -> Arc<dyn BashSandbox> {
    match native {
        Ok(sandbox) => Arc::new(sandbox),
        Err(error) if allow_process_host && local_root.is_dir() => {
            tracing::warn!(
                workspace = %local_root.display(),
                error = %error,
                "default native Bash sandbox is unavailable; using process-host sandbox because the host opted in"
            );
            Arc::new(ProcessHostBashSandbox::new(local_root.to_path_buf(), None))
        }
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

    #[tokio::test]
    async fn process_host_fallback_runs_when_native_fails_and_host_opts_in() {
        let workspace = tempfile::tempdir().unwrap();
        let sandbox = select_default_local_sandbox(
            workspace.path(),
            true,
            Err(anyhow::anyhow!("Linux native sandbox requires bubblewrap")),
        );
        let output = sandbox
            .exec(SandboxCommandRequest {
                command: "printf process-host-ok".into(),
                guest_workspace: workspace.path().display().to_string(),
                timeout_ms: 5_000,
                output_observer: None,
                env: None,
            })
            .await
            .expect("process-host sandbox must execute");
        assert_eq!(output.exit_code, 0);
        assert!(output.stdout.contains("process-host-ok"));
    }

    #[tokio::test]
    async fn process_host_fallback_stays_fail_closed_without_opt_in() {
        let workspace = tempfile::tempdir().unwrap();
        let sandbox = select_default_local_sandbox(
            workspace.path(),
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
}
