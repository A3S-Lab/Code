//! Python Session queue and HITL methods.

use super::session::PySession;
use super::*;

#[pymethods]
impl PySession {
    // ========================================================================
    // Advanced optional Queue API
    // ========================================================================

    /// Check if this session has an advanced lane queue configured.
    fn has_queue(&self) -> bool {
        self.inner.has_queue()
    }

    /// Configure a lane's handler mode for explicit external/hybrid dispatch.
    ///
    /// Args:
    ///     lane (Literal["control", "query", "execute", "generate"]): Which lane to configure.
    ///     mode (Literal["internal", "external", "hybrid"]): Execution mode for the lane's tools.
    ///     timeout_ms: Timeout for external processing in milliseconds (default 60000).
    #[pyo3(signature = (lane, mode="internal", timeout_ms=60000))]
    fn set_lane_handler(
        &self,
        py: Python<'_>,
        lane: &str,
        mode: &str,
        timeout_ms: u64,
    ) -> PyResult<()> {
        let lane = parse_lane(lane)?;
        let mode = parse_handler_mode(mode)?;
        let config = RustLaneHandlerConfig { mode, timeout_ms };
        let session = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(session.set_lane_handler(lane, config)))
            .map_err(py_code_error)
    }

    /// Complete an external queue task by ID.
    ///
    /// Args:
    ///     task_id: The task identifier
    ///     success: Whether the task succeeded
    ///     result: Result data (any JSON-serializable value)
    ///     error: Optional error message
    ///
    /// Returns:
    ///     True if the task was found and completed, False if not found.
    #[pyo3(signature = (task_id, success=true, result=None, error=None))]
    fn complete_external_task(
        &self,
        py: Python<'_>,
        task_id: String,
        success: bool,
        result: Option<&Bound<'_, PyDict>>,
        error: Option<String>,
    ) -> PyResult<bool> {
        let result_value = match result {
            Some(dict) => {
                let json_str = py_dict_to_json(dict)?;
                serde_json::from_str(&json_str)
                    .map_err(|e| PyValueError::new_err(format!("Invalid JSON: {e}")))?
            }
            None => serde_json::json!({}),
        };
        let ext_result = RustExternalTaskResult {
            success,
            result: result_value,
            error,
        };
        let session = self.inner.clone();
        let found = py.allow_threads(move || {
            get_runtime().block_on(session.complete_external_task(&task_id, ext_result))
        });
        Ok(found)
    }

    /// Get pending external queue tasks.
    ///
    /// Returns:
    ///     List of dicts with task_id, session_id, lane, command_type, payload, timeout_ms.
    fn pending_external_tasks<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let session = self.inner.clone();
        let tasks =
            py.allow_threads(move || get_runtime().block_on(session.pending_external_tasks()));
        let json_str = serde_json::to_string(&tasks)
            .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
        let json_mod = py.import("json")?;
        let py_obj = json_mod.call_method1("loads", (json_str,))?;
        py_obj
            .downcast::<PyList>()
            .cloned()
            .map_err(|e| PyRuntimeError::new_err(format!("Unexpected result: {e}")))
    }

    /// Return pending HITL tool confirmations for this session.
    fn pending_confirmations<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let session = self.inner.clone();
        let pending =
            py.allow_threads(move || get_runtime().block_on(session.pending_confirmations()));
        let json_str = serde_json::to_string(&pending)
            .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
        let json_mod = py.import("json")?;
        let py_obj = json_mod.call_method1("loads", (json_str,))?;
        py_obj
            .downcast::<PyList>()
            .cloned()
            .map_err(|e| PyRuntimeError::new_err(format!("Unexpected result: {e}")))
    }

    /// Resolve a pending HITL tool confirmation.
    #[pyo3(signature = (tool_id, approved, reason=None))]
    fn confirm_tool_use(
        &self,
        py: Python<'_>,
        tool_id: String,
        approved: bool,
        reason: Option<String>,
    ) -> PyResult<bool> {
        let session = self.inner.clone();
        py.allow_threads(move || {
            get_runtime().block_on(session.confirm_tool_use(&tool_id, approved, reason))
        })
        .map_err(py_code_error)
    }

    /// Cancel all pending HITL confirmations for this session.
    fn cancel_confirmations(&self, py: Python<'_>) -> usize {
        let session = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(session.cancel_confirmations()))
    }

    /// Get optional queue statistics.
    ///
    /// Returns:
    ///     Dict with total_pending, total_active, external_pending, and per-lane status.
    fn queue_stats<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let session = self.inner.clone();
        let stats = py.allow_threads(move || get_runtime().block_on(session.queue_stats()));
        let json_str = serde_json::to_string(&stats)
            .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
        let json_mod = py.import("json")?;
        let py_obj = json_mod.call_method1("loads", (json_str,))?;
        py_obj
            .downcast::<PyDict>()
            .cloned()
            .map_err(|e| PyRuntimeError::new_err(format!("Unexpected result: {e}")))
    }

    /// Get dead letters from the optional queue's DLQ.
    ///
    /// Returns:
    ///     List of dicts with command_id, command_type, lane, error, attempts, failed_at.
    fn dead_letters<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let session = self.inner.clone();
        let letters = py.allow_threads(move || get_runtime().block_on(session.dead_letters()));
        let json_str = serde_json::to_string(&letters)
            .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
        let json_mod = py.import("json")?;
        let py_obj = json_mod.call_method1("loads", (json_str,))?;
        py_obj
            .downcast::<PyList>()
            .cloned()
            .map_err(|e| PyRuntimeError::new_err(format!("Unexpected result: {e}")))
    }

    /// Get a detailed metrics snapshot from the queue.
    ///
    /// Returns ``None`` if metrics are not enabled (queue not configured or
    /// ``enable_metrics`` was not set in ``SessionQueueConfig``).
    ///
    /// Returns:
    ///     Dict with ``counters``, ``gauges``, and ``histograms`` maps, or None.
    fn queue_metrics<'py>(&self, py: Python<'py>) -> PyResult<PyObject> {
        let session = self.inner.clone();
        let snapshot = py.allow_threads(move || get_runtime().block_on(session.queue_metrics()));
        match snapshot {
            None => Ok(py.None()),
            Some(s) => {
                let json_str = metrics_snapshot_to_json_str(s)
                    .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
                let json_mod = py.import("json")?;
                Ok(json_mod.call_method1("loads", (json_str,))?.into())
            }
        }
    }
}
