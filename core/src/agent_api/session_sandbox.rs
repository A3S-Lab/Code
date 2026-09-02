//! Default process-isolation binding for local sessions.

use super::SessionOptions;
use crate::sandbox::{BashSandbox, SandboxOutput};
use std::path::Path;
use std::sync::Arc;

/// Bind the built-in native sandbox before capabilities are assembled so the
/// top-level Bash tool, workflows, and delegated child runs all inherit the
/// same process-isolation boundary.
///
/// A host-provided sandbox remains authoritative. Non-local workspace services
/// retain their own command runner because they do not expose a local root. If
/// the native backend cannot initialize, an error-only handle is installed so
/// no direct or governed Bash path can fall back to the host runner.
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

    let sandbox: Arc<dyn BashSandbox> = match crate::sandbox::native::NativeBashSandbox::new(
        local_root,
    ) {
        Ok(sandbox) => Arc::new(sandbox),
        Err(error) => {
            let message = format!(
                "the default A3S native sandbox is unavailable for '{}': {error:#}",
                local_root.display()
            );
            tracing::warn!(workspace = %local_root.display(), error = %error, "default native Bash sandbox is unavailable; Bash remains denied");
            Arc::new(UnavailableDefaultSandbox { message })
        }
    };
    opts.sandbox_handle = Some(sandbox);
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
}
