use a3s_code_core::{GraphEvent, GraphEventRecord, GraphPatch, GraphRuntime};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use std::sync::Mutex;

#[pyclass(name = "StateGraphRuntime")]
pub struct PyStateGraphRuntime {
    inner: Mutex<GraphRuntime>,
}

#[pymethods]
impl PyStateGraphRuntime {
    #[new]
    #[pyo3(signature = (correlation_id=None))]
    fn new(correlation_id: Option<String>) -> Self {
        let runtime = correlation_id
            .map(|id| GraphRuntime::new().with_correlation_id(id))
            .unwrap_or_default();
        Self {
            inner: Mutex::new(runtime),
        }
    }

    #[staticmethod]
    fn restore(events_json: &str) -> PyResult<Self> {
        let events: Vec<GraphEventRecord> = serde_json::from_str(events_json)
            .map_err(|error| PyValueError::new_err(format!("invalid graph events: {error}")))?;
        let runtime = GraphRuntime::restore(events)
            .map_err(|error| PyValueError::new_err(format!("invalid graph event log: {error}")))?;
        Ok(Self {
            inner: Mutex::new(runtime),
        })
    }

    #[getter]
    fn branch_id(&self) -> PyResult<String> {
        Ok(self.lock()?.branch_id().to_string())
    }

    #[getter]
    fn version(&self) -> PyResult<u64> {
        Ok(self.lock()?.graph().version())
    }

    fn propose_patch(&self, patch_json: &str) -> PyResult<bool> {
        let patch: GraphPatch = serde_json::from_str(patch_json)
            .map_err(|error| PyValueError::new_err(format!("invalid graph patch: {error}")))?;
        self.lock()?
            .propose_patch(patch, None)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))
    }

    fn run_goal(&self, goal: String) -> PyResult<String> {
        let event = self
            .lock()?
            .run_goal(goal)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        encode(&event, "goal event")
    }

    fn emit_custom(&self, name: String, payload_json: &str) -> PyResult<String> {
        let payload = serde_json::from_str(payload_json)
            .map_err(|error| PyValueError::new_err(format!("invalid custom payload: {error}")))?;
        let event = self
            .lock()?
            .emit(GraphEvent::Custom { name, payload })
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        encode(&event, "custom event")
    }

    fn graph_json(&self) -> PyResult<String> {
        encode(self.lock()?.graph(), "state graph")
    }

    fn events_json(&self) -> PyResult<String> {
        encode(self.lock()?.events(), "graph events")
    }

    fn fork_at(&self, sequence_exclusive: u64) -> PyResult<Self> {
        let runtime = self
            .lock()?
            .fork_at(sequence_exclusive)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Self {
            inner: Mutex::new(runtime),
        })
    }

    fn diff_json(&self, other: &PyStateGraphRuntime) -> PyResult<String> {
        let left = self.lock()?.graph().clone();
        let right = other.lock()?;
        encode(&left.diff(right.graph()), "graph diff")
    }
}

impl PyStateGraphRuntime {
    fn lock(&self) -> PyResult<std::sync::MutexGuard<'_, GraphRuntime>> {
        self.inner
            .lock()
            .map_err(|_| PyRuntimeError::new_err("state graph runtime lock poisoned"))
    }
}

fn encode(value: &(impl serde::Serialize + ?Sized), label: &str) -> PyResult<String> {
    serde_json::to_string(value)
        .map_err(|error| PyRuntimeError::new_err(format!("failed to encode {label}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADD_TASK: &str = r#"{"expected_graph_version":0,"operations":[{"op":"add_object","id":"task-1","object_type":"task","data":{"status":"open"}}]}"#;

    #[test]
    fn wrapper_patches_forks_diffs_and_restores() {
        let runtime = PyStateGraphRuntime::new(Some("trace".into()));
        assert!(runtime.propose_patch(ADD_TASK).unwrap());
        assert_eq!(runtime.version().unwrap(), 1);
        let restored = PyStateGraphRuntime::restore(&runtime.events_json().unwrap()).unwrap();
        assert_eq!(restored.version().unwrap(), 1);
        let fork = restored.fork_at(3).unwrap();
        let diff: serde_json::Value =
            serde_json::from_str(&fork.diff_json(&restored).unwrap()).unwrap();
        assert!(diff.is_object());
    }
}
