//! OpenTelemetry Integration (feature-gated)
//!
//! Provides OTLP export for traces when the `telemetry` feature is enabled.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use a3s_code_core::telemetry_otel::TelemetryConfig;
//!
//! // Initialize with OTLP endpoint
//! let guard = TelemetryConfig::new("http://localhost:4317")
//!     .with_service_name("my-agent")
//!     .init()?;
//!
//! // ... run agent ...
//!
//! // Shutdown flushes all pending spans
//! guard.shutdown();
//! ```

use std::time::Duration;

use anyhow::{Context, Result};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::{BatchConfigBuilder, BatchSpanProcessor};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// A down collector must not hold process shutdown or a coding turn.
const COLLECTOR_EXPORT_BOUND: Duration = Duration::from_secs(1);

/// Configuration for OpenTelemetry export
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    /// OTLP endpoint (e.g., "http://localhost:4317")
    endpoint: String,
    /// Service name (default: "a3s-code")
    service_name: String,
    /// Enable trace export
    traces: bool,
    /// Log level filter (default: "info")
    log_filter: String,
    /// When set, export OTLP/HTTP protobuf instead of the default gRPC transport.
    /// The payload is still `ExportTraceServiceRequest`. Tests use this so a
    /// loopback collector can read the bytes without a gRPC stack.
    use_http: bool,
    /// Overrides the batch processor's scheduled export delay.
    export_delay: Option<Duration>,
}

impl TelemetryConfig {
    /// Create a new telemetry config with the given OTLP endpoint.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            service_name: crate::telemetry::SERVICE_NAME.to_string(),
            traces: true,
            log_filter: "info".to_string(),
            use_http: false,
            export_delay: None,
        }
    }

    /// Set the service name.
    pub fn with_service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = name.into();
        self
    }

    /// Enable or disable trace export.
    pub fn with_traces(mut self, enabled: bool) -> Self {
        self.traces = enabled;
        self
    }

    /// Set the log level filter (e.g., "info", "debug", "a3s_code=debug,info").
    pub fn with_log_filter(mut self, filter: impl Into<String>) -> Self {
        self.log_filter = filter.into();
        self
    }

    /// Export traces as OTLP/HTTP protobuf. Default remains gRPC.
    pub fn with_otlp_http(mut self) -> Self {
        self.use_http = true;
        self
    }

    /// How long the batch processor waits before sending a non-full batch.
    pub fn with_export_delay(mut self, delay: Duration) -> Self {
        self.export_delay = Some(delay);
        self
    }

    /// Initialize OpenTelemetry and return a guard that shuts down on drop.
    pub fn init(self) -> Result<TelemetryGuard> {
        let resource = opentelemetry_sdk::Resource::new(vec![opentelemetry::KeyValue::new(
            "service.name",
            self.service_name.clone(),
        )]);

        // Set up OTLP trace exporter
        let tracer_provider = if self.traces {
            let exporter = if self.use_http {
                opentelemetry_otlp::SpanExporter::builder()
                    .with_http()
                    .with_endpoint(&self.endpoint)
                    .with_timeout(COLLECTOR_EXPORT_BOUND)
                    .build()
                    .context("Failed to create OTLP HTTP span exporter")?
            } else {
                opentelemetry_otlp::SpanExporter::builder()
                    .with_tonic()
                    .with_endpoint(&self.endpoint)
                    .with_timeout(COLLECTOR_EXPORT_BOUND)
                    .build()
                    .context("Failed to create OTLP span exporter")?
            };
            let mut batch_config = BatchConfigBuilder::default()
                .with_max_export_timeout(COLLECTOR_EXPORT_BOUND)
                .build();
            if let Some(delay) = self.export_delay {
                batch_config = BatchConfigBuilder::default()
                    .with_max_export_timeout(COLLECTOR_EXPORT_BOUND)
                    .with_scheduled_delay(delay)
                    .build();
            }
            let batch = BatchSpanProcessor::builder(exporter, opentelemetry_sdk::runtime::Tokio)
                .with_batch_config(batch_config)
                .build();

            let provider = opentelemetry_sdk::trace::TracerProvider::builder()
                .with_resource(resource)
                .with_span_processor(batch)
                .build();

            opentelemetry::global::set_tracer_provider(provider.clone());
            Some(provider)
        } else {
            None
        };

        // Build tracing subscriber with OTel layer
        let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&self.log_filter));

        let fmt_layer = tracing_subscriber::fmt::layer().with_target(true);

        if let Some(ref provider) = tracer_provider {
            let tracer = provider.tracer(self.service_name.clone());
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt_layer)
                .with(otel_layer)
                .try_init()
                .context("Failed to initialize tracing subscriber with OTel")?;
        } else {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt_layer)
                .try_init()
                .context("Failed to initialize tracing subscriber")?;
        }

        tracing::info!(
            service = %self.service_name,
            endpoint = %self.endpoint,
            traces = self.traces,
            "OpenTelemetry initialized"
        );

        Ok(TelemetryGuard { tracer_provider })
    }
}

/// Guard that shuts down OpenTelemetry on drop.
///
/// Call `shutdown()` explicitly for graceful flush, or let it drop.
pub struct TelemetryGuard {
    tracer_provider: Option<opentelemetry_sdk::trace::TracerProvider>,
}

impl TelemetryGuard {
    /// Gracefully shutdown OpenTelemetry, flushing all pending data.
    pub fn shutdown(self) {
        drop(self);
    }
}

fn shutdown_provider(provider: opentelemetry_sdk::trace::TracerProvider) {
    let run = move || {
        if let Err(error) = provider.shutdown() {
            eprintln!("Failed to shutdown tracer provider: {error}");
        }
    };
    let on_runtime_worker = tokio::runtime::Handle::try_current()
        .ok()
        .is_some_and(|handle| {
            handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
        });
    if on_runtime_worker {
        // `TracerProvider::shutdown` uses `futures_executor::block_on`. Calling
        // it on a worker prevents the batch task from completing.
        tokio::task::block_in_place(run);
        return;
    }
    let (done, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        run();
        let _ = done.send(());
    });
    let _ = wait.recv_timeout(COLLECTOR_EXPORT_BOUND.saturating_add(Duration::from_secs(1)));
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.tracer_provider.take() {
            shutdown_provider(provider);
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_telemetry_config_defaults() {
        let config = TelemetryConfig::new("http://localhost:4317");
        assert_eq!(config.endpoint, "http://localhost:4317");
        assert_eq!(config.service_name, "a3s-code");
        assert!(config.traces);
        assert_eq!(config.log_filter, "info");
    }

    #[test]
    fn test_telemetry_config_builders() {
        let config = TelemetryConfig::new("http://otel:4317")
            .with_service_name("my-agent")
            .with_traces(false)
            .with_log_filter("debug");

        assert_eq!(config.endpoint, "http://otel:4317");
        assert_eq!(config.service_name, "my-agent");
        assert!(!config.traces);
        assert_eq!(config.log_filter, "debug");
    }

    #[test]
    fn test_telemetry_config_clone() {
        let config = TelemetryConfig::new("http://localhost:4317").with_service_name("test");
        let cloned = config.clone();
        assert_eq!(cloned.endpoint, "http://localhost:4317");
        assert_eq!(cloned.service_name, "test");
    }

    #[test]
    fn test_telemetry_config_debug() {
        let config = TelemetryConfig::new("http://localhost:4317");
        let debug = format!("{:?}", config);
        assert!(debug.contains("TelemetryConfig"));
        assert!(debug.contains("localhost:4317"));
    }
}
