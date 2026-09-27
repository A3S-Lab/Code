/// The signals the 130/143/129 exit-code map covers, claimed from construction for as long as this
/// lives: without a live stream a signal takes its default action and kills the process.
#[cfg(unix)]
pub struct SignalStreams {
    interrupt: Option<tokio::signal::unix::Signal>,
    terminate: Option<tokio::signal::unix::Signal>,
    hangup: Option<tokio::signal::unix::Signal>,
}

#[cfg(unix)]
impl SignalStreams {
    pub fn install() -> SignalStreams {
        use tokio::signal::unix::{SignalKind, signal};

        SignalStreams {
            interrupt: signal(SignalKind::interrupt())
                .inspect_err(|error| tracing::warn!(%error, "no SIGINT handler"))
                .ok(),
            terminate: signal(SignalKind::terminate())
                .inspect_err(|error| tracing::warn!(%error, "no SIGTERM handler"))
                .ok(),
            hangup: signal(SignalKind::hangup())
                .inspect_err(|error| tracing::warn!(%error, "no SIGHUP handler"))
                .ok(),
        }
    }

    pub async fn next_code(&mut self) -> i32 {
        next_signal_code(&mut self.interrupt, &mut self.terminate, &mut self.hangup).await
    }
}

#[cfg(windows)]
pub struct SignalStreams {
    ctrl_c: Option<tokio::signal::windows::CtrlC>,
}

#[cfg(windows)]
impl SignalStreams {
    pub fn install() -> SignalStreams {
        SignalStreams {
            ctrl_c: tokio::signal::windows::ctrl_c()
                .inspect_err(|error| tracing::warn!(%error, "no Ctrl-C handler"))
                .ok(),
        }
    }

    pub async fn next_code(&mut self) -> i32 {
        match self.ctrl_c.as_mut() {
            Some(ctrl_c) => {
                ctrl_c.recv().await;
            }
            None => std::future::pending::<()>().await,
        }
        130
    }
}

/// Wait for the next SIGINT/SIGTERM/SIGHUP and map it to its exit code.
#[cfg(unix)]
pub(crate) async fn next_signal_code(
    sigint: &mut Option<tokio::signal::unix::Signal>,
    sigterm: &mut Option<tokio::signal::unix::Signal>,
    sighup: &mut Option<tokio::signal::unix::Signal>,
) -> i32 {
    tokio::select! {
        _ = recv_optional_unix_signal(sigint) => 130,
        _ = recv_optional_unix_signal(sigterm) => 143,
        _ = recv_optional_unix_signal(sighup) => 129,
    }
}

/// Await one recv on an optional unix signal stream, or pend forever if absent.
#[cfg(unix)]
async fn recv_optional_unix_signal(sig: &mut Option<tokio::signal::unix::Signal>) {
    if let Some(s) = sig.as_mut() {
        let _ = s.recv().await;
    } else {
        std::future::pending::<()>().await;
    }
}
