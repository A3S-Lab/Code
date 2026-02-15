//! OpenTelemetry Telemetry Initialization (Server-only)
//!
//! Provides OTLP exporter setup and subscriber initialization.
//! This module depends on `tonic` (via `opentelemetry-otlp`) and is
//! intentionally kept out of `a3s-code-core` to keep the core library
//! free of gRPC dependencies.
//!
//! For telemetry constants, span helpers, and cost tracking types,
//! see `a3s_code_core::telemetry`.

use a3s_code_core::telemetry::SERVICE_NAME;
use opentelemetry::global;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Configuration for telemetry initialization
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    /// OTLP endpoint (e.g., "http://localhost:4317")
    pub otlp_endpoint: Option<String>,
    /// Service version
    pub service_version: String,
    /// Enable console tracing output
    pub console_output: bool,
    /// Output logs in JSON format (default: human-readable)
    pub json_log: bool,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            otlp_endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok(),
            service_version: env!("CARGO_PKG_VERSION").to_string(),
            console_output: true,
            json_log: false,
        }
    }
}

/// Initialize the telemetry pipeline.
///
/// When `otlp_endpoint` is configured, traces are exported via OTLP.
/// Otherwise, only console tracing is enabled (backward compatible).
pub fn init_telemetry(config: &TelemetryConfig) {
    let env_filter = EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into());

    if let Some(ref endpoint) = config.otlp_endpoint {
        // OTLP exporter configured — set up full OpenTelemetry pipeline
        match init_otlp_tracer(endpoint, &config.service_version) {
            Ok(tracer) => {
                if config.console_output {
                    if config.json_log {
                        tracing_subscriber::registry()
                            .with(env_filter)
                            .with(tracing_subscriber::fmt::layer().json().flatten_event(true))
                            .with(tracing_opentelemetry::layer().with_tracer(tracer))
                            .init();
                    } else {
                        tracing_subscriber::registry()
                            .with(env_filter)
                            .with(tracing_subscriber::fmt::layer())
                            .with(tracing_opentelemetry::layer().with_tracer(tracer))
                            .init();
                    }
                } else {
                    tracing_subscriber::registry()
                        .with(env_filter)
                        .with(tracing_opentelemetry::layer().with_tracer(tracer))
                        .init();
                }

                tracing::info!(
                    endpoint = endpoint.as_str(),
                    "OpenTelemetry OTLP exporter initialized"
                );
            }
            Err(e) => {
                // Fall back to console-only tracing
                if config.json_log {
                    tracing_subscriber::fmt()
                        .json()
                        .flatten_event(true)
                        .with_env_filter(env_filter)
                        .init();
                } else {
                    tracing_subscriber::fmt().with_env_filter(env_filter).init();
                }
                tracing::warn!(
                    "Failed to initialize OTLP exporter: {}. Using console only.",
                    e
                );
            }
        }
    } else {
        // No OTLP endpoint — console tracing only (backward compatible)
        if config.json_log {
            tracing_subscriber::fmt()
                .json()
                .flatten_event(true)
                .with_env_filter(env_filter)
                .init();
        } else {
            tracing_subscriber::fmt().with_env_filter(env_filter).init();
        }
    }
}

/// Create an OTLP trace exporter and return a tracer
fn init_otlp_tracer(
    endpoint: &str,
    service_version: &str,
) -> Result<opentelemetry_sdk::trace::Tracer, opentelemetry::trace::TraceError> {
    let exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_endpoint(endpoint);

    let resource = Resource::new(vec![
        KeyValue::new(
            opentelemetry_semantic_conventions::resource::SERVICE_NAME,
            SERVICE_NAME,
        ),
        KeyValue::new(
            opentelemetry_semantic_conventions::resource::SERVICE_VERSION,
            service_version.to_string(),
        ),
    ]);

    opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(exporter)
        .with_trace_config(opentelemetry_sdk::trace::config().with_resource(resource))
        .install_batch(opentelemetry_sdk::runtime::Tokio)
}

/// Shutdown the telemetry pipeline gracefully
pub fn shutdown_telemetry() {
    global::shutdown_tracer_provider();
}

/// Record LLM metrics to OpenTelemetry counters (fire-and-forget)
pub fn record_llm_metrics(
    model: &str,
    prompt_tokens: usize,
    completion_tokens: usize,
    cost_usd: f64,
    _duration_secs: f64,
) {
    let meter = global::meter(SERVICE_NAME);

    let token_counter = meter
        .u64_counter("a3s.llm.tokens")
        .with_description("Total LLM tokens consumed")
        .init();
    token_counter.add(
        (prompt_tokens + completion_tokens) as u64,
        &[KeyValue::new("model", model.to_string())],
    );

    let cost_counter = meter
        .f64_counter("a3s.llm.cost_usd")
        .with_description("Total LLM cost in USD")
        .init();
    cost_counter.add(cost_usd, &[KeyValue::new("model", model.to_string())]);
}
