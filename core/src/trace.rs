//! Runtime trace primitives.
//!
//! Trace events are compact execution facts emitted by the harness. They are
//! separate from model-visible tool output and from large artifacts.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};
use std::time::Duration;

pub const TRACE_EVENT_SCHEMA: &str = "a3s.trace_event.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceEventKind {
    ToolExecution,
    ProgramExecution,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceEvent {
    pub schema: String,
    pub kind: TraceEventKind,
    pub name: String,
    pub success: bool,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub output_bytes: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metadata_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_uris: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl TraceEvent {
    pub fn tool_execution(
        name: impl Into<String>,
        success: bool,
        exit_code: i32,
        duration: Duration,
        output_bytes: usize,
        metadata: Option<&serde_json::Value>,
    ) -> Self {
        Self {
            schema: TRACE_EVENT_SCHEMA.to_string(),
            kind: TraceEventKind::ToolExecution,
            name: name.into(),
            success,
            exit_code,
            duration_ms: duration.as_millis().min(u128::from(u64::MAX)) as u64,
            output_bytes,
            metadata_keys: metadata_keys(metadata),
            artifact_uris: artifact_uris(metadata),
            details: None,
        }
    }

    pub fn program_execution(
        name: impl Into<String>,
        success: bool,
        exit_code: i32,
        duration: Duration,
        output_bytes: usize,
        metadata: Option<&serde_json::Value>,
    ) -> Self {
        let details = metadata
            .and_then(|metadata| metadata.get("trace"))
            .map(program_trace_summary);

        Self {
            schema: TRACE_EVENT_SCHEMA.to_string(),
            kind: TraceEventKind::ProgramExecution,
            name: name.into(),
            success,
            exit_code,
            duration_ms: duration.as_millis().min(u128::from(u64::MAX)) as u64,
            output_bytes,
            metadata_keys: metadata_keys(metadata),
            artifact_uris: artifact_uris(metadata),
            details,
        }
    }
}

pub trait TraceSink: Send + Sync {
    fn record(&self, event: TraceEvent);
}

#[derive(Debug, Clone, Default)]
pub struct InMemoryTraceSink {
    events: Arc<RwLock<Vec<TraceEvent>>>,
}

impl InMemoryTraceSink {
    pub fn events(&self) -> Vec<TraceEvent> {
        self.events.read().unwrap().clone()
    }

    pub fn replace_events(&self, events: Vec<TraceEvent>) {
        *self.events.write().unwrap() = events;
    }

    pub fn clear(&self) {
        self.events.write().unwrap().clear();
    }
}

impl TraceSink for InMemoryTraceSink {
    fn record(&self, event: TraceEvent) {
        self.events.write().unwrap().push(event);
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoopTraceSink;

impl TraceSink for NoopTraceSink {
    fn record(&self, _event: TraceEvent) {}
}

fn metadata_keys(metadata: Option<&serde_json::Value>) -> Vec<String> {
    let Some(serde_json::Value::Object(object)) = metadata else {
        return Vec::new();
    };

    let mut keys = object.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    keys
}

fn artifact_uris(metadata: Option<&serde_json::Value>) -> Vec<String> {
    let mut uris = Vec::new();
    if let Some(metadata) = metadata {
        collect_artifact_uris(metadata, &mut uris);
    }
    uris.sort();
    uris.dedup();
    uris
}

fn collect_artifact_uris(value: &serde_json::Value, uris: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(object) => {
            if let Some(uri) = object.get("artifact_uri").and_then(|value| value.as_str()) {
                uris.push(uri.to_string());
            }
            for value in object.values() {
                collect_artifact_uris(value, uris);
            }
        }
        serde_json::Value::Array(items) => {
            for value in items {
                collect_artifact_uris(value, uris);
            }
        }
        _ => {}
    }
}

fn program_trace_summary(trace: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "program_name": trace.get("program_name").cloned().unwrap_or_default(),
        "success": trace.get("success").cloned().unwrap_or_default(),
        "step_count": trace.get("step_count").cloned().unwrap_or_default(),
        "failed_steps": trace.get("failed_steps").cloned().unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_trace_sink_records_events() {
        let sink = InMemoryTraceSink::default();
        sink.record(TraceEvent::tool_execution(
            "read",
            true,
            0,
            Duration::from_millis(7),
            12,
            Some(&serde_json::json!({
                "artifact": {
                    "artifact_uri": "a3s://tool-output/read/abc"
                },
                "file_path": "src/lib.rs"
            })),
        ));

        let events = sink.events();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].schema, TRACE_EVENT_SCHEMA);
        assert_eq!(events[0].kind, TraceEventKind::ToolExecution);
        assert_eq!(events[0].metadata_keys, vec!["artifact", "file_path"]);
        assert_eq!(events[0].artifact_uris, vec!["a3s://tool-output/read/abc"]);
    }

    #[test]
    fn program_trace_event_stores_compact_summary() {
        let event = TraceEvent::program_execution(
            "program",
            true,
            0,
            Duration::from_millis(3),
            42,
            Some(&serde_json::json!({
                "trace": {
                    "program_name": "program_repo_map",
                    "success": true,
                    "step_count": 7,
                    "failed_steps": 0,
                    "steps": [{"output": "not copied into event"}]
                }
            })),
        );

        assert_eq!(event.kind, TraceEventKind::ProgramExecution);
        assert_eq!(
            event.details.as_ref().unwrap()["program_name"],
            "program_repo_map"
        );
        assert!(event.details.as_ref().unwrap().get("steps").is_none());
    }
}
