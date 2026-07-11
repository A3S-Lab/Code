use super::*;

/// A single event from the agent's streaming output.
#[pyclass(name = "AgentEvent")]
#[derive(Clone)]
pub(super) struct PyAgentEvent {
    /// Stable event-envelope protocol version. Currently always ``1``.
    #[pyo3(get)]
    pub(super) version: u16,
    #[pyo3(get)]
    pub(super) event_type: String,
    /// Complete event payload, encoded as JSON without information loss.
    #[pyo3(get)]
    pub(super) payload_json: String,
    /// Optional protocol metadata, encoded as JSON.
    #[pyo3(get)]
    pub(super) metadata_json: Option<String>,
    #[pyo3(get)]
    pub(super) text: Option<String>,
    #[pyo3(get)]
    pub(super) tool_name: Option<String>,
    #[pyo3(get)]
    pub(super) tool_id: Option<String>,
    #[pyo3(get)]
    pub(super) tool_output: Option<String>,
    #[pyo3(get)]
    pub(super) exit_code: Option<i32>,
    #[pyo3(get)]
    pub(super) turn: Option<usize>,
    #[pyo3(get)]
    pub(super) prompt: Option<String>,
    #[pyo3(get)]
    pub(super) error: Option<String>,
    #[pyo3(get)]
    pub(super) total_tokens: Option<usize>,
    #[pyo3(get)]
    pub(super) verification_summary_json: Option<String>,
    #[pyo3(get)]
    pub(super) verification_summary_text: Option<String>,
    /// Legacy JSON view for events not fully represented by convenience
    /// fields. Prefer ``payload`` or ``payload_json`` in new code.
    #[pyo3(get)]
    pub(super) data: Option<String>,
    /// Structured discriminant for tool failures on ``tool_end`` events
    /// (JSON-encoded with a ``type`` field on the top level —
    /// e.g. ``{"type":"version_conflict","path":"doc.md","expected":"etag-1","actual":"etag-2"}``).
    /// ``None`` on success or untyped failure. Streaming consumers parse
    /// this via the ``error_kind`` property to branch on the failure
    /// kind without scanning ``tool_output``.
    #[pyo3(get)]
    pub(super) error_kind_json: Option<String>,
}

#[pymethods]
impl PyAgentEvent {
    /// Canonical envelope discriminant. ``event_type`` is retained as a
    /// compatibility alias.
    #[getter]
    fn r#type(&self) -> &str {
        &self.event_type
    }

    /// Parsed, lossless payload for known and future event types.
    #[getter]
    fn payload(&self, py: Python<'_>) -> PyResult<PyObject> {
        json_string_to_py(py, &self.payload_json)
    }

    /// Parsed protocol metadata, when present.
    #[getter]
    fn metadata(&self, py: Python<'_>) -> PyResult<Option<PyObject>> {
        self.metadata_json
            .as_deref()
            .map(|json| json_string_to_py(py, json))
            .transpose()
    }

    fn __repr__(&self) -> String {
        match self.event_type.as_str() {
            "text_delta" => format!(
                "AgentEvent(type='text_delta', text={:?})",
                self.text.as_deref().unwrap_or("")
            ),
            "tool_start" => format!(
                "AgentEvent(type='tool_start', tool='{}')",
                self.tool_name.as_deref().unwrap_or("")
            ),
            "agent_end" => format!(
                "AgentEvent(type='agent_end', tokens={})",
                self.total_tokens.unwrap_or(0)
            ),
            _ => format!("AgentEvent(type='{}')", self.event_type),
        }
    }

    /// Parsed `error_kind_json` as a dict — the discriminator lives on
    /// the ``type`` key (see [`ToolErrorKind`](crate::tools::ToolErrorKind)
    /// for the full set of variants). Downstream code matches on
    /// ``event.error_kind["type"]`` to decide retry behaviour without
    /// scanning ``tool_output``.
    #[getter]
    fn error_kind(&self, py: Python<'_>) -> PyResult<Option<PyObject>> {
        self.error_kind_json
            .as_deref()
            .map(|json| json_string_to_py(py, json))
            .transpose()
    }
}

impl TryFrom<RustAgentEvent> for PyAgentEvent {
    type Error = RustEventProtocolError;

    fn try_from(event: RustAgentEvent) -> Result<Self, Self::Error> {
        let projection = RustAgentEventProjectionV1::try_from(event)?;
        Ok(Self::from_projection(projection))
    }
}

impl PyAgentEvent {
    pub(super) fn from_projection(projection: RustAgentEventProjectionV1) -> Self {
        Self {
            version: projection.version,
            event_type: projection.event_type,
            payload_json: projection.payload_json,
            metadata_json: projection.metadata_json,
            text: projection.text,
            tool_name: projection.tool_name,
            tool_id: projection.tool_id,
            tool_output: projection.tool_output,
            exit_code: projection.exit_code,
            turn: projection.turn,
            prompt: projection.prompt,
            error: projection.error,
            total_tokens: projection.total_tokens,
            verification_summary_json: projection.verification_summary_json,
            verification_summary_text: projection.verification_summary_text,
            data: projection.data_json,
            error_kind_json: projection.error_kind_json,
        }
    }
}

/// Return the canonical version-1 event type catalog.
///
/// Event types remain open strings so consumers preserve future values.
#[pyfunction]
pub(super) fn agent_event_types_v1() -> Vec<&'static str> {
    AGENT_EVENT_TYPES_V1.to_vec()
}

/// Return the current stable event-envelope protocol version.
#[pyfunction]
pub(super) fn event_envelope_v1_version() -> u16 {
    EVENT_ENVELOPE_V1_VERSION
}

type StreamLifecycle = Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>;

pub(super) async fn recv_stream_event(
    rx: Arc<Mutex<tokio::sync::mpsc::Receiver<RustAgentEvent>>>,
    lifecycle: StreamLifecycle,
) -> Option<RustAgentEvent> {
    let event = {
        let mut guard = rx.lock().await;
        guard.recv().await
    };
    let terminal = matches!(
        event,
        Some(RustAgentEvent::End { .. } | RustAgentEvent::Error { .. })
    );
    if terminal || event.is_none() {
        if let Some(handle) = lifecycle.lock().await.take() {
            let _ = handle.await;
        }
    }
    event
}

/// One-shot callable used by `run_in_executor` for async iteration.
///
/// Each `__anext__` call creates a new instance; `__call__` blocks on the
/// next channel receive and raises `StopAsyncIteration` when done.
#[pyclass]
struct BlockingRecv {
    rx: Arc<Mutex<tokio::sync::mpsc::Receiver<RustAgentEvent>>>,
    done: Arc<AtomicBool>,
    lifecycle: StreamLifecycle,
}

#[pymethods]
impl BlockingRecv {
    fn __call__(&self, py: Python<'_>) -> PyResult<PyAgentEvent> {
        let rx = self.rx.clone();
        let done_flag = self.done.clone();
        let lifecycle = self.lifecycle.clone();
        let result = py.allow_threads(|| get_runtime().block_on(recv_stream_event(rx, lifecycle)));
        match result {
            Some(event) => {
                let is_end = matches!(event, RustAgentEvent::End { .. });
                let is_error = matches!(event, RustAgentEvent::Error { .. });
                let py_event = PyAgentEvent::try_from(event).map_err(|error| {
                    PyRuntimeError::new_err(format!("Failed to project agent event: {error}"))
                })?;
                if is_end || is_error {
                    done_flag.store(true, Ordering::Relaxed);
                }
                Ok(py_event)
            }
            None => {
                done_flag.store(true, Ordering::Relaxed);
                Err(PyStopAsyncIteration::new_err("stream exhausted"))
            }
        }
    }
}

/// Iterator / async-iterator that yields AgentEvents from a streaming execution.
///
/// Sync usage:  `for event in session.stream(prompt):`
/// Async usage: `async for event in session.stream(prompt):`
#[pyclass(name = "EventStream")]
pub(super) struct PyEventStream {
    pub(super) rx: Arc<Mutex<tokio::sync::mpsc::Receiver<RustAgentEvent>>>,
    pub(super) done: Arc<AtomicBool>,
    pub(super) lifecycle: StreamLifecycle,
}

#[pymethods]
impl PyEventStream {
    // ------------------------------------------------------------------
    // Sync iterator protocol
    // ------------------------------------------------------------------

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<Option<PyAgentEvent>> {
        if self.done.load(Ordering::Relaxed) {
            return Err(PyStopIteration::new_err("stream exhausted"));
        }

        let rx = self.rx.clone();
        let done_flag = self.done.clone();
        let lifecycle = self.lifecycle.clone();
        let result = py.allow_threads(|| get_runtime().block_on(recv_stream_event(rx, lifecycle)));

        match result {
            Some(event) => {
                let is_end = matches!(event, RustAgentEvent::End { .. });
                let is_error = matches!(event, RustAgentEvent::Error { .. });
                let py_event = PyAgentEvent::try_from(event).map_err(|error| {
                    PyRuntimeError::new_err(format!("Failed to project agent event: {error}"))
                })?;
                if is_end || is_error {
                    done_flag.store(true, Ordering::Relaxed);
                }
                Ok(Some(py_event))
            }
            None => {
                done_flag.store(true, Ordering::Relaxed);
                Err(PyStopIteration::new_err("stream exhausted"))
            }
        }
    }

    // ------------------------------------------------------------------
    // Async iterator protocol
    // ------------------------------------------------------------------

    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    /// Returns an `asyncio.Future` that resolves to the next `AgentEvent`.
    ///
    /// Uses `run_in_executor` to bridge the blocking channel recv into an
    /// asyncio-compatible awaitable without requiring `pyo3-async`.
    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        if self.done.load(Ordering::Relaxed) {
            return Err(PyStopAsyncIteration::new_err("stream exhausted"));
        }

        let callable = Bound::new(
            py,
            BlockingRecv {
                rx: self.rx.clone(),
                done: self.done.clone(),
                lifecycle: self.lifecycle.clone(),
            },
        )?;

        let asyncio = py.import("asyncio")?;
        let loop_ = asyncio.call_method0("get_running_loop")?;
        let future = loop_.call_method1("run_in_executor", (py.None(), callable))?;
        Ok(future)
    }
}
