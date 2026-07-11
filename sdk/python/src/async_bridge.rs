//! Asyncio executor bridge for Python-facing Agent and Session operations.

use super::*;

pub(super) enum AsyncSessionOperation {
    Create {
        agent: Arc<RustAgent>,
        workspace: String,
        options: Option<RustSessionOptions>,
    },
    Resume {
        agent: Arc<RustAgent>,
        session_id: String,
        options: RustSessionOptions,
    },
}

/// One-shot callable executed by asyncio's default executor.
#[pyclass]
pub(super) struct AsyncSessionCall {
    pub(super) operation: Option<AsyncSessionOperation>,
}

/// One-shot Agent constructor executed by asyncio's default executor.
#[pyclass]
pub(super) struct AsyncAgentCreateCall {
    pub(super) config_source: Option<String>,
}

#[pymethods]
impl AsyncAgentCreateCall {
    fn __call__(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let config_source = self
            .config_source
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("async agent creation already consumed"))?;
        let agent = py
            .allow_threads(move || get_runtime().block_on(RustAgent::new(config_source)))
            .map_err(py_code_error)?;
        Ok(Py::new(
            py,
            PyAgent {
                inner: Arc::new(agent),
            },
        )?
        .into_any())
    }
}

/// One-shot Agent close executed by asyncio's default executor.
#[pyclass]
pub(super) struct AsyncAgentCloseCall {
    pub(super) agent: Option<Arc<RustAgent>>,
}

#[pymethods]
impl AsyncAgentCloseCall {
    fn __call__(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let agent = self
            .agent
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("async agent close already consumed"))?;
        py.allow_threads(move || get_runtime().block_on(agent.close()));
        Ok(py.None())
    }
}

#[pymethods]
impl AsyncSessionCall {
    fn __call__(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let operation = self
            .operation
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("async session operation already consumed"))?;
        let session = py
            .allow_threads(move || match operation {
                AsyncSessionOperation::Create {
                    agent,
                    workspace,
                    options,
                } => get_runtime().block_on(agent.session_async(workspace, options)),
                AsyncSessionOperation::Resume {
                    agent,
                    session_id,
                    options,
                } => get_runtime().block_on(agent.resume_session_async(&session_id, options)),
            })
            .map_err(py_code_error)?;
        Ok(Py::new(
            py,
            PySession {
                inner: Arc::new(session),
            },
        )?
        .into_any())
    }
}

pub(super) enum AsyncSessionControlOperation {
    Cancel,
    Close,
    Save,
}

/// One-shot session control callable executed by asyncio's default executor.
#[pyclass]
pub(super) struct AsyncSessionControlCall {
    pub(super) session: Arc<RustAgentSession>,
    pub(super) operation: Option<AsyncSessionControlOperation>,
}

/// One-shot model request executed by asyncio's default executor.
#[pyclass]
pub(super) struct AsyncSessionSendCall {
    pub(super) session: Arc<RustAgentSession>,
    pub(super) prompt: Option<String>,
    pub(super) history: Option<Vec<RustMessage>>,
    pub(super) attachments: Option<Vec<a3s_code_core::llm::Attachment>>,
}

/// One-shot governed direct-tool call executed by asyncio's default executor.
#[pyclass]
pub(super) struct AsyncSessionToolCall {
    pub(super) session: Arc<RustAgentSession>,
    pub(super) name: Option<String>,
    pub(super) args: Option<serde_json::Value>,
}

#[pymethods]
impl AsyncSessionToolCall {
    fn __call__(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let name = self
            .name
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("async tool call already consumed"))?;
        let args = self.args.take().unwrap_or(serde_json::Value::Null);
        let session = Arc::clone(&self.session);
        let result = py
            .allow_threads(move || get_runtime().block_on(session.tool(&name, args)))
            .map_err(py_code_error)?;
        Ok(Py::new(py, PyToolResult::from(result))?.into_any())
    }
}

#[pymethods]
impl AsyncSessionSendCall {
    fn __call__(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let prompt = self
            .prompt
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("async send already consumed"))?;
        let history = self.history.take();
        let attachments = self.attachments.take().unwrap_or_default();
        let session = Arc::clone(&self.session);
        let result = py
            .allow_threads(move || {
                if attachments.is_empty() {
                    get_runtime().block_on(session.send(&prompt, history.as_deref()))
                } else {
                    get_runtime().block_on(session.send_with_attachments(
                        &prompt,
                        &attachments,
                        history.as_deref(),
                    ))
                }
            })
            .map_err(py_code_error)?;
        Ok(Py::new(py, PyAgentResult::from(result))?.into_any())
    }
}

pub(super) enum AsyncRunQueryOperation {
    Runs,
    Snapshot {
        run_id: String,
    },
    Events {
        run_id: String,
    },
    EventPage {
        run_id: String,
        after_sequence: Option<usize>,
        limit: usize,
    },
}

/// One-shot run observability query executed by asyncio's default executor.
#[pyclass]
pub(super) struct AsyncRunQueryCall {
    pub(super) session: Arc<RustAgentSession>,
    pub(super) operation: Option<AsyncRunQueryOperation>,
}

#[pymethods]
impl AsyncRunQueryCall {
    fn __call__(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let operation = self
            .operation
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("async run query already consumed"))?;
        let session = Arc::clone(&self.session);
        match operation {
            AsyncRunQueryOperation::Runs => {
                let runs = py.allow_threads(move || get_runtime().block_on(session.runs()));
                rust_json_to_py(py, &runs, "runs")
            }
            AsyncRunQueryOperation::Snapshot { run_id } => {
                let snapshot =
                    py.allow_threads(move || get_runtime().block_on(session.run_snapshot(&run_id)));
                rust_json_to_py(py, &snapshot, "run snapshot")
            }
            AsyncRunQueryOperation::Events { run_id } => {
                let session_id = session.session_id().to_string();
                let requested_run_id = run_id.clone();
                let events = py.allow_threads(move || {
                    get_runtime().block_on(session.run_events(&requested_run_id))
                });
                let envelopes = events
                    .iter()
                    .map(|record| rust_run_event_envelope_v1(record, &run_id, &session_id))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| PyRuntimeError::new_err(format!("Event protocol error: {e}")))?;
                rust_json_to_py(py, &envelopes, "run events")
            }
            AsyncRunQueryOperation::EventPage {
                run_id,
                after_sequence,
                limit,
            } => {
                let session_id = session.session_id().to_string();
                let requested_run_id = run_id.clone();
                let page = py.allow_threads(move || {
                    get_runtime().block_on(session.run_event_page(
                        &requested_run_id,
                        after_sequence,
                        limit,
                    ))
                });
                let Some(page) = page else {
                    return Ok(py.None());
                };
                let events = page
                    .events
                    .iter()
                    .map(|record| rust_run_event_envelope_v1(record, &run_id, &session_id))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| PyRuntimeError::new_err(format!("Event protocol error: {e}")))?;
                let value = serde_json::json!({
                    "events": events,
                    "first_available_sequence": page.first_available_sequence,
                    "latest_sequence_exclusive": page.latest_sequence_exclusive,
                    "next_after_sequence": page.next_after_sequence,
                    "retention_gap": page.retention_gap,
                    "has_more": page.has_more,
                });
                rust_json_to_py(py, &value, "run event page")
            }
        }
    }
}

fn rust_json_to_py<T: serde::Serialize>(
    py: Python<'_>,
    value: &T,
    description: &str,
) -> PyResult<Py<PyAny>> {
    let json = serde_json::to_string(value).map_err(|error| {
        PyRuntimeError::new_err(format!("Failed to serialize {description}: {error}"))
    })?;
    json_string_to_py(py, &json)
}

#[pymethods]
impl AsyncSessionControlCall {
    fn __call__(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let operation = self
            .operation
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("async session control already consumed"))?;
        let session = Arc::clone(&self.session);
        match operation {
            AsyncSessionControlOperation::Cancel => {
                let cancelled = py.allow_threads(move || get_runtime().block_on(session.cancel()));
                Ok(cancelled.into_pyobject(py)?.to_owned().unbind().into_any())
            }
            AsyncSessionControlOperation::Close => {
                py.allow_threads(move || get_runtime().block_on(session.close()));
                Ok(py.None())
            }
            AsyncSessionControlOperation::Save => {
                py.allow_threads(move || get_runtime().block_on(session.save()))
                    .map_err(py_code_error)?;
                Ok(py.None())
            }
        }
    }
}

pub(super) fn run_in_asyncio_executor<'py>(
    py: Python<'py>,
    callable: Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    let asyncio = py.import("asyncio")?;
    let loop_ = asyncio.call_method0("get_running_loop")?;
    loop_.call_method1("run_in_executor", (py.None(), callable))
}

pub(super) fn async_run_query_future<'py>(
    py: Python<'py>,
    session: Arc<RustAgentSession>,
    operation: AsyncRunQueryOperation,
) -> PyResult<Bound<'py, PyAny>> {
    let callable = Bound::new(
        py,
        AsyncRunQueryCall {
            session,
            operation: Some(operation),
        },
    )?;
    run_in_asyncio_executor(py, callable.into_any())
}
