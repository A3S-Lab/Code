//! Python Session memory methods.

use super::session::PySession;
use super::*;

#[pymethods]
impl PySession {
    // ========================================================================
    // Memory API
    // ========================================================================

    /// Check if memory is available for this session.
    #[getter]
    fn has_memory(&self) -> bool {
        self.inner.memory().is_some()
    }

    /// Remember a successful task execution.
    ///
    /// Args:
    ///     task: Description of the task
    ///     tools: List of tool names used
    ///     result: Summary of the result
    #[pyo3(signature = (task, tools, result))]
    fn remember_success(
        &self,
        py: Python<'_>,
        task: String,
        tools: Vec<String>,
        result: String,
    ) -> PyResult<()> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        py.allow_threads(move || {
            get_runtime().block_on(memory.remember_success(&task, &tools, &result))
        })
        .map_err(|e| PyRuntimeError::new_err(format!("Remember failed: {e}")))
    }

    /// Remember a failed task execution.
    ///
    /// Args:
    ///     task: Description of the task
    ///     error: Error message
    ///     tools: List of tool names attempted
    #[pyo3(signature = (task, error, tools))]
    fn remember_failure(
        &self,
        py: Python<'_>,
        task: String,
        error: String,
        tools: Vec<String>,
    ) -> PyResult<()> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        py.allow_threads(move || {
            get_runtime().block_on(memory.remember_failure(&task, &error, &tools))
        })
        .map_err(|e| PyRuntimeError::new_err(format!("Remember failed: {e}")))
    }

    /// Recall memories similar to a query.
    ///
    /// Args:
    ///     query: Search query
    ///     limit: Maximum number of results (default: 5)
    ///
    /// Returns:
    ///     List of dicts with task, tools, result/error, outcome, timestamp.
    #[pyo3(signature = (query, limit=5))]
    fn recall_similar<'py>(
        &self,
        py: Python<'py>,
        query: String,
        limit: usize,
    ) -> PyResult<Bound<'py, PyList>> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        let items = py
            .allow_threads(move || get_runtime().block_on(memory.recall_similar(&query, limit)))
            .map_err(|e| PyRuntimeError::new_err(format!("Recall failed: {e}")))?;
        let json_str = serde_json::to_string(&items)
            .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
        let json_mod = py.import("json")?;
        let py_obj = json_mod.call_method1("loads", (json_str,))?;
        py_obj
            .downcast::<PyList>()
            .cloned()
            .map_err(|e| PyRuntimeError::new_err(format!("Unexpected result: {e}")))
    }

    /// Recall memories by tags.
    ///
    /// Args:
    ///     tags: List of tags to search for
    ///     limit: Maximum number of results (default: 10)
    ///
    /// Returns:
    ///     List of memory item dicts.
    #[pyo3(signature = (tags, limit=10))]
    fn recall_by_tags<'py>(
        &self,
        py: Python<'py>,
        tags: Vec<String>,
        limit: usize,
    ) -> PyResult<Bound<'py, PyList>> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        let items = py
            .allow_threads(move || get_runtime().block_on(memory.recall_by_tags(&tags, limit)))
            .map_err(|e| PyRuntimeError::new_err(format!("Recall failed: {e}")))?;
        let json_str = serde_json::to_string(&items)
            .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
        let json_mod = py.import("json")?;
        let py_obj = json_mod.call_method1("loads", (json_str,))?;
        py_obj
            .downcast::<PyList>()
            .cloned()
            .map_err(|e| PyRuntimeError::new_err(format!("Unexpected result: {e}")))
    }

    /// Get recent memory items.
    ///
    /// Args:
    ///     limit: Maximum number of results (default: 10)
    ///
    /// Returns:
    ///     List of memory item dicts.
    #[pyo3(signature = (limit=10))]
    fn memory_recent<'py>(&self, py: Python<'py>, limit: usize) -> PyResult<Bound<'py, PyList>> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        let items = py
            .allow_threads(move || get_runtime().block_on(memory.get_recent(limit)))
            .map_err(|e| PyRuntimeError::new_err(format!("Recall failed: {e}")))?;
        let json_str = serde_json::to_string(&items)
            .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
        let json_mod = py.import("json")?;
        let py_obj = json_mod.call_method1("loads", (json_str,))?;
        py_obj
            .downcast::<PyList>()
            .cloned()
            .map_err(|e| PyRuntimeError::new_err(format!("Unexpected result: {e}")))
    }

    /// Get memory statistics.
    ///
    /// Returns:
    ///     Dict with long_term_count, short_term_count, working_count.
    fn memory_stats<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        let stats = py
            .allow_threads(move || get_runtime().block_on(memory.stats()))
            .map_err(|e| PyRuntimeError::new_err(format!("Stats failed: {e}")))?;
        let json_str = serde_json::to_string(&stats)
            .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
        let json_mod = py.import("json")?;
        let py_obj = json_mod.call_method1("loads", (json_str,))?;
        py_obj
            .downcast::<PyDict>()
            .cloned()
            .map_err(|e| PyRuntimeError::new_err(format!("Unexpected result: {e}")))
    }

    /// Get current working memory items.
    ///
    /// Working memory holds the active context items for the current task.
    ///
    /// Returns:
    ///     List of memory item dicts currently in working memory.
    fn get_working<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        let items = py.allow_threads(move || get_runtime().block_on(memory.get_working()));
        let json_str = serde_json::to_string(&items)
            .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
        let json_mod = py.import("json")?;
        let py_obj = json_mod.call_method1("loads", (json_str,))?;
        py_obj
            .downcast::<PyList>()
            .cloned()
            .map_err(|e| PyRuntimeError::new_err(format!("Unexpected result: {e}")))
    }

    /// Clear working memory.
    ///
    /// Removes all items from working memory without affecting short-term or long-term memory.
    fn clear_working(&self, py: Python<'_>) -> PyResult<()> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        py.allow_threads(move || get_runtime().block_on(memory.clear_working()));
        Ok(())
    }

    /// Get current short-term memory items.
    ///
    /// Short-term memory contains items stored during this session.
    ///
    /// Returns:
    ///     List of memory item dicts in short-term memory for this session.
    fn get_short_term<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        let items = py.allow_threads(move || get_runtime().block_on(memory.get_short_term()));
        let json_str = serde_json::to_string(&items)
            .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
        let json_mod = py.import("json")?;
        let py_obj = json_mod.call_method1("loads", (json_str,))?;
        py_obj
            .downcast::<PyList>()
            .cloned()
            .map_err(|e| PyRuntimeError::new_err(format!("Unexpected result: {e}")))
    }

    /// Clear short-term memory for this session.
    ///
    /// Removes all session-scoped memory items without affecting long-term or working memory.
    fn clear_short_term(&self, py: Python<'_>) -> PyResult<()> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        py.allow_threads(move || get_runtime().block_on(memory.clear_short_term()));
        Ok(())
    }
}
