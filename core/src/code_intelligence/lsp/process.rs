//! Long-lived language server process lifecycle.

use super::client::LspClient;
use super::router::ServerRequestRouter;
use crate::code_intelligence::language_profile::LanguageServerCommand;
use crate::tools::process::{configure_process_group, ProcessGroupGuard};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const STDERR_LIMIT_BYTES: usize = 64 * 1024;
const STDERR_SETTLEMENT_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LspProcessState {
    Running,
    Exited { code: Option<i32>, forced: bool },
    Failed { message: String },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LspProcessError {
    #[error("failed to start language server {program:?}: {source}")]
    Spawn {
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("language server process did not expose piped {stream}")]
    MissingPipe { stream: &'static str },

    #[error("language server did not report an exit within {duration:?}")]
    ShutdownIncomplete { duration: Duration },
}

enum ProcessControl {
    ForceKill,
}

/// Handle to one process and its protocol client.
pub(crate) struct LspProcess {
    client: LspClient,
    control: mpsc::UnboundedSender<ProcessControl>,
    state: watch::Receiver<LspProcessState>,
    stderr: Arc<StdMutex<BoundedStderr>>,
    shutdown_started: AtomicBool,
}

impl std::fmt::Debug for LspProcess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LspProcess")
            .field("state", &*self.state.borrow())
            .field("client_closed", &self.client.is_closed())
            .finish()
    }
}

impl LspProcess {
    pub(crate) fn spawn(
        command: &LanguageServerCommand,
        working_directory: &Path,
        router: ServerRequestRouter,
    ) -> Result<Self, LspProcessError> {
        let mut process = Command::new(&command.program);
        process
            .args(&command.args)
            .envs(&command.env)
            .current_dir(working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        configure_process_group(&mut process);

        let mut child = process.spawn().map_err(|source| LspProcessError::Spawn {
            program: command.program.clone(),
            source,
        })?;
        let process_group = ProcessGroupGuard::for_child(&child);
        let stdout = child
            .stdout
            .take()
            .ok_or(LspProcessError::MissingPipe { stream: "stdout" })?;
        let stdin = child
            .stdin
            .take()
            .ok_or(LspProcessError::MissingPipe { stream: "stdin" })?;
        let stderr = child
            .stderr
            .take()
            .ok_or(LspProcessError::MissingPipe { stream: "stderr" })?;

        let client = LspClient::start_split(stdout, stdin, router);
        let protocol_closed = client.shutdown_token();
        let stderr_buffer = Arc::new(StdMutex::new(BoundedStderr::new(STDERR_LIMIT_BYTES)));
        let stderr_task = tokio::spawn(read_stderr(stderr, Arc::clone(&stderr_buffer)));
        let (control, control_rx) = mpsc::unbounded_channel();
        let (state_tx, state) = watch::channel(LspProcessState::Running);
        tokio::spawn(monitor_process(
            child,
            process_group,
            client.clone(),
            protocol_closed,
            control_rx,
            state_tx,
            stderr_task,
        ));

        Ok(Self {
            client,
            control,
            state,
            stderr: stderr_buffer,
            shutdown_started: AtomicBool::new(false),
        })
    }

    pub(crate) fn client(&self) -> LspClient {
        self.client.clone()
    }

    pub(crate) fn state(&self) -> LspProcessState {
        self.state.borrow().clone()
    }

    pub(crate) fn subscribe_state(&self) -> watch::Receiver<LspProcessState> {
        self.state.clone()
    }

    pub(crate) fn stderr_snapshot(&self) -> String {
        self.stderr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .render()
    }

    /// Request a graceful protocol shutdown, then kill the process group if
    /// the server does not exit within the supplied bounds.
    pub(crate) async fn shutdown(
        &self,
        request_timeout: Duration,
        exit_timeout: Duration,
    ) -> Result<LspProcessState, LspProcessError> {
        if !self.shutdown_started.swap(true, Ordering::AcqRel)
            && matches!(self.state(), LspProcessState::Running)
        {
            let _ = self
                .client
                .request("shutdown", None, CancellationToken::new(), request_timeout)
                .await;
            // A saturated writer queue must not make host shutdown
            // unbounded after the graceful request has already timed out.
            let _ = tokio::time::timeout(request_timeout, self.client.notify("exit", None)).await;
        }

        if let Some(state) = wait_for_terminal_state(self.state.clone(), exit_timeout).await {
            return Ok(state);
        }

        let _ = self.control.send(ProcessControl::ForceKill);
        wait_for_terminal_state(self.state.clone(), exit_timeout)
            .await
            .ok_or(LspProcessError::ShutdownIncomplete {
                duration: exit_timeout,
            })
    }

    pub(crate) fn force_kill(&self) {
        self.shutdown_started.store(true, Ordering::Release);
        let _ = self.control.send(ProcessControl::ForceKill);
    }
}

impl Drop for LspProcess {
    fn drop(&mut self) {
        if matches!(self.state(), LspProcessState::Running) {
            let _ = self.control.send(ProcessControl::ForceKill);
        }
    }
}

async fn monitor_process(
    mut child: Child,
    mut process_group: ProcessGroupGuard,
    client: LspClient,
    protocol_closed: CancellationToken,
    mut control: mpsc::UnboundedReceiver<ProcessControl>,
    state: watch::Sender<LspProcessState>,
    mut stderr_task: JoinHandle<()>,
) {
    let (forced, result) = tokio::select! {
        result = child.wait() => (false, result),
        _ = protocol_closed.cancelled() => {
            match child.try_wait() {
                Ok(Some(status)) => (false, Ok(status)),
                Ok(None) => {
                    // A language server can close its protocol streams without
                    // exiting. Reap that generation before the workspace is
                    // allowed to start a replacement process.
                    process_group.kill();
                    let _ = child.start_kill();
                    (true, child.wait().await)
                }
                Err(error) => (false, Err(error)),
            }
        }
        command = control.recv() => {
            match command {
                Some(ProcessControl::ForceKill) | None => {
                    process_group.kill();
                    let _ = child.start_kill();
                    (true, child.wait().await)
                }
            }
        }
    };

    // A server can leave helper children behind even after its leader exits.
    process_group.kill();
    let final_state = match result {
        Ok(status) => LspProcessState::Exited {
            code: status.code(),
            forced,
        },
        Err(error) => LspProcessState::Failed {
            message: error.to_string(),
        },
    };
    let _ = state.send(final_state);
    client.close().await;

    if tokio::time::timeout(STDERR_SETTLEMENT_TIMEOUT, &mut stderr_task)
        .await
        .is_err()
    {
        stderr_task.abort();
    }
}

async fn wait_for_terminal_state(
    mut state: watch::Receiver<LspProcessState>,
    timeout: Duration,
) -> Option<LspProcessState> {
    if !matches!(*state.borrow(), LspProcessState::Running) {
        return Some(state.borrow().clone());
    }

    tokio::time::timeout(timeout, async {
        loop {
            state.changed().await.ok()?;
            if !matches!(*state.borrow(), LspProcessState::Running) {
                return Some(state.borrow().clone());
            }
        }
    })
    .await
    .ok()
    .flatten()
}

async fn read_stderr(
    mut stderr: tokio::process::ChildStderr,
    buffer: Arc<StdMutex<BoundedStderr>>,
) {
    let mut chunk = [0_u8; 4096];
    loop {
        match stderr.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(count) => buffer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(&chunk[..count]),
        }
    }
}

#[derive(Debug)]
struct BoundedStderr {
    bytes: VecDeque<u8>,
    limit: usize,
    total_bytes: usize,
}

impl BoundedStderr {
    fn new(limit: usize) -> Self {
        Self {
            bytes: VecDeque::with_capacity(limit),
            limit,
            total_bytes: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        self.bytes.extend(bytes.iter().copied());
        while self.bytes.len() > self.limit {
            self.bytes.pop_front();
        }
    }

    fn render(&self) -> String {
        let bytes = self.bytes.iter().copied().collect::<Vec<_>>();
        let stderr = String::from_utf8_lossy(&bytes);
        if self.total_bytes <= self.bytes.len() {
            stderr.into_owned()
        } else {
            format!(
                "[language server stderr truncated: retained the last {} of {} bytes]\n{}",
                self.bytes.len(),
                self.total_bytes,
                stderr
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_intelligence::lsp::router::{ServerRequestRouter, ServerRequestRouterConfig};
    use std::collections::BTreeMap;
    #[cfg(unix)]
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::time::Instant;

    #[test]
    fn bounded_stderr_retains_only_the_tail() {
        let mut stderr = BoundedStderr::new(5);
        stderr.push(b"abc");
        stderr.push(b"defgh");

        let rendered = stderr.render();
        assert!(rendered.contains("last 5 of 8 bytes"));
        assert!(rendered.ends_with("defgh"));
    }

    #[tokio::test]
    async fn missing_executable_is_a_typed_spawn_error() {
        let command = LanguageServerCommand {
            program: PathBuf::from("a3s-code-missing-language-server-executable"),
            args: Vec::new(),
            env: BTreeMap::new(),
        };
        let directory = tempfile::tempdir().unwrap();
        let error = LspProcess::spawn(
            &command,
            directory.path(),
            ServerRequestRouter::new(ServerRequestRouterConfig::default()),
        )
        .unwrap_err();

        assert!(matches!(error, LspProcessError::Spawn { .. }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_kills_an_unresponsive_process_group() {
        let command = LanguageServerCommand {
            program: PathBuf::from("sh"),
            args: vec![OsString::from("-c"), OsString::from("sleep 30")],
            env: BTreeMap::new(),
        };
        let directory = tempfile::tempdir().unwrap();
        let process = LspProcess::spawn(
            &command,
            directory.path(),
            ServerRequestRouter::new(ServerRequestRouterConfig::default()),
        )
        .unwrap();
        let started = Instant::now();

        let state = process
            .shutdown(Duration::from_millis(50), Duration::from_millis(500))
            .await
            .unwrap();

        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(matches!(
            state,
            LspProcessState::Exited { forced: true, .. }
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn protocol_eof_reaps_a_still_running_process() {
        let command = LanguageServerCommand {
            program: PathBuf::from("sh"),
            args: vec![OsString::from("-c"), OsString::from("exec 1>&-; sleep 30")],
            env: BTreeMap::new(),
        };
        let directory = tempfile::tempdir().unwrap();
        let process = LspProcess::spawn(
            &command,
            directory.path(),
            ServerRequestRouter::new(ServerRequestRouterConfig::default()),
        )
        .unwrap();

        let state = wait_for_terminal_state(process.subscribe_state(), Duration::from_secs(2))
            .await
            .expect("protocol EOF should force the live process to exit");

        assert!(matches!(
            state,
            LspProcessState::Exited { forced: true, .. }
        ));
    }
}
