use a3s_code_core::{GraphEvent, GraphEventRecord, GraphPatch, GraphRuntime};
use napi::bindgen_prelude::*;
use std::sync::Mutex;

#[napi(js_name = "StateGraphRuntime")]
pub struct JsStateGraphRuntime {
    inner: Mutex<GraphRuntime>,
}

#[napi]
impl JsStateGraphRuntime {
    #[napi(constructor, catch_unwind)]
    pub fn new(correlation_id: Option<String>) -> Self {
        let runtime = correlation_id
            .map(|id| GraphRuntime::new().with_correlation_id(id))
            .unwrap_or_default();
        Self {
            inner: Mutex::new(runtime),
        }
    }

    #[napi(factory, catch_unwind)]
    pub fn restore(events_json: String) -> Result<Self> {
        let events: Vec<GraphEventRecord> = serde_json::from_str(&events_json)
            .map_err(|error| Error::from_reason(format!("invalid graph events: {error}")))?;
        let runtime = GraphRuntime::restore(events)
            .map_err(|error| Error::from_reason(format!("invalid graph event log: {error}")))?;
        Ok(Self {
            inner: Mutex::new(runtime),
        })
    }

    #[napi(getter, catch_unwind)]
    pub fn branch_id(&self) -> Result<String> {
        Ok(self.lock()?.branch_id().to_string())
    }

    #[napi(getter, catch_unwind)]
    pub fn version(&self) -> Result<i64> {
        i64::try_from(self.lock()?.graph().version())
            .map_err(|_| Error::from_reason("graph version exceeds JavaScript safe binding range"))
    }

    #[napi(catch_unwind)]
    pub fn propose_patch(&self, patch_json: String) -> Result<bool> {
        let patch: GraphPatch = serde_json::from_str(&patch_json)
            .map_err(|error| Error::from_reason(format!("invalid graph patch: {error}")))?;
        self.lock()?
            .propose_patch(patch, None)
            .map_err(|error| Error::from_reason(error.to_string()))
    }

    #[napi(catch_unwind)]
    pub fn run_goal(&self, goal: String) -> Result<String> {
        let event = self
            .lock()?
            .run_goal(goal)
            .map_err(|error| Error::from_reason(error.to_string()))?;
        encode(&event, "goal event")
    }

    #[napi(catch_unwind)]
    pub fn emit_custom(&self, name: String, payload_json: String) -> Result<String> {
        let payload = serde_json::from_str(&payload_json)
            .map_err(|error| Error::from_reason(format!("invalid custom payload: {error}")))?;
        let event = self
            .lock()?
            .emit(GraphEvent::Custom { name, payload })
            .map_err(|error| Error::from_reason(error.to_string()))?;
        encode(&event, "custom event")
    }

    #[napi(catch_unwind)]
    pub fn graph_json(&self) -> Result<String> {
        encode(self.lock()?.graph(), "state graph")
    }

    #[napi(catch_unwind)]
    pub fn events_json(&self) -> Result<String> {
        encode(self.lock()?.events(), "graph events")
    }

    #[napi(catch_unwind)]
    pub fn fork_at(&self, sequence_exclusive: i64) -> Result<Self> {
        let sequence = u64::try_from(sequence_exclusive)
            .map_err(|_| Error::from_reason("fork sequence must be non-negative"))?;
        let runtime = self
            .lock()?
            .fork_at(sequence)
            .map_err(|error| Error::from_reason(error.to_string()))?;
        Ok(Self {
            inner: Mutex::new(runtime),
        })
    }

    #[napi(catch_unwind)]
    pub fn diff_json(&self, other: &JsStateGraphRuntime) -> Result<String> {
        let left = self.lock()?.graph().clone();
        let right = other.lock()?;
        encode(&left.diff(right.graph()), "graph diff")
    }
}

impl JsStateGraphRuntime {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, GraphRuntime>> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("state graph runtime lock poisoned"))
    }
}

fn encode(value: &(impl serde::Serialize + ?Sized), label: &str) -> Result<String> {
    serde_json::to_string(value)
        .map_err(|error| Error::from_reason(format!("failed to encode {label}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADD_TASK: &str = r#"{"expected_graph_version":0,"operations":[{"op":"add_object","id":"task-1","object_type":"task","data":{"status":"open"}}]}"#;

    #[test]
    fn wrapper_patches_forks_diffs_and_restores() {
        let runtime = JsStateGraphRuntime::new(Some("trace".into()));
        assert!(runtime.propose_patch(ADD_TASK.into()).unwrap());
        assert_eq!(runtime.version().unwrap(), 1);
        let events = runtime.events_json().unwrap();
        let restored = JsStateGraphRuntime::restore(events).unwrap();
        assert_eq!(restored.version().unwrap(), 1);
        let fork = restored.fork_at(3).unwrap();
        assert!(
            serde_json::from_str::<serde_json::Value>(&fork.diff_json(&restored).unwrap())
                .unwrap()
                .as_object()
                .is_some()
        );
    }
}
