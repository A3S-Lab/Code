//! Process-host Bash sandbox for environments that already provide an outer
//! isolation boundary (Harbor task containers, CI job VMs).
//!
//! Unlike [`super::native::NativeBashSandbox`], this backend does not require
//! bubblewrap / Seatbelt. It still owns process-group termination and bounded
//! output capture so tool contracts stay intact. It is never the silent
//! default: hosts must opt in via
//! [`crate::SessionOptions::with_allow_process_host_sandbox`] or
//! `A3S_CODE_ALLOW_PROCESS_HOST_SANDBOX=1` when the native backend cannot
//! initialize.

use super::{BashSandbox, SandboxCommandRequest, SandboxExecutionOutput, SandboxOutput};
use crate::tools::MAX_OUTPUT_SIZE;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

const MAX_CAPTURE_BYTES: usize = MAX_OUTPUT_SIZE;
const OUTPUT_HEAD_BYTES: usize = 64 * 1024;
const READ_CHUNK_BYTES: usize = 8 * 1024;
const PROCESS_SETTLEMENT_MS: u64 = 500;

/// Run Bash on the host process with process-group and output bounds.
///
/// Use only when an outer container / VM already isolates the workspace from
/// the developer machine. This is the Harbor Terminal-Bench boundary.
#[derive(Debug)]
pub struct ProcessHostBashSandbox {
    workspace: PathBuf,
    deadline: Option<Instant>,
}

impl ProcessHostBashSandbox {
    /// Create a process-host sandbox rooted at `workspace`.
    ///
    /// `deadline`, when set, caps every command against a shared run budget
    /// (Terminal-Bench outer timeout).
    pub fn new(workspace: impl Into<PathBuf>, deadline: Option<Instant>) -> Self {
        Self {
            workspace: workspace.into(),
            deadline,
        }
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    fn remaining_timeout_ms(&self, requested_ms: u64) -> u64 {
        let Some(deadline) = self.deadline else {
            return requested_ms;
        };
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        requested_ms.min(remaining)
    }
}

#[async_trait]
impl BashSandbox for ProcessHostBashSandbox {
    async fn exec_command(&self, command: &str, guest_workspace: &str) -> Result<SandboxOutput> {
        let output = self
            .exec(SandboxCommandRequest {
                command: command.to_string(),
                guest_workspace: guest_workspace.to_string(),
                timeout_ms: 120_000,
                output_observer: None,
                env: None,
            })
            .await?;
        Ok(SandboxOutput {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.exit_code,
        })
    }

    async fn exec(&self, request: SandboxCommandRequest) -> Result<SandboxExecutionOutput> {
        let timeout_ms = self.remaining_timeout_ms(request.timeout_ms);
        if timeout_ms == 0 {
            let output = SandboxExecutionOutput {
                stdout: String::new(),
                stderr: "command skipped because the run deadline expired\n".to_string(),
                exit_code: 124,
                timed_out: true,
            };
            if let Some(observer) = request.output_observer {
                observer.on_output_delta(&output.stderr).await;
                observer
                    .on_output_complete(&crate::workspace::CommandOutputSummary {
                        total_bytes: output.stderr.len(),
                        captured_bytes: output.stderr.len(),
                        truncated: false,
                        timed_out: true,
                    })
                    .await;
            }
            return Ok(output);
        }

        let mut shell = Command::new("bash");
        shell
            .arg("-lc")
            .arg(&request.command)
            .current_dir(&self.workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        {
            // The shell is the process-group leader. A timeout therefore
            // terminates descendants spawned by scripts, package managers, or
            // test runners instead of leaving them behind in the outer job.
            shell.process_group(0);
        }
        if let Some(env) = request.env.as_deref() {
            shell.envs(env);
        }
        let mut child = shell.spawn().context("spawn process-host bash")?;
        let captured =
            capture_process_output(&mut child, timeout_ms, request.output_observer.as_deref())
                .await
                .context("capture process-host bash output")?;
        let exit_code = if captured.timed_out {
            124
        } else {
            captured
                .status
                .and_then(|status| status.code())
                .unwrap_or(1)
        };
        Ok(SandboxExecutionOutput {
            stdout: captured.stdout,
            stderr: captured.stderr,
            exit_code,
            timed_out: captured.timed_out,
        })
    }

    async fn shutdown(&self) {}
}

#[derive(Clone, Copy)]
enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Copy)]
struct CapturedByte {
    stream: OutputStream,
    byte: u8,
}

struct BoundedCapture {
    head: Vec<CapturedByte>,
    tail: VecDeque<CapturedByte>,
    total_bytes: usize,
    stdout_bytes: usize,
    stderr_bytes: usize,
}

impl BoundedCapture {
    fn new() -> Self {
        Self {
            head: Vec::with_capacity(OUTPUT_HEAD_BYTES),
            tail: VecDeque::with_capacity(MAX_CAPTURE_BYTES - OUTPUT_HEAD_BYTES),
            total_bytes: 0,
            stdout_bytes: 0,
            stderr_bytes: 0,
        }
    }

    fn push(&mut self, stream: OutputStream, bytes: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        match stream {
            OutputStream::Stdout => {
                self.stdout_bytes = self.stdout_bytes.saturating_add(bytes.len())
            }
            OutputStream::Stderr => {
                self.stderr_bytes = self.stderr_bytes.saturating_add(bytes.len())
            }
        }

        let head_remaining = OUTPUT_HEAD_BYTES.saturating_sub(self.head.len());
        let head_bytes = head_remaining.min(bytes.len());
        self.head
            .extend(bytes[..head_bytes].iter().map(|byte| CapturedByte {
                stream,
                byte: *byte,
            }));
        self.tail
            .extend(bytes[head_bytes..].iter().map(|byte| CapturedByte {
                stream,
                byte: *byte,
            }));
        while self.tail.len() > MAX_CAPTURE_BYTES - OUTPUT_HEAD_BYTES {
            self.tail.pop_front();
        }
    }

    fn summary(&self, timed_out: bool) -> crate::workspace::CommandOutputSummary {
        crate::workspace::CommandOutputSummary {
            total_bytes: self.total_bytes,
            captured_bytes: self.head.len() + self.tail.len(),
            truncated: self.total_bytes > MAX_CAPTURE_BYTES,
            timed_out,
        }
    }

    #[cfg(test)]
    fn render_combined(&self) -> String {
        let mut rendered = String::new();
        append_captured_bytes(&mut rendered, self.head.iter().copied());
        if self.total_bytes > MAX_CAPTURE_BYTES {
            rendered.push_str(&format!(
                "\n\n[command output truncated: retained the first {} and last {} of {} bytes]\n\n",
                self.head.len(),
                self.tail.len(),
                self.total_bytes
            ));
        }
        append_captured_bytes(&mut rendered, self.tail.iter().copied());
        rendered
    }

    fn render_stream(&self, stream: OutputStream) -> String {
        let head = self
            .head
            .iter()
            .copied()
            .filter(|captured| matches_stream(captured.stream, stream))
            .collect::<Vec<_>>();
        let tail = self
            .tail
            .iter()
            .copied()
            .filter(|captured| matches_stream(captured.stream, stream))
            .collect::<Vec<_>>();
        let total_bytes = match stream {
            OutputStream::Stdout => self.stdout_bytes,
            OutputStream::Stderr => self.stderr_bytes,
        };
        let mut rendered = String::new();
        append_captured_bytes(&mut rendered, head.iter().copied());
        if total_bytes > head.len() + tail.len() {
            let label = match stream {
                OutputStream::Stdout => "stdout",
                OutputStream::Stderr => "stderr",
            };
            rendered.push_str(&format!(
                "\n\n[command {label} truncated by the global output limit: retained the first {} and last {} of {} bytes]\n\n",
                head.len(),
                tail.len(),
                total_bytes
            ));
        }
        append_captured_bytes(&mut rendered, tail.iter().copied());
        rendered
    }
}

fn matches_stream(left: OutputStream, right: OutputStream) -> bool {
    matches!(
        (left, right),
        (OutputStream::Stdout, OutputStream::Stdout) | (OutputStream::Stderr, OutputStream::Stderr)
    )
}

fn append_captured_bytes(rendered: &mut String, captured: impl IntoIterator<Item = CapturedByte>) {
    let bytes = captured
        .into_iter()
        .map(|captured| captured.byte)
        .collect::<Vec<_>>();
    rendered.push_str(&String::from_utf8_lossy(&bytes));
}

struct CapturedProcessOutput {
    stdout: String,
    stderr: String,
    status: Option<std::process::ExitStatus>,
    timed_out: bool,
}

async fn capture_process_output(
    child: &mut Child,
    timeout_ms: u64,
    observer: Option<&dyn crate::workspace::CommandOutputObserver>,
) -> std::io::Result<CapturedProcessOutput> {
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("child stdout was not piped"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("child stderr was not piped"))?;
    let mut process_group = ProcessGroupGuard::for_child(child);
    let mut capture = BoundedCapture::new();
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut stdout_buffer = vec![0_u8; READ_CHUNK_BYTES];
    let mut stderr_buffer = vec![0_u8; READ_CHUNK_BYTES];

    let execution = tokio::time::timeout(Duration::from_millis(timeout_ms.max(1)), async {
        while !stdout_done || !stderr_done {
            tokio::select! {
                read = stdout.read(&mut stdout_buffer), if !stdout_done => {
                    match read {
                        Ok(0) => stdout_done = true,
                        Ok(count) => {
                            let bytes = &stdout_buffer[..count];
                            capture.push(OutputStream::Stdout, bytes);
                            if let Some(observer) = observer {
                                observer.on_output_delta(&String::from_utf8_lossy(bytes)).await;
                            }
                        }
                        Err(error) => {
                            let message = format!("\n[failed to read command stdout: {error}]\n");
                            capture.push(OutputStream::Stderr, message.as_bytes());
                            stdout_done = true;
                        }
                    }
                }
                read = stderr.read(&mut stderr_buffer), if !stderr_done => {
                    match read {
                        Ok(0) => stderr_done = true,
                        Ok(count) => {
                            let bytes = &stderr_buffer[..count];
                            capture.push(OutputStream::Stderr, bytes);
                            if let Some(observer) = observer {
                                observer.on_output_delta(&String::from_utf8_lossy(bytes)).await;
                            }
                        }
                        Err(error) => {
                            let message = format!("\n[failed to read command stderr: {error}]\n");
                            capture.push(OutputStream::Stderr, message.as_bytes());
                            stderr_done = true;
                        }
                    }
                }
            }
        }
        child.wait().await
    })
    .await;

    let (status, timed_out) = match execution {
        Ok(status) => {
            process_group.disarm();
            (Some(status?), false)
        }
        Err(_) => {
            process_group.kill();
            child.start_kill().ok();
            let status = match tokio::time::timeout(
                Duration::from_millis(PROCESS_SETTLEMENT_MS),
                child.wait(),
            )
            .await
            {
                Ok(Ok(status)) => Some(status),
                Ok(Err(_)) | Err(_) => None,
            };
            (status, true)
        }
    };

    let summary = capture.summary(timed_out);
    if let Some(observer) = observer {
        observer.on_output_complete(&summary).await;
    }
    Ok(CapturedProcessOutput {
        stdout: capture.render_stream(OutputStream::Stdout),
        stderr: capture.render_stream(OutputStream::Stderr),
        status,
        timed_out,
    })
}

struct ProcessGroupGuard {
    #[cfg(unix)]
    process_group: Option<i32>,
}

impl ProcessGroupGuard {
    fn for_child(child: &Child) -> Self {
        #[cfg(unix)]
        {
            Self {
                process_group: child.id().and_then(|id| i32::try_from(id).ok()),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = child;
            Self {}
        }
    }

    fn kill(&mut self) {
        #[cfg(unix)]
        if let Some(process_group) = self.process_group.take() {
            let _ = unsafe { libc::kill(-process_group, libc::SIGKILL) };
        }
    }

    fn disarm(&mut self) {
        #[cfg(unix)]
        {
            self.process_group = None;
        }
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_capture_keeps_global_memory_with_head_and_tail() {
        let mut capture = BoundedCapture::new();
        let input = vec![b'x'; MAX_CAPTURE_BYTES + 1];
        capture.push(OutputStream::Stdout, &input);
        assert_eq!(capture.head.len() + capture.tail.len(), MAX_CAPTURE_BYTES);
        assert_eq!(capture.total_bytes, MAX_CAPTURE_BYTES + 1);
        assert!(capture.summary(false).truncated);
        assert!(capture
            .render_combined()
            .contains("command output truncated"));
    }

    #[tokio::test]
    async fn high_volume_output_is_bounded_before_returning_to_the_tool() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let sandbox = ProcessHostBashSandbox::new(
            directory.path().to_path_buf(),
            Some(Instant::now() + Duration::from_secs(5)),
        );
        let output = sandbox
            .exec(SandboxCommandRequest {
                command: "yes x | head -c 200000".to_string(),
                guest_workspace: directory.path().to_string_lossy().into_owned(),
                timeout_ms: 1_000,
                output_observer: None,
                env: None,
            })
            .await
            .expect("sandbox execution");
        assert!(!output.timed_out);
        assert!(output.stdout.contains("command stdout truncated"));
        assert!(output.stdout.len() <= MAX_CAPTURE_BYTES + 256);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_process_group_after_streams_close() {
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
