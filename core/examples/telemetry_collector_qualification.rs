//! Hermetic OpenTelemetry qualification against a local OTLP collector.
//!
//! Run with `--features telemetry` while a collector listens on the endpoint in
//! `A3S_CODE_OTLP_TEST_ENDPOINT`. The workflow also checks collector-side logs,
//! so this process report covers bounded initialization and flush/shutdown while
//! the collector provides independent receipt evidence.

#![cfg(feature = "telemetry")]

use a3s_code_core::telemetry_otel::TelemetryConfig;
use serde_json::json;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const SHUTDOWN_BUDGET: Duration = Duration::from_secs(10);
const QUALIFICATION_SERVICE: &str = "a3s-code-hermetic";
const QUALIFICATION_SPAN: &str = "a3s.telemetry.qualification";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let endpoint = std::env::var("A3S_CODE_OTLP_TEST_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:4317".to_owned());
    let init_started = Instant::now();
    let guard = TelemetryConfig::new(&endpoint)
        .with_service_name(QUALIFICATION_SERVICE)
        .with_log_filter("info")
        .init()?;
    let init_elapsed = init_started.elapsed();

    {
        let span = tracing::info_span!(
            "a3s.telemetry.qualification",
            qualification.id = "controlled-local-collector",
            qualification.expected = true,
        );
        let _entered = span.enter();
        tracing::info!("emitting controlled OpenTelemetry qualification span");
    }

    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let shutdown = std::thread::spawn(move || {
        let started = Instant::now();
        guard.shutdown();
        let _ = finished_tx.send(started.elapsed());
    });
    let shutdown_elapsed = finished_rx.recv_timeout(SHUTDOWN_BUDGET).ok();
    if shutdown_elapsed.is_some() {
        shutdown
            .join()
            .map_err(|_| anyhow::anyhow!("telemetry shutdown thread panicked"))?;
    }
    let passed = shutdown_elapsed.is_some_and(|elapsed| elapsed <= SHUTDOWN_BUDGET);

    let report = json!({
        "schemaVersion": 1,
        "profile": "opentelemetry-local-collector-v1",
        "endpoint": endpoint,
        "serviceName": QUALIFICATION_SERVICE,
        "spanName": QUALIFICATION_SPAN,
        "initMs": duration_ms(init_elapsed),
        "shutdownMs": shutdown_elapsed.map(duration_ms),
        "shutdownBudgetMs": SHUTDOWN_BUDGET.as_millis(),
        "collectorReceiptVerifiedByWorkflow": true,
        "providerNetworkIncluded": false,
        "passed": passed,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);

    if !passed {
        anyhow::bail!("OpenTelemetry flush/shutdown exceeded its 10 second budget");
    }
    Ok(())
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}
