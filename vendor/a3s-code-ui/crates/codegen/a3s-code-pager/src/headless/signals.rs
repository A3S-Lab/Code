//! Signals for the one-shot `a3s -p` run. The streams are installed before the agent starts,
//! because until they exist the default disposition kills the run instead of exiting `128 + signal`.

use std::io;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::{Notify, oneshot};

use crate::app::signal_handler;
use crate::signal_streams::SignalStreams;

#[derive(Clone, Copy, Debug, Default)]
enum ExitMode {
    #[default]
    Immediate,
    Deferred,
    Pending(i32),
}

#[derive(Debug, PartialEq, Eq)]
enum SignalRoute {
    Exit,
    Pending,
}

#[derive(Default)]
struct SignalHandoff {
    mode: Mutex<ExitMode>,
    parked: Notify,
}

#[derive(Debug, thiserror::Error)]
#[error("headless signal handling failed to start")]
pub(super) struct SignalSetupError(#[source] io::Error);

pub(super) struct HeadlessSignals {
    handoff: Arc<SignalHandoff>,
}

impl HeadlessSignals {
    pub(super) async fn install() -> Result<HeadlessSignals, SignalSetupError> {
        let handoff = Arc::new(SignalHandoff::default());
        spawn_watcher(SignalStreams::install, Arc::clone(&handoff), |code| {
            signal_handler::force_exit(code)
        })
        .await
        .map_err(SignalSetupError)?;
        Ok(HeadlessSignals { handoff })
    }

    pub(super) fn defer_exit(&self) -> DeferredExit<'_> {
        *self.handoff.mode.lock() = ExitMode::Deferred;
        DeferredExit {
            handoff: &self.handoff,
        }
    }
}

/// While held, a signal is parked for the turn instead of exiting, and only
/// [`DeferredExit::release`] can claim it.
#[must_use]
pub(super) struct DeferredExit<'a> {
    handoff: &'a SignalHandoff,
}

impl DeferredExit<'_> {
    /// Never resolves until a signal is parked, so it can sit in a `select!` arm.
    pub(super) async fn signalled(&self) {
        loop {
            let parked = self.handoff.parked.notified();
            if matches!(*self.handoff.mode.lock(), ExitMode::Pending(_)) {
                return;
            }
            parked.await;
        }
    }

    #[must_use]
    pub(super) fn release(self) -> Option<i32> {
        match std::mem::replace(&mut *self.handoff.mode.lock(), ExitMode::Immediate) {
            ExitMode::Pending(code) => Some(code),
            ExitMode::Immediate | ExitMode::Deferred => None,
        }
    }
}

#[must_use]
fn route_signal(handoff: &SignalHandoff, code: i32) -> SignalRoute {
    let mut mode = handoff.mode.lock();
    match *mode {
        ExitMode::Deferred => {
            *mode = ExitMode::Pending(code);
            handoff.parked.notify_waiters();
            SignalRoute::Pending
        }
        ExitMode::Immediate | ExitMode::Pending(_) => SignalRoute::Exit,
    }
}

trait SignalSource {
    fn next_code(&mut self) -> impl Future<Output = i32>;
}

impl SignalSource for SignalStreams {
    fn next_code(&mut self) -> impl Future<Output = i32> {
        SignalStreams::next_code(self)
    }
}

/// The watcher owns its thread and runtime because the run blocks its own workers on shutdown,
/// and the second signal must still exit. It lives until the process exits: tokio never releases
/// a claimed signal, so a stopped watcher would leave SIGINT and SIGTERM ignored.
async fn spawn_watcher<S: SignalSource + 'static>(
    install_source: impl FnOnce() -> S + Send + 'static,
    handoff: Arc<SignalHandoff>,
    exit: impl FnOnce(i32) + Send + 'static,
) -> io::Result<()> {
    let (claimed_tx, claimed) = oneshot::channel();
    std::thread::Builder::new()
        .name("grok-headless-signals".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = claimed_tx.send(Err(error));
                    return;
                }
            };
            let mut signals = {
                let _context = runtime.enter();
                install_source()
            };
            let _ = claimed_tx.send(Ok(()));
            exit(runtime.block_on(forced_exit_code(&mut signals, &handoff)));
        })?;
    claimed.await.map_err(io::Error::other)?
}

/// A parked signal leaves the turn to finish its cleanup however long it takes, since a second
/// signal or the sender's SIGKILL is the hard stop; only a signal that is not parked exits at once.
async fn forced_exit_code(signals: &mut impl SignalSource, handoff: &SignalHandoff) -> i32 {
    loop {
        let code = signals.next_code().await;
        match route_signal(handoff, code) {
            SignalRoute::Exit => return code,
            SignalRoute::Pending => {}
        }
    }
}

#[cfg(test)]
#[path = "signals_tests.rs"]
mod tests;
