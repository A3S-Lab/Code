use a3s_code_core::{flow_run_object_id, FlowGraphObserver, GraphRuntime, RuntimeLimits};
use a3s_flow::{FlowEvent, FlowEventEnvelope, RetryPolicy, WorkflowSpec};
use chrono::Utc;
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use uuid::Uuid;

fn envelope(sequence: u64, event: FlowEvent) -> FlowEventEnvelope {
    FlowEventEnvelope {
        run_id: "benchmark-run".to_string(),
        sequence,
        event_id: Uuid::new_v4(),
        timestamp: Utc::now(),
        event,
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let steps = std::env::args()
        .nth(1)
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(1_000);
    let flow_events = steps
        .checked_mul(2)
        .and_then(|value| value.checked_add(2))
        .ok_or_else(|| anyhow::anyhow!("benchmark size overflow"))?;
    let max_events = flow_events
        .checked_mul(8)
        .and_then(|value| value.checked_add(100))
        .ok_or_else(|| anyhow::anyhow!("graph event limit overflow"))?;
    let runtime = Arc::new(Mutex::new(GraphRuntime::with_limits(RuntimeLimits {
        max_events,
        max_behavior_depth: 64,
    })));
    let observer = FlowGraphObserver::new(Arc::clone(&runtime));
    let started = Instant::now();
    observer
        .project(envelope(
            1,
            FlowEvent::RunCreated {
                spec: WorkflowSpec::rust_embedded("benchmark", "1", "bench", "run"),
                input: json!({"steps": steps}),
            },
        ))
        .await?;
    observer.project(envelope(2, FlowEvent::RunStarted)).await?;
    let mut sequence = 3;
    for index in 0..steps {
        let step_id = format!("step-{index}");
        observer
            .project(envelope(
                sequence,
                FlowEvent::StepCreated {
                    step_id: step_id.clone(),
                    step_name: "benchmark_tool".to_string(),
                    input: json!({"index": index}),
                    retry: RetryPolicy::none(),
                },
            ))
            .await?;
        sequence += 1;
        observer
            .project(envelope(
                sequence,
                FlowEvent::StepCompleted {
                    step_id,
                    output: json!({"ok": true}),
                },
            ))
            .await?;
        sequence += 1;
    }
    let projection_elapsed = started.elapsed();
    let health = observer.health().await;
    let runtime = runtime.lock().await;
    let records = runtime.events().to_vec();
    let objects = runtime.graph().objects().count();
    let relations = runtime.graph().relations().count();
    assert!(runtime
        .graph()
        .object(&flow_run_object_id("benchmark-run"))
        .is_some());
    drop(runtime);

    let replay_started = Instant::now();
    let restored = GraphRuntime::restore(records.clone())?;
    let replay_elapsed = replay_started.elapsed();
    assert_eq!(restored.events().len(), records.len());

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "steps": steps,
            "flow_events": flow_events,
            "graph_records": records.len(),
            "objects": objects,
            "relations": relations,
            "projection_elapsed_ms": projection_elapsed.as_millis(),
            "projection_events_per_second": flow_events as f64 / projection_elapsed.as_secs_f64(),
            "replay_elapsed_ms": replay_elapsed.as_millis(),
            "replay_records_per_second": records.len() as f64 / replay_elapsed.as_secs_f64(),
            "health": health,
        }))?
    );
    Ok(())
}
