//! Release bindings over Core policy, evidence and original Run authority.

use super::session::PySession;
use super::*;

#[pyclass(name = "RunHandle")]
pub(super) struct PyRunHandle {
    inner: a3s_code_core::run::RunHandle,
}

#[pymethods]
impl PyRunHandle {
    #[getter]
    fn id(&self) -> &str {
        self.inner.id()
    }
    #[getter]
    fn session_id(&self) -> &str {
        self.inner.session_id()
    }
    /// Request cancellation of this exact original worker.
    fn cancel(&self, py: Python<'_>) -> bool {
        py.allow_threads(|| get_runtime().block_on(self.inner.cancel()))
    }
    fn snapshot(&self, py: Python<'_>) -> PyResult<PyObject> {
        let value = py.allow_threads(|| get_runtime().block_on(self.inner.snapshot()));
        let json = serde_json::to_string(&value)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        json_string_to_py(py, &json)
    }
}

/// `{stream, run}` for one admitted Core stream.
pub(super) fn run_stream(
    py: Python<'_>,
    admitted: a3s_code_core::AgentRunStream,
) -> PyResult<PyObject> {
    let result = PyDict::new(py);
    result.set_item(
        "stream",
        Py::new(
            py,
            PyEventStream {
                rx: Arc::new(Mutex::new(admitted.events)),
                done: Arc::new(AtomicBool::new(false)),
                lifecycle: Arc::new(Mutex::new(Some(admitted.worker))),
            },
        )?,
    )?;
    result.set_item(
        "run",
        admitted
            .run
            .map(|inner| Py::new(py, PyRunHandle { inner }))
            .transpose()?,
    )?;
    Ok(result.into_any().unbind())
}

#[pymethods]
impl PySession {
    /// Return {stream, run} with the exact admitted Core Run. Local commands
    /// that do not admit a model Run return None as their Run handle.
    #[pyo3(signature = (prompt, history=None))]
    fn stream_with_run(
        &self,
        py: Python<'_>,
        prompt: String,
        history: Option<&Bound<'_, PyList>>,
    ) -> PyResult<PyObject> {
        let history = history.map(py_list_to_messages).transpose()?;
        let admitted = py
            .allow_threads(|| {
                get_runtime().block_on(self.inner.stream_with_run(&prompt, history.as_deref()))
            })
            .map_err(py_code_error)?;
        run_stream(py, admitted)
    }

    /// Irreversibly discard facts after seq in this session. Files, vectors
    /// and memory are not rolled back; refuses while a Run owns the session.
    fn fork_session_at(&self, py: Python<'_>, seq: u64) -> PyResult<u64> {
        py.allow_threads(|| get_runtime().block_on(self.inner.fork_session_at(seq)))
            .map_err(py_code_error)
    }

    fn projected_capability_names(&self) -> Vec<String> {
        self.inner.projected_capability_names()
    }

    /// Store the next-Run policy. An open Run retains its admitted policy.
    fn set_permission_policy(&self, policy: PyPermissionPolicy) -> PyResult<()> {
        self.inner
            .set_permission_policy(py_permission_policy_to_rust(policy)?)
            .map_err(py_code_error)
    }

    fn set_permission_posture(&self, posture: &str) -> PyResult<()> {
        if !matches!(
            posture,
            "default" | "plan" | "auto" | "always-approve" | "yolo"
        ) {
            return Err(PyValueError::new_err("unknown permission posture"));
        }
        let posture = if posture == "always-approve" {
            "yolo"
        } else {
            posture
        };
        self.inner
            .set_permission_posture(posture)
            .map_err(py_code_error)
    }

    /// Bounded read of this session's original Run/artifact evidence.
    #[cfg(feature = "advanced-harness")]
    fn read_evaluation_evidence(
        &self,
        py: Python<'_>,
        request: &Bound<'_, PyAny>,
    ) -> PyResult<PyObject> {
        let request = serde_json::from_str(&py_any_to_json(request)?)
            .map_err(|error| PyValueError::new_err(format!("invalid evidence request: {error}")))?;
        let evidence = py
            .allow_threads(|| get_runtime().block_on(self.inner.read_evaluation_evidence(request)))
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        let json = serde_json::to_string(&evidence)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        json_string_to_py(py, &json)
    }
}
