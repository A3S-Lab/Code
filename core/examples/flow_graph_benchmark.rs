//! Release qualification for large Flow-to-State-Graph projection and replay.
//!
//! Run from the Code repository root:
//!
//! `cargo run --locked --release -p a3s-code-core --example flow_graph_benchmark`

use a3s_code_core::{flow_run_object_id, FlowGraphObserver, GraphRuntime, RuntimeLimits};
use a3s_flow::{FlowEvent, FlowEventEnvelope, RetryPolicy, WorkflowSpec};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use uuid::Uuid;

const DEFAULT_STEPS: usize = 1_000;
const WARMUP_SAMPLES: usize = 3;
const MEASURED_SAMPLES: usize = 20;
const PROJECTION_P95_BUDGET_MS: f64 = 2_000.0;
const REPLAY_P95_BUDGET_MS: f64 = 2_000.0;
const SERIALIZED_EVENT_BUDGET_BYTES: usize = 64 * 1024 * 1024;

fn envelope(sequence: u64, event: FlowEvent) -> FlowEventEnvelope {
    FlowEventEnvelope {
        run_id: "benchmark-run".to_string(),
        sequence,
        event_id: Uuid::from_u128(u128::from(sequence).saturating_add(1)),
        // Resource accounting must not change with wall-clock formatting or
        // fractional-second precision. Sequence and event id still provide
        // the stable event ordering and identity this profile needs.
        timestamp: DateTime::<Utc>::UNIX_EPOCH,
        event,
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
struct Latency {
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}

#[derive(Debug)]
struct Sample {
    projection: Duration,
    replay: Duration,
    graph_records: usize,
    objects: usize,
    relations: usize,
    serialized_event_bytes: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if cfg!(debug_assertions) {
        anyhow::bail!("Flow and State Graph qualification must run with --release");
    }

    let steps = std::env::args()
        .nth(1)
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(DEFAULT_STEPS);
    if steps == 0 {
        anyhow::bail!("benchmark steps must be greater than zero");
    }

    for _ in 0..WARMUP_SAMPLES {
        run_sample(steps).await?;
    }

    let mut samples = Vec::with_capacity(MEASURED_SAMPLES);
    for _ in 0..MEASURED_SAMPLES {
        samples.push(run_sample(steps).await?);
    }
    let reference = samples
        .first()
        .ok_or_else(|| anyhow::anyhow!("benchmark produced no samples"))?;
    if samples.iter().any(|sample| {
        sample.graph_records != reference.graph_records
            || sample.objects != reference.objects
            || sample.relations != reference.relations
            || sample.serialized_event_bytes != reference.serialized_event_bytes
    }) {
        anyhow::bail!("Flow projection produced nondeterministic resource counts");
    }

    let projection = latency(samples.iter().map(|sample| sample.projection).collect())?;
    let replay = latency(samples.iter().map(|sample| sample.replay).collect())?;
    let projection_passed = projection.p95_ms <= PROJECTION_P95_BUDGET_MS;
    let replay_passed = replay.p95_ms <= REPLAY_P95_BUDGET_MS;
    let resource_passed = reference.serialized_event_bytes <= SERIALIZED_EVENT_BUDGET_BYTES;
    let passed = projection_passed && replay_passed && resource_passed;
    let flow_events = flow_event_count(steps)?;

    let report = json!({
        "schemaVersion": 1,
        "profile": "flow-state-graph-v1",
        "build": "release",
        "machine": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "logicalCpus": std::thread::available_parallelism()
                .map(|value| value.get())
                .unwrap_or(1),
            "processor": std::env::var("PROCESSOR_IDENTIFIER").ok(),
        },
        "parameters": {
            "steps": steps,
            "flowEvents": flow_events,
            "warmupSamples": WARMUP_SAMPLES,
            "measuredSamples": MEASURED_SAMPLES,
        },
        "projection": {
            "p50Ms": projection.p50_ms,
            "p95Ms": projection.p95_ms,
            "maxMs": projection.max_ms,
            "p50FlowEventsPerSecond": throughput(flow_events, projection.p50_ms),
            "budgetP95Ms": PROJECTION_P95_BUDGET_MS,
            "passed": projection_passed,
        },
        "replay": {
            "p50Ms": replay.p50_ms,
            "p95Ms": replay.p95_ms,
            "maxMs": replay.max_ms,
            "p50GraphRecordsPerSecond": throughput(reference.graph_records, replay.p50_ms),
            "budgetP95Ms": REPLAY_P95_BUDGET_MS,
            "passed": replay_passed,
        },
        "resources": {
            "graphRecords": reference.graph_records,
            "objects": reference.objects,
            "relations": reference.relations,
            "serializedEventBytes": reference.serialized_event_bytes,
            "serializedEventBudgetBytes": SERIALIZED_EVENT_BUDGET_BYTES,
            "passed": resource_passed,
        },
        "providerNetworkIncluded": false,
        "filesystemIncluded": false,
        "passed": passed,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);

    if !passed {
        anyhow::bail!(
            "Flow/State Graph qualification failed: projection p95 {:.3} ms, replay p95 {:.3} ms, serialized events {} bytes",
            projection.p95_ms,
            replay.p95_ms,
            reference.serialized_event_bytes,
        );
    }
    Ok(())
}

async fn run_sample(steps: usize) -> anyhow::Result<Sample> {
    let flow_events = flow_event_count(steps)?;
    let max_events = flow_events
        .checked_mul(8)
        .and_then(|value| value.checked_add(100))
        .ok_or_else(|| anyhow::anyhow!("graph event limit overflow"))?;
    let runtime = Arc::new(Mutex::new(GraphRuntime::with_limits(RuntimeLimits {
        max_events,
        max_behavior_depth: 64,
    })));
    let observer = FlowGraphObserver::new(Arc::clone(&runtime));

    let projection_started = Instant::now();
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
    let projection = projection_started.elapsed();

    let runtime = runtime.lock().await;
    let records = runtime.events().to_vec();
    let objects = runtime.graph().objects().count();
    let relations = runtime.graph().relations().count();
    if runtime
        .graph()
        .object(&flow_run_object_id("benchmark-run"))
        .is_none()
    {
        anyhow::bail!("Flow run object is missing after projection");
    }
    drop(runtime);

    let serialized_event_bytes = serde_json::to_vec(&records)?.len();
    let replay_started = Instant::now();
    let restored = GraphRuntime::restore(records.clone())?;
    let replay = replay_started.elapsed();
    if restored.events().len() != records.len() {
        anyhow::bail!("State Graph replay changed the event count");
    }
    if restored.graph().objects().count() != objects
        || restored.graph().relations().count() != relations
    {
        anyhow::bail!("State Graph replay changed the projection shape");
    }

    Ok(Sample {
        projection,
        replay,
        graph_records: records.len(),
        objects,
        relations,
        serialized_event_bytes,
    })
}

fn flow_event_count(steps: usize) -> anyhow::Result<usize> {
    steps
        .checked_mul(2)
        .and_then(|value| value.checked_add(2))
        .ok_or_else(|| anyhow::anyhow!("benchmark size overflow"))
}

fn latency(mut durations: Vec<Duration>) -> anyhow::Result<Latency> {
    if durations.is_empty() {
        anyhow::bail!("benchmark has no measured durations");
    }
    durations.sort_unstable();
    Ok(Latency {
        p50_ms: percentile_ms(&durations, 50),
        p95_ms: percentile_ms(&durations, 95),
        max_ms: duration_ms(*durations.last().expect("durations are not empty")),
    })
}

fn percentile_ms(durations: &[Duration], percentile: usize) -> f64 {
    let rank = (durations.len() * percentile)
        .div_ceil(100)
        .saturating_sub(1);
    duration_ms(durations[rank.min(durations.len() - 1)])
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn throughput(items: usize, elapsed_ms: f64) -> f64 {
    if elapsed_ms <= f64::EPSILON {
        return items as f64;
    }
    items as f64 * 1_000.0 / elapsed_ms
}
