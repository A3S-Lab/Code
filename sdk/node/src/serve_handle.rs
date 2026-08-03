//! Observable lifecycle handle for the filesystem-first serve daemon.

use super::*;

/// Lifetime handle for a running serve daemon (see {@link Agent.serveAgentDir}).
///
/// The daemon keeps running until `stop()` is called. Dropping the handle does
/// NOT cancel the daemon — call `stop()` explicitly for graceful shutdown.
#[napi]
pub struct ServeHandle {
    pub(super) inner: Arc<RustServeDaemonHandle>,
}

#[napi]
impl ServeHandle {
    /// Request graceful shutdown of the serve daemon.
    ///
    /// Cancels in-flight schedule work and closes daemon-owned sessions.
    /// Idempotent; resolves only after the daemon task has settled, or rejects
    /// when the bounded shutdown deadline is exceeded.
    #[napi]
    pub async fn stop(&self) -> napi::Result<()> {
        let handle = Arc::clone(&self.inner);
        let error_handle = Arc::clone(&handle);
        get_runtime()
            .spawn(async move { handle.stop().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|error| node_serve_error(&error_handle, error))?;
        Ok(())
    }

    /// Whether preparation completed and the daemon currently accepts work.
    #[napi]
    pub fn is_ready(&self) -> bool {
        self.inner.is_ready()
    }

    /// Current lifecycle state: starting, ready, draining, stopped, or failed.
    #[napi]
    pub fn state(&self) -> String {
        self.inner.status().phase.as_str().to_string()
    }

    /// Stable terminal failure code, or null while no failure is present.
    #[napi]
    pub fn failure_code(&self) -> Option<String> {
        self.inner.failure_code().map(str::to_string)
    }

    /// Whether the daemon has stopped or failed.
    #[napi]
    pub fn is_stopped(&self) -> bool {
        self.inner.is_stopped()
    }
}
