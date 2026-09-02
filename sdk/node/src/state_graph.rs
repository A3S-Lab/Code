use a3s_code_core::{
    ExternalEvent, ExternalProjectionOutcome, GraphEvent, GraphEventRecord, GraphPatch,
    GraphRuntime, RuntimeLimits,
};
use napi::bindgen_prelude::*;
use std::sync::Mutex;

#[napi(js_name = "StateGraphRuntime")]
pub struct JsStateGraphRuntime {
    inner: Mutex<GraphRuntime>,
}

#[napi]
impl JsStateGraphRuntime {
    #[napi(
        constructor,
        catch_unwind,
        ts_args_type = "correlationId?: string | null, options?: StateGraphOptions"
    )]
    pub fn new(
        correlation_id: Option<String>,
        options: Option<StateGraphOptions>,
    ) -> Self {
        let options = options.unwrap_or_default();
        let limits = RuntimeLimits {
            max_events: options
                .max_events
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or_else(|| RuntimeLimits::default().max_events),
            max_behavior_depth: options
                .max_behavior_depth
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or_else(|| RuntimeLimits::default().max_behavior_depth),
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

    /// Emit any versioned Core graph event. `emitCustom` remains the compact
    /// helper for callers that only need a named JSON payload.
    #[napi(catch_unwind)]
    pub fn emit_json(&self, event_json: String) -> Result<String> {
        let event: GraphEvent = serde_json::from_str(&event_json)
            .map_err(|error| Error::from_reason(format!("invalid graph event: {error}")))?;
        let record = self
            .lock()?
            .emit(event)
            .map_err(|error| Error::from_reason(error.to_string()))?;
        encode(&record, "graph event")
    }

    /// Validate a host-owned ordered event without mutating the graph.
    #[napi(catch_unwind)]
    pub fn check_external(&self, event_json: String) -> Result<Option<String>> {
        let event: ExternalEvent = serde_json::from_str(&event_json)
            .map_err(|error| Error::from_reason(format!("invalid external event: {error}")))?;
        let outcome = self
            .lock()?
            .check_external(&event)
            .map_err(|error| Error::from_reason(error.to_string()))?;
        Ok(outcome.map(|value| match value {
            ExternalProjectionOutcome::Applied => "applied".to_string(),
            ExternalProjectionOutcome::Duplicate => "duplicate".to_string(),
        }))
    }

    /// Atomically project a host event and its graph patch.
    #[napi(catch_unwind)]
    pub fn project_external(&self, event_json: String, patch_json: String) -> Result<String> {
        let event: ExternalEvent = serde_json::from_str(&event_json)
            .map_err(|error| Error::from_reason(format!("invalid external event: {error}")))?;
        let patch: GraphPatch = serde_json::from_str(&patch_json)
            .map_err(|error| Error::from_reason(format!("invalid graph patch: {error}")))?;
        let outcome = self
            .lock()?
            .project_external(event, patch)
            .map_err(|error| Error::from_reason(error.to_string()))?;
        Ok(match outcome {
            ExternalProjectionOutcome::Applied => "applied".to_string(),
            ExternalProjectionOutcome::Duplicate => "duplicate".to_string(),
        })
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

/// Strictly replay an event log without creating a mutable runtime.
#[napi(js_name = "strictReplay", catch_unwind)]
pub fn strict_replay(events_json: String) -> Result<String> {
    let events: Vec<GraphEventRecord> = serde_json::from_str(&events_json)
        .map_err(|error| Error::from_reason(format!("invalid graph events: {error}")))?;
    let graph = GraphRuntime::strict_replay(&events)
        .map_err(|error| Error::from_reason(format!("invalid graph event log: {error}")))?;
    encode(&graph, "state graph")
}

#[napi(object)]
#[derive(Clone, Default)]
pub struct StateGraphOptions {
    /// Maximum number of records retained by the runtime.
    pub max_events: Option<i64>,
    /// Maximum reactive behavior recursion depth.
    pub max_behavior_depth: Option<i64>,
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
        let runtime = JsStateGraphRuntime::new(Some("trace".into()), None);
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

    #[test]
    fn wrapper_projects_external_events_and_strict_replays() {
        let runtime = JsStateGraphRuntime::new(None, None);
        let event = r#"{"source":"queue","stream_id":"orders","sequence":1,"event_id":"e1","name":"order.created","payload":{"id":"o1"}}"#;
        assert_eq!(runtime.check_external(event.into()).unwrap(), None);
        let outcome = runtime
            .project_external(event.into(), ADD_TASK.into())
            .unwrap();
        assert_eq!(outcome, "applied");
        assert_eq!(runtime.check_external(event.into()).unwrap(), Some("duplicate".into()));
        let events = runtime.events_json().unwrap();
        let graph = strict_replay(events).unwrap();
        let graph: serde_json::Value = serde_json::from_str(&graph).unwrap();
        assert_eq!(graph["version"], 1);
    }
}
