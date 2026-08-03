use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use futures::FutureExt;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::daemon::prepare_agent_dir;
use crate::agent_api::{Agent, SessionOptions};
use crate::config::AgentDir;
use crate::error::{CodeError, Result};

/// Default deadline for joined serve daemon shutdown.
pub const DEFAULT_SERVE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

/// Observable lifecycle phase for a filesystem-first serve daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ServeDaemonPhase {
    Starting,
    Ready,
    Draining,
    Stopped,
    Failed,
}

impl ServeDaemonPhase {
    /// Stable lowercase representation suitable for SDK and health boundaries.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Draining => "draining",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }

    /// Whether no more work can be accepted by this daemon.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Stopped | Self::Failed)
    }
}

/// Stable failure category for a serve daemon that reached a terminal failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ServeDaemonFailure {
    Startup,
    Runtime,
    Panic,
    ShutdownDeadline,
}

impl ServeDaemonFailure {
    /// Stable machine-readable code suitable for SDK error boundaries.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Startup => "SERVE_STARTUP_FAILED",
            Self::Runtime => "SERVE_RUNTIME_FAILED",
            Self::Panic => "SERVE_DAEMON_PANICKED",
            Self::ShutdownDeadline => "SERVE_SHUTDOWN_DEADLINE_EXCEEDED",
        }
    }
}

/// Point-in-time serve daemon status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServeDaemonStatus {
    /// Current lifecycle phase.
    pub phase: ServeDaemonPhase,
    /// Failure category when `phase` is [`ServeDaemonPhase::Failed`].
    pub failure: Option<ServeDaemonFailure>,
}

impl ServeDaemonStatus {
    const fn starting() -> Self {
        Self {
            phase: ServeDaemonPhase::Starting,
            failure: None,
        }
    }
}

struct ServeDaemonHandleInner {
    cancel: CancellationToken,
    status_tx: watch::Sender<ServeDaemonStatus>,
    status_rx: watch::Receiver<ServeDaemonStatus>,
    failure_detail: Arc<StdMutex<Option<String>>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
#[must_use = "keep the handle to observe the daemon and shut it down cleanly"]
pub struct ServeDaemonHandle {
    inner: Arc<ServeDaemonHandleInner>,
}

/// Spawn preparation and execution of a filesystem-first agent daemon.
///
/// The returned handle starts in [`ServeDaemonPhase::Starting`]. Call
/// [`ServeDaemonHandle::wait_ready`] before advertising readiness. Dropping the
/// handle does not cancel the daemon; call [`ServeDaemonHandle::stop`] and await
/// the result for bounded, joined shutdown.
pub fn spawn_agent_dir_daemon(
    agent: Arc<Agent>,
    agent_dir: AgentDir,
    workspace: impl Into<String>,
    extra: Option<SessionOptions>,
) -> Result<ServeDaemonHandle> {
    let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
        CodeError::Context("serve daemon requires an active Tokio runtime".to_string())
    })?;
    let workspace = workspace.into();
    let cancel = CancellationToken::new();
    let worker_cancel = cancel.clone();
    let (status_tx, status_rx) = watch::channel(ServeDaemonStatus::starting());
    let worker_status = status_tx.clone();
    let failure_detail = Arc::new(StdMutex::new(None));
    let worker_failure_detail = Arc::clone(&failure_detail);
    let task = runtime.spawn(async move {
        let outcome = AssertUnwindSafe(run_daemon(
            agent,
            agent_dir,
            workspace,
            extra,
            worker_cancel,
            worker_status.clone(),
        ))
        .catch_unwind()
        .await;
        match outcome {
            Ok(Ok(())) => publish(&worker_status, ServeDaemonPhase::Stopped, None),
            Ok(Err((failure, error))) => {
                *lock_failure_detail(&worker_failure_detail) = Some(error.to_string());
                publish(&worker_status, ServeDaemonPhase::Failed, Some(failure));
            }
            Err(_) => {
                *lock_failure_detail(&worker_failure_detail) =
                    Some("serve daemon panicked".to_string());
                publish(
                    &worker_status,
                    ServeDaemonPhase::Failed,
                    Some(ServeDaemonFailure::Panic),
                );
            }
        }
    });

    Ok(ServeDaemonHandle {
        inner: Arc::new(ServeDaemonHandleInner {
            cancel,
            status_tx,
            status_rx,
            failure_detail,
            task: Mutex::new(Some(task)),
        }),
    })
}

async fn run_daemon(
    agent: Arc<Agent>,
    agent_dir: AgentDir,
    workspace: String,
    extra: Option<SessionOptions>,
    cancel: CancellationToken,
    status: watch::Sender<ServeDaemonStatus>,
) -> std::result::Result<(), (ServeDaemonFailure, CodeError)> {
    let daemon = prepare_agent_dir(&agent, &agent_dir, workspace, extra)
        .await
        .map_err(|error| (ServeDaemonFailure::Startup, error))?;
    if cancel.is_cancelled() {
        return Ok(());
    }
    publish(&status, ServeDaemonPhase::Ready, None);
    daemon
        .run(cancel)
        .await
        .map_err(|error| (ServeDaemonFailure::Runtime, error))
}

fn publish(
    status: &watch::Sender<ServeDaemonStatus>,
    phase: ServeDaemonPhase,
    failure: Option<ServeDaemonFailure>,
) {
    status.send_replace(ServeDaemonStatus { phase, failure });
}

fn lock_failure_detail(
    detail: &StdMutex<Option<String>>,
) -> std::sync::MutexGuard<'_, Option<String>> {
    detail
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl ServeDaemonHandle {
    /// Return the latest lifecycle status without waiting.
    pub fn status(&self) -> ServeDaemonStatus {
        *self.inner.status_rx.borrow()
    }

    /// Whether preparation completed and the daemon currently accepts work.
    pub fn is_ready(&self) -> bool {
        self.status().phase == ServeDaemonPhase::Ready
    }

    /// Whether the daemon has stopped or failed.
    pub fn is_stopped(&self) -> bool {
        self.status().phase.is_terminal()
    }

    /// Return the stable terminal failure code, if any.
    pub fn failure_code(&self) -> Option<&'static str> {
        self.status().failure.map(ServeDaemonFailure::code)
    }

    /// Wait until preparation succeeds, or return the startup failure.
    pub async fn wait_ready(&self) -> Result<()> {
        let mut status = self.inner.status_rx.clone();
        loop {
            match *status.borrow_and_update() {
                ServeDaemonStatus {
                    phase: ServeDaemonPhase::Ready,
                    ..
                } => return Ok(()),
                ServeDaemonStatus {
                    phase: ServeDaemonPhase::Failed,
                    ..
                } => return Err(self.failure_error()),
                ServeDaemonStatus {
                    phase: ServeDaemonPhase::Draining | ServeDaemonPhase::Stopped,
                    ..
                } => {
                    return Err(CodeError::Context(
                        "serve daemon stopped before becoming ready".to_string(),
                    ));
                }
                ServeDaemonStatus {
                    phase: ServeDaemonPhase::Starting,
                    ..
                } => {}
            }
            status.changed().await.map_err(|_| {
                CodeError::Context("serve daemon status channel closed".to_string())
            })?;
        }
    }

    /// Request graceful shutdown and wait up to the default deadline.
    pub async fn stop(&self) -> Result<ServeDaemonStatus> {
        self.stop_with_timeout(DEFAULT_SERVE_SHUTDOWN_TIMEOUT).await
    }

    /// Request graceful shutdown and wait up to `deadline` for task settlement.
    pub async fn stop_with_timeout(&self, deadline: Duration) -> Result<ServeDaemonStatus> {
        if !self.status().phase.is_terminal() {
            self.inner.cancel.cancel();
            self.inner.status_tx.send_modify(|status| {
                if !status.phase.is_terminal() {
                    *status = ServeDaemonStatus {
                        phase: ServeDaemonPhase::Draining,
                        failure: None,
                    };
                }
            });
        }
        self.join(Some(deadline)).await
    }

    /// Wait for the daemon to terminate without requesting shutdown.
    pub async fn wait(&self) -> Result<ServeDaemonStatus> {
        self.join(None).await
    }

    async fn join(&self, deadline: Option<Duration>) -> Result<ServeDaemonStatus> {
        let mut task = self.inner.task.lock().await;
        if let Some(mut handle) = task.take() {
            let joined = match deadline {
                Some(deadline) => match tokio::time::timeout(deadline, &mut handle).await {
                    Ok(joined) => joined,
                    Err(_) => {
                        handle.abort();
                        let _ = handle.await;
                        self.mark_failure(
                            ServeDaemonFailure::ShutdownDeadline,
                            "serve daemon exceeded its shutdown deadline",
                        );
                        return Err(self.failure_error());
                    }
                },
                None => handle.await,
            };
            if joined.is_err() && self.status().phase != ServeDaemonPhase::Failed {
                self.mark_failure(ServeDaemonFailure::Panic, "serve daemon task failed");
            }
        }

        let status = self.status();
        if status.phase == ServeDaemonPhase::Failed {
            Err(self.failure_error())
        } else {
            Ok(status)
        }
    }

    fn mark_failure(&self, failure: ServeDaemonFailure, detail: &str) {
        *lock_failure_detail(&self.inner.failure_detail) = Some(detail.to_string());
        publish(
            &self.inner.status_tx,
            ServeDaemonPhase::Failed,
            Some(failure),
        );
    }

    fn failure_error(&self) -> CodeError {
        let code = self.failure_code().unwrap_or("unknown_failure");
        let detail = lock_failure_detail(&self.inner.failure_detail)
            .clone()
            .unwrap_or_else(|| "serve daemon failed".to_string());
        CodeError::Context(format!("serve daemon {code}: {detail}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CodeConfig, ScheduleSpec};
    use crate::prompts::SystemPromptSlots;

    fn test_agent_config() -> CodeConfig {
        CodeConfig::from_acl(
            r#"
default_model = "anthropic/claude-sonnet-4-20250514"
providers "anthropic" {
  api_key = "test-key"
  models "claude-sonnet-4-20250514" { name = "Claude Sonnet 4" }
}
"#,
        )
        .unwrap()
    }

    fn agent_dir_with(schedules: Vec<ScheduleSpec>) -> AgentDir {
        AgentDir {
            dir: std::path::PathBuf::from("/tmp/serve-lifecycle-test-agent"),
            config: CodeConfig::default(),
            prompt_slots: SystemPromptSlots::default(),
            schedules,
            tools: Vec::new(),
        }
    }

    #[tokio::test]
    async fn readiness_waits_for_preparation_and_stop_joins_the_daemon() {
        let agent = Arc::new(Agent::from_config(test_agent_config()).await.unwrap());
        let workspace = tempfile::tempdir().unwrap();
        let handle = spawn_agent_dir_daemon(
            agent,
            agent_dir_with(Vec::new()),
            workspace.path().to_string_lossy(),
            None,
        )
        .unwrap();

        tokio::time::timeout(Duration::from_secs(1), handle.wait_ready())
            .await
            .unwrap()
            .unwrap();
        assert!(handle.is_ready());

        let status = handle
            .stop_with_timeout(Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(status.phase, ServeDaemonPhase::Stopped);
        assert!(handle.is_stopped());
        assert_eq!(handle.failure_code(), None);
    }

    #[tokio::test]
    async fn startup_failure_is_typed_and_never_reports_ready() {
        let agent = Arc::new(Agent::from_config(test_agent_config()).await.unwrap());
        let workspace = tempfile::tempdir().unwrap();
        let handle = spawn_agent_dir_daemon(
            agent,
            agent_dir_with(vec![ScheduleSpec {
                name: "invalid".to_string(),
                cron: "not a cron".to_string(),
                prompt: "unused".to_string(),
                enabled: true,
            }]),
            workspace.path().to_string_lossy(),
            None,
        )
        .unwrap();

        assert!(
            tokio::time::timeout(Duration::from_secs(1), handle.wait_ready())
                .await
                .unwrap()
                .is_err()
        );
        assert!(!handle.is_ready());
        assert!(handle.is_stopped());
        assert_eq!(handle.failure_code(), Some("SERVE_STARTUP_FAILED"));
        assert!(handle.wait().await.is_err());
    }

    #[tokio::test]
    async fn shutdown_deadline_aborts_a_non_cooperative_worker() {
        let cancel = CancellationToken::new();
        let (status_tx, status_rx) = watch::channel(ServeDaemonStatus {
            phase: ServeDaemonPhase::Ready,
            failure: None,
        });
        let task = tokio::spawn(std::future::pending::<()>());
        let handle = ServeDaemonHandle {
            inner: Arc::new(ServeDaemonHandleInner {
                cancel,
                status_tx,
                status_rx,
                failure_detail: Arc::new(StdMutex::new(None)),
                task: Mutex::new(Some(task)),
            }),
        };

        assert!(handle
            .stop_with_timeout(Duration::from_millis(10))
            .await
            .is_err());
        assert_eq!(handle.status().phase, ServeDaemonPhase::Failed);
        assert_eq!(
            handle.failure_code(),
            Some("SERVE_SHUTDOWN_DEADLINE_EXCEEDED")
        );
    }
}
