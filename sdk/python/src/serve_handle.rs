//! Observable lifecycle handle for the filesystem-first serve daemon.

use super::*;

/// Lifetime handle for a running serve daemon (see `Agent.serve_agent_dir`).
///
/// The daemon keeps running until `stop()` is called. Dropping the handle does
/// NOT cancel the daemon — call `stop()` explicitly for graceful shutdown.
#[pyclass(name = "ServeHandle")]
pub(super) struct PyServeHandle {
    pub(super) inner: Arc<RustServeDaemonHandle>,
}

#[pymethods]
impl PyServeHandle {
    /// Request graceful shutdown and wait for the daemon task to settle.
    fn stop(&self, py: Python<'_>) -> PyResult<()> {
        let handle = Arc::clone(&self.inner);
        let error_handle = Arc::clone(&handle);
        py.allow_threads(move || get_runtime().block_on(handle.stop()))
            .map_err(|error| py_serve_error(error_handle.failure_code(), error))?;
        Ok(())
    }

    /// Whether preparation completed and the daemon currently accepts work.
    fn is_ready(&self) -> bool {
        self.inner.is_ready()
    }

    /// Current lifecycle state: starting, ready, draining, stopped, or failed.
    fn state(&self) -> String {
        self.inner.status().phase.as_str().to_string()
    }

    /// Stable terminal failure code, or None while no failure is present.
    fn failure_code(&self) -> Option<String> {
        self.inner.failure_code().map(str::to_string)
    }

    /// Whether the daemon has stopped or failed.
    fn is_stopped(&self) -> bool {
        self.inner.is_stopped()
    }
}
