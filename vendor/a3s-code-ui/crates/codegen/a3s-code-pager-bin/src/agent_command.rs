use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use a3s_code_pager::agent_runtime::AgentRuntime;
use a3s_code_pager::signal_streams::SignalStreams;
use a3s_code_shell::agent::config::Config;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

use crate::shutdown_and_flush_telemetry;

const STDIO_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) struct AgentSignals {
    defer_exit: Arc<AtomicBool>,
    received: oneshot::Receiver<i32>,
    cancel: CancellationToken,
    _listener: AbortOnDropHandle<()>,
}

impl Drop for AgentSignals {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub(crate) fn spawn_signal_flush() -> AgentSignals {
    let defer_exit = Arc::new(AtomicBool::new(false));
    let defer_exit_for_listener = Arc::clone(&defer_exit);
    let (sender, received) = oneshot::channel();
    let cancel = CancellationToken::new();
    let cancelled = cancel.clone();
    let mut streams = SignalStreams::install();
    // Signal observation must remain independent of synchronous agent startup
    let listener = AbortOnDropHandle::new(tokio::spawn(async move {
        let code = tokio::select! {
            () = cancelled.cancelled() => return,
            code = streams.next_code() => code,
        };
        if !defer_exit_for_listener.load(Ordering::Acquire) || sender.send(code).is_err() {
            shutdown_and_flush_telemetry(code);
        }
        // A graceful teardown gets one timer; a second signal ends it now
        let code = tokio::select! {
            () = cancelled.cancelled() => return,
            () = tokio::time::sleep(STDIO_SHUTDOWN_TIMEOUT) => code,
            again = streams.next_code() => again,
        };
        shutdown_and_flush_telemetry(code);
    }));
    AgentSignals {
        defer_exit,
        received,
        cancel,
        _listener: listener,
    }
}

pub(crate) async fn run_stdio(
    runtime: &AgentRuntime,
    agent_config: &Config,
    mut signals: AgentSignals,
) -> anyhow::Result<()> {
    signals.defer_exit.store(true, Ordering::Release);

    let outcome = tokio::select! {
        biased;
        code = &mut signals.received => code.map(Some).map_err(anyhow::Error::from),
        result = runtime.run_stdio(agent_config) => result.map(|()| None),
    };
    if let Some(exit_code) = outcome? {
        shutdown_and_flush_telemetry(exit_code);
    }
    Ok(())
}
