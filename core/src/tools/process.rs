//! Bounded process-output capture utilities.

use super::MAX_OUTPUT_SIZE;
use crate::workspace::{CommandOutputObserver, CommandOutputSummary};
use std::collections::VecDeque;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

const READ_CHUNK_BYTES: usize = 8 * 1024;
const OUTPUT_HEAD_BYTES: usize = 64 * 1024;
const PROCESS_SETTLEMENT_MS: u64 = 500;

struct BoundedCapture {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total_bytes: usize,
}

impl BoundedCapture {
    fn new() -> Self {
        Self {
            head: Vec::with_capacity(OUTPUT_HEAD_BYTES),
            tail: VecDeque::with_capacity(MAX_OUTPUT_SIZE - OUTPUT_HEAD_BYTES),
            total_bytes: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        let head_remaining = OUTPUT_HEAD_BYTES.saturating_sub(self.head.len());
        let head_bytes = head_remaining.min(bytes.len());
        self.head.extend_from_slice(&bytes[..head_bytes]);

        let tail_limit = MAX_OUTPUT_SIZE - OUTPUT_HEAD_BYTES;
        self.tail.extend(bytes[head_bytes..].iter().copied());
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

    fn render(&self) -> String {
        let mut rendered = String::new();
        rendered.push_str(&String::from_utf8_lossy(&self.head));
        if self.total_bytes > MAX_OUTPUT_SIZE {
            rendered.push_str(&format!(
                "\n\n[command output truncated: retained the first {} and last {} of {} bytes]\n\n",
                self.head.len(),
                self.tail.len(),
                self.total_bytes
            ));
        }
        let tail = self.tail.iter().copied().collect::<Vec<_>>();
        rendered.push_str(&String::from_utf8_lossy(&tail));
        rendered
    }
}

/// Configure a child as the leader of its own process group when supported.
pub(crate) fn configure_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
}

pub(crate) struct ProcessGroupGuard {
    #[cfg(unix)]
    process_group: Option<i32>,
}

impl ProcessGroupGuard {
    pub(crate) fn for_child(child: &Child) -> Self {
        Self {
            #[cfg(unix)]
            process_group: child.id().and_then(|id| i32::try_from(id).ok()),
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
) -> (String, bool) {
    let mut stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            return ("Internal error: child stdout not piped".to_string(), false);
        }
    };
    let mut stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            return ("Internal error: child stderr not piped".to_string(), false);
        }
    };

    let mut process_group = ProcessGroupGuard::for_child(child);
    let mut capture = BoundedCapture::new();
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut stdout_buffer = vec![0_u8; READ_CHUNK_BYTES];
    let mut stderr_buffer = vec![0_u8; READ_CHUNK_BYTES];

    let timed_out = tokio::time::timeout(tokio::time::Duration::from_millis(timeout_ms), async {
        while !stdout_done || !stderr_done {
            tokio::select! {
                read = stdout.read(&mut stdout_buffer), if !stdout_done => {
                    match read {
                        Ok(0) => stdout_done = true,
                        Ok(count) => {
                            let bytes = &stdout_buffer[..count];
                            capture.push(bytes);
                            if let Some(observer) = observer {
                                observer.on_output_delta(&String::from_utf8_lossy(bytes)).await;
                            }
                        }
                        Err(error) => {
                            let message = format!("\n[failed to read command stdout: {error}]\n");
                            capture.push(message.as_bytes());
                            stdout_done = true;
                        }
                    }
                }
                read = stderr.read(&mut stderr_buffer), if !stderr_done => {
                    match read {
                        Ok(0) => stderr_done = true,
                        Ok(count) => {
                            let bytes = &stderr_buffer[..count];
                            capture.push(bytes);
                            if let Some(observer) = observer {
                                observer.on_output_delta(&String::from_utf8_lossy(bytes)).await;
                            }
                        }
                        Err(error) => {
                            let message = format!("\n[failed to read command stderr: {error}]\n");
                            capture.push(message.as_bytes());
                            stderr_done = true;
                        }
                    }
                }
            }
        }
    })
    .await
    .is_err();

    if timed_out {
        process_group.kill();
        child.start_kill().ok();
        let _ = tokio::time::timeout(
            tokio::time::Duration::from_millis(PROCESS_SETTLEMENT_MS),
            child.wait(),
        )
        .await;
    } else {
        process_group.disarm();
    }

    let summary = capture.summary(timed_out);
    if let Some(observer) = observer {
        observer.on_output_complete(&summary).await;
    }
    (capture.render(), timed_out)
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
        capture.push(&input);

        let summary = capture.summary(false);
        assert_eq!(summary.total_bytes, input.len());
        assert_eq!(summary.captured_bytes, MAX_OUTPUT_SIZE);
        assert!(summary.truncated);
        let rendered = capture.render();
        assert!(rendered.contains("command output truncated"));
        let expected_head = String::from_utf8_lossy(&input[..OUTPUT_HEAD_BYTES]);
        let expected_tail =
            String::from_utf8_lossy(&input[input.len() - (MAX_OUTPUT_SIZE - OUTPUT_HEAD_BYTES)..]);
        assert!(rendered.starts_with(expected_head.as_ref()));
        assert!(rendered.ends_with(expected_tail.as_ref()));
    }
}
