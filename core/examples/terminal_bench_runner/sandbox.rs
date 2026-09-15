//! Terminal-Bench runner re-exports the Core process-host sandbox.
//!
//! The benchmark adapter must not buffer arbitrary command output before the
//! Core tool-output contract can bound it. Process-group kill, deadline, and
//! bounded capture live in
//! [`a3s_code_core::sandbox::ProcessHostBashSandbox`].

pub(super) use a3s_code_core::sandbox::ProcessHostBashSandbox;

#[cfg(test)]
mod tests {
    use super::*;
    use a3s_code_core::sandbox::{BashSandbox, SandboxCommandRequest};
    use std::time::{Duration, Instant};

    #[cfg(unix)]
    #[tokio::test]
    async fn sandbox_timeout_terminates_the_command_process_group() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let sandbox = ProcessHostBashSandbox::new(
            directory.path().to_path_buf(),
            Some(Instant::now() + Duration::from_secs(5)),
        );
        let output = sandbox
            .exec(SandboxCommandRequest {
                command: "exec 1>&- 2>&-; (sleep 0.3; touch leaked) & wait".to_string(),
                guest_workspace: directory.path().to_string_lossy().into_owned(),
                timeout_ms: 20,
                output_observer: None,
                env: None,
            })
            .await
            .expect("sandbox execution");
        assert!(output.timed_out);
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!directory.path().join("leaked").exists());
    }
}
