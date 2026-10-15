#![cfg(test)]

//! Deterministic backend for the generic background contract. It creates no
//! process and interprets no shell text; native isolation is a separate proof.

use crate::sandbox::{BashSandbox, SandboxCommandRequest, SandboxExecutionOutput, SandboxOutput};
use crate::workspace::CommandOutputSummary;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub(crate) struct DeterministicBackgroundSandbox {
    root: PathBuf,
    line: String,
    exit_code: i32,
    held: bool,
    write: Option<(String, Vec<u8>, u32)>,
    calls: AtomicUsize,
    released: AtomicBool,
}

impl DeterministicBackgroundSandbox {
    pub(crate) fn new(root: PathBuf, line: &str, exit_code: i32, held: bool) -> Self {
        Self {
            root,
            line: line.to_string(),
            exit_code,
            held,
            write: None,
            calls: AtomicUsize::new(0),
            released: AtomicBool::new(false),
        }
    }

    pub(crate) fn with_write(mut self, path: &str, bytes: &[u8], mode: u32) -> Self {
        self.write = Some((path.to_string(), bytes.to_vec(), mode));
        self
    }

    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub(crate) fn was_released(&self) -> bool {
        self.released.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl BashSandbox for DeterministicBackgroundSandbox {
    fn supports_background_execution(&self) -> bool {
        true
    }

    async fn exec_command(&self, _: &str, _: &str) -> anyhow::Result<SandboxOutput> {
        anyhow::bail!("the deterministic background backend requires extended execution")
    }

    async fn exec(&self, request: SandboxCommandRequest) -> anyhow::Result<SandboxExecutionOutput> {
        struct ReleaseProof<'a>(&'a AtomicBool);
        impl Drop for ReleaseProof<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let _release_proof = ReleaseProof(&self.released);
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some((path, bytes, mode)) = &self.write {
            let path = self.root.join(path);
            std::fs::write(&path, bytes)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(*mode))?;
            }
            #[cfg(not(unix))]
            let _ = mode;
        }
        let line = format!("{}-{call}\n", self.line);
        let observer = request.output_observer.ok_or_else(|| {
            anyhow::anyhow!("deterministic background proof requires its output observer")
        })?;
        observer.on_output_delta(&line).await;
        if self.held {
            std::future::pending::<()>().await;
        }
        let bytes = line.len();
        observer
            .on_output_complete(&CommandOutputSummary {
                total_bytes: bytes,
                captured_bytes: bytes,
                truncated: false,
                timed_out: false,
            })
            .await;
        Ok(SandboxExecutionOutput {
            stdout: line,
            stderr: String::new(),
            exit_code: self.exit_code,
            timed_out: false,
        })
    }

    async fn shutdown(&self) {}
}
