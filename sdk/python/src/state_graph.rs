use a3s_code_core::{
    ExternalEvent, ExternalProjectionOutcome, GraphEvent, GraphEventRecord, GraphPatch,
    GraphRuntime, RuntimeLimits,
};
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
    #[pyo3(signature = (correlation_id=None, max_events=None, max_behavior_depth=None))]
    fn new(
        correlation_id: Option<String>,
        max_events: Option<usize>,
        max_behavior_depth: Option<usize>,
    ) -> Self {
        let limits = RuntimeLimits {
            max_events: max_events.unwrap_or_default(),
            max_behavior_depth: max_behavior_depth.unwrap_or_default(),
        };
        let limits = if max_events.is_none() && max_behavior_depth.is_none() {
            RuntimeLimits::default()
        } else {
            RuntimeLimits {
                max_events: if max_events.is_some() {
                    limits.max_events
                } else {
                    RuntimeLimits::default().max_events
                },
                max_behavior_depth: if max_behavior_depth.is_some() {
                    limits.max_behavior_depth
                } else {
                    RuntimeLimits::default().max_behavior_depth
                },
            }
        };
        let runtime = GraphRuntime::with_limits(limits);
        let runtime = if let Some(id) = correlation_id.filter(|id| !id.trim().is_empty()) {
            runtime.with_correlation_id(id)
        } else {
            runtime
        };
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

    /// Emit any versioned Core graph event. This keeps the SDK open to all
    /// current and future event variants while `emit_custom` remains a
    /// convenient typed shortcut.
    fn emit_json(&self, event_json: &str) -> PyResult<String> {
        let event: GraphEvent = serde_json::from_str(event_json)
            .map_err(|error| PyValueError::new_err(format!("invalid graph event: {error}")))?;
        let record = self
            .lock()?
            .emit(event)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        encode(&record, "graph event")
    }

    /// Validate a host-owned ordered event without mutating the graph.
    fn check_external(&self, event_json: &str) -> PyResult<Option<String>> {
        let event: ExternalEvent = serde_json::from_str(event_json)
            .map_err(|error| PyValueError::new_err(format!("invalid external event: {error}")))?;
        let outcome = self
            .lock()?
            .check_external(&event)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        Ok(outcome.map(|value| match value {
            ExternalProjectionOutcome::Applied => "applied".to_string(),
            ExternalProjectionOutcome::Duplicate => "duplicate".to_string(),
        }))
    }

    /// Atomically project a host event and its graph patch.
    fn project_external(&self, event_json: &str, patch_json: &str) -> PyResult<String> {
        let event: ExternalEvent = serde_json::from_str(event_json)
            .map_err(|error| PyValueError::new_err(format!("invalid external event: {error}")))?;
        let patch: GraphPatch = serde_json::from_str(patch_json)
            .map_err(|error| PyValueError::new_err(format!("invalid graph patch: {error}")))?;
        let outcome = self
            .lock()?
            .project_external(event, patch)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        Ok(match outcome {
            ExternalProjectionOutcome::Applied => "applied".to_string(),
            ExternalProjectionOutcome::Duplicate => "duplicate".to_string(),
        })
    }

    /// Strictly replay an event log without creating a mutable runtime.
    #[staticmethod]
    fn strict_replay(events_json: &str) -> PyResult<String> {
        let events: Vec<GraphEventRecord> = serde_json::from_str(events_json)
            .map_err(|error| PyValueError::new_err(format!("invalid graph events: {error}")))?;
        let graph = GraphRuntime::strict_replay(&events)
            .map_err(|error| PyValueError::new_err(format!("invalid graph event log: {error}")))?;
        encode(&graph, "state graph")
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
        let runtime = PyStateGraphRuntime::new(Some("trace".into()), None, None);
        assert!(runtime.propose_patch(ADD_TASK).unwrap());
        assert_eq!(runtime.version().unwrap(), 1);
        let restored = PyStateGraphRuntime::restore(&runtime.events_json().unwrap()).unwrap();
        assert_eq!(restored.version().unwrap(), 1);
        let fork = restored.fork_at(3).unwrap();
        let diff: serde_json::Value =
            serde_json::from_str(&fork.diff_json(&restored).unwrap()).unwrap();
        assert!(diff.is_object());
    }

    #[test]
    fn wrapper_projects_external_events_and_strict_replays() {
        let runtime = PyStateGraphRuntime::new(None, None, None);
        let event = r#"{"source":"queue","stream_id":"orders","sequence":1,"event_id":"e1","name":"order.created","payload":{"id":"o1"}}"#;
        assert_eq!(runtime.check_external(event).unwrap(), None);
        assert_eq!(runtime.project_external(event, ADD_TASK).unwrap(), "applied");
        assert_eq!(runtime.check_external(event).unwrap(), Some("duplicate".into()));
        let graph: serde_json::Value =
            serde_json::from_str(&PyStateGraphRuntime::strict_replay(&runtime.events_json().unwrap()).unwrap()).unwrap();
        assert_eq!(graph["version"], 1);
    }
}
