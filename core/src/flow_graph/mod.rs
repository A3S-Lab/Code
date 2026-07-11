//! One-way projection from committed A3S Flow history into the state graph.
//!
//! Flow remains authoritative for scheduling and execution. This module only
//! maintains an auditable domain projection; it never edits a Flow snapshot.

use crate::state_graph::{
    ExternalEvent, ExternalProjectionOutcome, GraphPatch, GraphRuntime, PatchOperation,
    RuntimeError,
};
use a3s_flow::{FlowEvent, FlowEventEnvelope, FlowEventObserver};
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

mod decision;
pub use decision::{
    FlowDecision, FlowDecisionDispatchError, FlowDecisionDispatcher, FlowDecisionRequest,
    FlowDecisionSink, FlowDecisionStep,
};

pub const FLOW_GRAPH_SOURCE: &str = "a3s-flow";

#[derive(Clone)]
pub struct FlowGraphObserver {
    runtime: Arc<Mutex<GraphRuntime>>,
    last_error: Arc<RwLock<Option<String>>>,
}

impl FlowGraphObserver {
    pub fn new(runtime: Arc<Mutex<GraphRuntime>>) -> Self {
        Self {
            runtime,
            last_error: Arc::new(RwLock::new(None)),
        }
    }

    pub fn runtime(&self) -> Arc<Mutex<GraphRuntime>> {
        Arc::clone(&self.runtime)
    }

    pub async fn last_error(&self) -> Option<String> {
        self.last_error.read().await.clone()
    }

    pub async fn project(
        &self,
        envelope: FlowEventEnvelope,
    ) -> Result<ExternalProjectionOutcome, RuntimeError> {
        let mut runtime = self.runtime.lock().await;
        let patch = projection_patch(&runtime, &envelope)?;
        runtime.project_external(external_event(&envelope), patch)
    }

    /// Replay committed Flow history in sequence order, applying only events
    /// not already represented by the restored graph cursor.
    pub async fn catch_up(
        &self,
        mut history: Vec<FlowEventEnvelope>,
    ) -> Result<usize, RuntimeError> {
        history.sort_by(|left, right| {
            left.run_id
                .cmp(&right.run_id)
                .then(left.sequence.cmp(&right.sequence))
        });
        let mut applied = 0;
        for envelope in history {
            if self.project(envelope).await? == ExternalProjectionOutcome::Applied {
                applied += 1;
            }
        }
        Ok(applied)
    }
}

#[async_trait]
impl FlowEventObserver for FlowGraphObserver {
    async fn observe(&self, envelope: FlowEventEnvelope) {
        match self.project(envelope).await {
            Ok(_) => *self.last_error.write().await = None,
            Err(error) => *self.last_error.write().await = Some(error.to_string()),
        }
    }
}

fn external_event(envelope: &FlowEventEnvelope) -> ExternalEvent {
    ExternalEvent {
        source: FLOW_GRAPH_SOURCE.to_string(),
        stream_id: envelope.run_id.clone(),
        sequence: envelope.sequence,
        event_id: envelope.event_id.to_string(),
        name: envelope.event.event_key().to_string(),
        payload: flow_event_payload(&envelope.event),
    }
}

fn flow_event_payload(event: &FlowEvent) -> Value {
    match event {
        // Hook tokens are execution capabilities and must not enter the graph
        // audit log. Flow retains the authoritative secret-bearing event.
        FlowEvent::HookCreated {
            hook_id, metadata, ..
        } => json!({"type": "hook_created", "hook_id": hook_id, "metadata": metadata}),
        _ => serde_json::to_value(event).unwrap_or(Value::Null),
    }
}

fn projection_patch(
    runtime: &GraphRuntime,
    envelope: &FlowEventEnvelope,
) -> Result<GraphPatch, RuntimeError> {
    let graph = runtime.graph();
    let run_id = run_object_id(&envelope.run_id);
    let mut operations = Vec::new();
    match &envelope.event {
        FlowEvent::RunCreated { spec, input } => operations.push(PatchOperation::AddObject {
            id: run_id,
            object_type: "workflow_run".to_string(),
            data: json!({
                "run_id": envelope.run_id,
                "status": "created",
                "spec": spec,
                "input": input,
                "last_sequence": envelope.sequence,
            }),
        }),
        FlowEvent::RunStarted => update_object(
            graph,
            &run_id,
            envelope.sequence,
            [("status", json!("running"))],
            &mut operations,
        )?,
        FlowEvent::RunCompleted { output } => update_object(
            graph,
            &run_id,
            envelope.sequence,
            [("status", json!("completed")), ("output", output.clone())],
            &mut operations,
        )?,
        FlowEvent::RunFailed { error } => update_object(
            graph,
            &run_id,
            envelope.sequence,
            [("status", json!("failed")), ("error", json!(error))],
            &mut operations,
        )?,
        FlowEvent::RunCancelled { reason } => update_object(
            graph,
            &run_id,
            envelope.sequence,
            [("status", json!("cancelled")), ("reason", json!(reason))],
            &mut operations,
        )?,
        FlowEvent::StepCreated {
            step_id,
            step_name,
            input,
            retry,
        } => {
            let object_id = step_object_id(&envelope.run_id, step_id);
            operations.push(PatchOperation::AddObject {
                id: object_id.clone(),
                object_type: "workflow_step".to_string(),
                data: json!({"run_id": envelope.run_id, "step_id": step_id, "name": step_name,
                    "input": input, "retry": retry, "status": "created", "last_sequence": envelope.sequence}),
            });
            operations.push(PatchOperation::AddRelation {
                id: contains_relation_id(&envelope.run_id, "step", step_id),
                relation_type: "contains".to_string(),
                source: run_id.clone(),
                target: object_id,
                data: json!({"kind": "step"}),
            });
            touch_run(graph, &run_id, envelope.sequence, &mut operations)?;
        }
        FlowEvent::StepStarted { step_id, attempt } => update_subject(
            graph,
            &run_id,
            &step_object_id(&envelope.run_id, step_id),
            envelope.sequence,
            [("status", json!("running")), ("attempt", json!(attempt))],
            &mut operations,
        )?,
        FlowEvent::StepCompleted { step_id, output } => update_subject(
            graph,
            &run_id,
            &step_object_id(&envelope.run_id, step_id),
            envelope.sequence,
            [("status", json!("completed")), ("output", output.clone())],
            &mut operations,
        )?,
        FlowEvent::StepRetrying {
            step_id,
            attempt,
            error,
            retry_after,
        } => update_subject(
            graph,
            &run_id,
            &step_object_id(&envelope.run_id, step_id),
            envelope.sequence,
            [
                ("status", json!("retrying")),
                ("attempt", json!(attempt)),
                ("error", json!(error)),
                ("retry_after", json!(retry_after)),
            ],
            &mut operations,
        )?,
        FlowEvent::StepFailed {
            step_id,
            attempt,
            error,
        } => update_subject(
            graph,
            &run_id,
            &step_object_id(&envelope.run_id, step_id),
            envelope.sequence,
            [
                ("status", json!("failed")),
                ("attempt", json!(attempt)),
                ("error", json!(error)),
            ],
            &mut operations,
        )?,
        FlowEvent::WaitCreated { wait_id, resume_at } => add_subject(
            graph,
            SubjectProjection {
                run_object_id: &run_id,
                raw_run_id: &envelope.run_id,
                kind: "wait",
                id: wait_id,
                object_type: "workflow_wait",
                sequence: envelope.sequence,
                extra: json!({"resume_at": resume_at, "status": "waiting"}),
            },
            &mut operations,
        )?,
        FlowEvent::WaitCompleted { wait_id } => update_subject(
            graph,
            &run_id,
            &subject_object_id(&envelope.run_id, "wait", wait_id),
            envelope.sequence,
            [("status", json!("completed"))],
            &mut operations,
        )?,
        FlowEvent::HookCreated {
            hook_id,
            token: _,
            metadata,
        } => add_subject(
            graph,
            SubjectProjection {
                run_object_id: &run_id,
                raw_run_id: &envelope.run_id,
                kind: "hook",
                id: hook_id,
                object_type: "workflow_hook",
                sequence: envelope.sequence,
                extra: json!({"metadata": metadata, "status": "waiting"}),
            },
            &mut operations,
        )?,
        FlowEvent::HookReceived { hook_id, payload } => update_subject(
            graph,
            &run_id,
            &subject_object_id(&envelope.run_id, "hook", hook_id),
            envelope.sequence,
            [("status", json!("received")), ("payload", payload.clone())],
            &mut operations,
        )?,
        FlowEvent::HookDisposed { hook_id } => update_subject(
            graph,
            &run_id,
            &subject_object_id(&envelope.run_id, "hook", hook_id),
            envelope.sequence,
            [("status", json!("disposed"))],
            &mut operations,
        )?,
    }
    Ok(GraphPatch::new(graph.version(), operations))
}

fn update_subject<const N: usize>(
    graph: &crate::StateGraph,
    run_id: &str,
    subject_id: &str,
    sequence: u64,
    fields: [(&str, Value); N],
    operations: &mut Vec<PatchOperation>,
) -> Result<(), RuntimeError> {
    update_object(graph, subject_id, sequence, fields, operations)?;
    touch_run(graph, run_id, sequence, operations)
}

struct SubjectProjection<'a> {
    run_object_id: &'a str,
    raw_run_id: &'a str,
    kind: &'a str,
    id: &'a str,
    object_type: &'a str,
    sequence: u64,
    extra: Value,
}

fn add_subject(
    graph: &crate::StateGraph,
    subject: SubjectProjection<'_>,
    operations: &mut Vec<PatchOperation>,
) -> Result<(), RuntimeError> {
    let object_id = subject_object_id(subject.raw_run_id, subject.kind, subject.id);
    let mut data = subject.extra.as_object().cloned().unwrap_or_default();
    data.insert("run_id".to_string(), json!(subject.raw_run_id));
    data.insert(format!("{}_id", subject.kind), json!(subject.id));
    data.insert("last_sequence".to_string(), json!(subject.sequence));
    operations.push(PatchOperation::AddObject {
        id: object_id.clone(),
        object_type: subject.object_type.to_string(),
        data: Value::Object(data),
    });
    operations.push(PatchOperation::AddRelation {
        id: contains_relation_id(subject.raw_run_id, subject.kind, subject.id),
        relation_type: "contains".to_string(),
        source: subject.run_object_id.to_string(),
        target: object_id,
        data: json!({"kind": subject.kind}),
    });
    touch_run(graph, subject.run_object_id, subject.sequence, operations)
}

fn touch_run(
    graph: &crate::StateGraph,
    run_id: &str,
    sequence: u64,
    operations: &mut Vec<PatchOperation>,
) -> Result<(), RuntimeError> {
    update_object(graph, run_id, sequence, [], operations)
}

fn update_object<const N: usize>(
    graph: &crate::StateGraph,
    id: &str,
    sequence: u64,
    fields: [(&str, Value); N],
    operations: &mut Vec<PatchOperation>,
) -> Result<(), RuntimeError> {
    let object = graph.object(id).ok_or_else(|| {
        RuntimeError::InvalidExternalProjection(format!("projected object `{id}` does not exist"))
    })?;
    let mut data: Map<String, Value> = object.data.as_object().cloned().unwrap_or_default();
    for (key, value) in fields {
        data.insert(key.to_string(), value);
    }
    data.insert("last_sequence".to_string(), json!(sequence));
    operations.push(PatchOperation::UpdateObject {
        id: id.to_string(),
        expected_version: object.version,
        data: Value::Object(data),
    });
    Ok(())
}

pub fn run_object_id(run_id: &str) -> String {
    format!("flow:run:{run_id}")
}
pub fn step_object_id(run_id: &str, step_id: &str) -> String {
    subject_object_id(run_id, "step", step_id)
}
fn subject_object_id(run_id: &str, kind: &str, id: &str) -> String {
    format!("flow:{kind}:{run_id}:{id}")
}
fn contains_relation_id(run_id: &str, kind: &str, id: &str) -> String {
    format!("flow:contains:{run_id}:{kind}:{id}")
}

#[cfg(test)]
mod tests;
