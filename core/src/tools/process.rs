//! Bounded process-output capture utilities.

use super::MAX_OUTPUT_SIZE;
use crate::workspace::{CommandOutputObserver, CommandOutputSummary};
use std::collections::VecDeque;
use std::io;
use std::process::ExitStatus;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

const READ_CHUNK_BYTES: usize = 8 * 1024;
const OUTPUT_HEAD_BYTES: usize = 64 * 1024;
const PROCESS_SETTLEMENT_MS: u64 = 500;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
            tail: VecDeque::with_capacity(MAX_OUTPUT_SIZE - OUTPUT_HEAD_BYTES),
            total_bytes: 0,
            stdout_bytes: 0,
            stderr_bytes: 0,
        }
    }

    fn push(&mut self, stream: OutputStream, bytes: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        match stream {
            OutputStream::Stdout => {
                self.stdout_bytes = self.stdout_bytes.saturating_add(bytes.len());
            }
            OutputStream::Stderr => {
                self.stderr_bytes = self.stderr_bytes.saturating_add(bytes.len());
            }
        }
        let head_remaining = OUTPUT_HEAD_BYTES.saturating_sub(self.head.len());
        let head_bytes = head_remaining.min(bytes.len());
        self.head
            .extend(bytes[..head_bytes].iter().map(|byte| CapturedByte {
                stream,
                byte: *byte,
            }));

        let tail_limit = MAX_OUTPUT_SIZE - OUTPUT_HEAD_BYTES;
        self.tail
            .extend(bytes[head_bytes..].iter().map(|byte| CapturedByte {
                stream,
                byte: *byte,
            }));
        while self.tail.len() > tail_limit {
            self.tail.pop_front();
        }
    }

    fn summary(&self, timed_out: bool) -> CommandOutputSummary {
        CommandOutputSummary {
            total_bytes: self.total_bytes,
            captured_bytes: self.head.len() + self.tail.len(),
            truncated: self.total_bytes > MAX_OUTPUT_SIZE,
            timed_out,
        }
    }

    fn render_combined(&self) -> String {
        let mut rendered = String::new();
        append_captured_bytes(&mut rendered, self.head.iter().copied());
        if self.total_bytes > MAX_OUTPUT_SIZE {
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
            .filter(|captured| captured.stream == stream)
            .collect::<Vec<_>>();
        let tail = self
            .tail
            .iter()
            .copied()
            .filter(|captured| captured.stream == stream)
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
                "\n\n[command {label} truncated by the global output limit: retained the first {} \
                 and last {} of {} bytes]\n\n",
                head.len(),
                tail.len(),
                total_bytes
            ));
        }
        append_captured_bytes(&mut rendered, tail.iter().copied());
        rendered
    }
}

fn append_captured_bytes(rendered: &mut String, captured: impl IntoIterator<Item = CapturedByte>) {
    let bytes = captured
        .into_iter()
        .map(|captured| captured.byte)
        .collect::<Vec<_>>();
    rendered.push_str(&String::from_utf8_lossy(&bytes));
}

#[derive(Debug)]
pub(crate) struct CapturedProcessOutput {
    pub(crate) combined: String,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) status: Option<ExitStatus>,
    pub(crate) timed_out: bool,
}

/// Configure a child as the leader of its own process group when supported.
pub(crate) fn configure_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    #[cfg(not(unix))]
    let _ = command;
}

/// Configure a blocking child as the leader of its own process group when
/// supported. Blocking workspace discovery commands use the same cancellation
/// semantics as Tokio-managed tools and language servers.
pub(crate) fn configure_std_process_group(command: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(not(unix))]
    let _ = command;
}

pub(crate) struct ProcessGroupGuard {
    #[cfg(unix)]
    process_group: Option<i32>,
}

impl ProcessGroupGuard {
    pub(crate) fn for_child(child: &Child) -> Self {
        Self::for_process_id(child.id())
    }

    pub(crate) fn for_process_id(process_id: Option<u32>) -> Self {
        #[cfg(not(unix))]
        let _ = process_id;
        Self {
            #[cfg(unix)]
            process_group: process_id.and_then(|id| i32::try_from(id).ok()),
        }
    }

    pub(crate) fn kill(&mut self) {
        #[cfg(unix)]
        {
            if let Some(process_group) = self.process_group.take() {
                // A negative PID addresses the whole group, including
                // grandchildren spawned by the language server or shell.
                unsafe {
                    libc::kill(-process_group, libc::SIGKILL);
                }
            }
        }
    }

    pub(crate) fn disarm(&mut self) {
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

pub(crate) async fn read_process_output(
    child: &mut Child,
    timeout_ms: u64,
    observer: Option<&dyn CommandOutputObserver>,
) -> io::Result<CapturedProcessOutput> {
    let mut stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            return Err(io::Error::other("child stdout was not piped"));
        }
    };
    let mut stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            return Err(io::Error::other("child stderr was not piped"));
        }
    };

    let mut process_group = ProcessGroupGuard::for_child(child);
    let mut capture = BoundedCapture::new();
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut stdout_buffer = vec![0_u8; READ_CHUNK_BYTES];
    let mut stderr_buffer = vec![0_u8; READ_CHUNK_BYTES];

    let execution = tokio::time::timeout(tokio::time::Duration::from_millis(timeout_ms), async {
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

    let (status, timed_out, wait_error) = match execution {
        Ok(Ok(status)) => {
            process_group.disarm();
            (Some(status), false, None)
        }
        Ok(Err(error)) => (None, false, Some(error)),
        Err(_) => {
            process_group.kill();
            child.start_kill().ok();
            let status = match tokio::time::timeout(
                tokio::time::Duration::from_millis(PROCESS_SETTLEMENT_MS),
                child.wait(),
            )
            .await
            {
                Ok(Ok(status)) => Some(status),
                Ok(Err(_)) | Err(_) => None,
            };
            (status, true, None)
        }
    };

    let summary = capture.summary(timed_out);
    if let Some(observer) = observer {
        observer.on_output_complete(&summary).await;
    }
    if let Some(error) = wait_error {
        return Err(error);
    }
    Ok(CapturedProcessOutput {
        combined: capture.render_combined(),
        stdout: capture.render_stream(OutputStream::Stdout),
        stderr: capture.render_stream(OutputStream::Stderr),
        status,
        timed_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_capture_keeps_head_and_tail_with_exact_accounting() {
        let mut capture = BoundedCapture::new();
        let input = (0..(MAX_OUTPUT_SIZE + 1234))
            .map(|index| b'a' + (index % 26) as u8)
            .collect::<Vec<_>>();
        capture.push(OutputStream::Stdout, &input);

        let summary = capture.summary(false);
        assert_eq!(summary.total_bytes, input.len());
        assert_eq!(summary.captured_bytes, MAX_OUTPUT_SIZE);
        assert!(summary.truncated);
        let rendered = capture.render_combined();
        assert!(rendered.contains("command output truncated"));
        let expected_head = String::from_utf8_lossy(&input[..OUTPUT_HEAD_BYTES]);
        let expected_tail =
            String::from_utf8_lossy(&input[input.len() - (MAX_OUTPUT_SIZE - OUTPUT_HEAD_BYTES)..]);
        assert!(rendered.starts_with(expected_head.as_ref()));
        assert!(rendered.ends_with(expected_tail.as_ref()));
        let stdout = capture.render_stream(OutputStream::Stdout);
        assert!(stdout.contains("command stdout truncated by the global output limit"));
        assert!(stdout.starts_with(expected_head.as_ref()));
        assert!(stdout.ends_with(expected_tail.as_ref()));
        assert!(capture.render_stream(OutputStream::Stderr).is_empty());
    }

    #[test]
    fn bounded_capture_preserves_stream_identity_under_one_global_limit() {
        let mut capture = BoundedCapture::new();
        let stdout = vec![b'o'; 70 * 1024];
        let stderr = vec![b'e'; 70 * 1024];
        capture.push(OutputStream::Stdout, &stdout);
        capture.push(OutputStream::Stderr, &stderr);

        let summary = capture.summary(false);
        assert_eq!(summary.total_bytes, stdout.len() + stderr.len());
        assert_eq!(summary.captured_bytes, MAX_OUTPUT_SIZE);
        assert!(summary.truncated);

        let rendered_stdout = capture.render_stream(OutputStream::Stdout);
        let rendered_stderr = capture.render_stream(OutputStream::Stderr);
        assert!(!rendered_stdout.contains("eeee"));
        assert!(!rendered_stderr.contains("oooo"));
        assert!(rendered_stdout.contains("stdout truncated"));
        assert!(rendered_stderr.contains("stderr truncated"));
    }

    #[cfg(unix)]
    fn spawn_test_shell(directory: &std::path::Path, script: &str) -> Child {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(script)
            .current_dir(directory)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        configure_process_group(&mut command);
        command.spawn().unwrap()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_deadline_includes_wait_after_both_streams_close() {
        let directory = tempfile::tempdir().unwrap();
        let mut child = spawn_test_shell(
            directory.path(),
            "exec 1>&- 2>&-; sleep 0.30; touch timeout-leak",
        );
        let started = std::time::Instant::now();

        let output = read_process_output(&mut child, 50, None).await.unwrap();

        assert!(output.timed_out);
        assert!(started.elapsed() < std::time::Duration::from_millis(250));
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        assert!(
            !directory.path().join("timeout-leak").exists(),
            "a process that closes its streams must not outlive its command deadline"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_capture_kills_descendants_in_the_process_group() {
        let directory = tempfile::tempdir().unwrap();
        let mut child = spawn_test_shell(
            directory.path(),
            "exec 1>&- 2>&-; \
             (touch descendant-started; sleep 0.30; touch cancellation-leak) & wait",
        );
        let capture =
            tokio::spawn(async move { read_process_output(&mut child, 5_000, None).await });

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !directory.path().join("descendant-started").exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("test process did not start");
        capture.abort();
        assert!(capture.await.unwrap_err().is_cancelled());

        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        assert!(
            !directory.path().join("cancellation-leak").exists(),
            "dropping process capture must kill every descendant"
        );
    }
}
