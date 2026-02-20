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

use anyhow::{Context, Result};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

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
}

impl TelemetryConfig {
    /// Create a new telemetry config with the given OTLP endpoint.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            service_name: crate::telemetry::SERVICE_NAME.to_string(),
            traces: true,
            log_filter: "info".to_string(),
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

    /// Initialize OpenTelemetry and return a guard that shuts down on drop.
    pub fn init(self) -> Result<TelemetryGuard> {
        let resource = opentelemetry_sdk::Resource::new(vec![opentelemetry::KeyValue::new(
            "service.name",
            self.service_name.clone(),
        )]);

        // Set up OTLP trace exporter
        let tracer_provider = if self.traces {
            let exporter = opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .with_endpoint(&self.endpoint)
                .build()
                .context("Failed to create OTLP span exporter")?;

            let provider = opentelemetry_sdk::trace::TracerProvider::builder()
                .with_resource(resource)
                .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
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

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(ref provider) = self.tracer_provider {
            if let Err(e) = provider.shutdown() {
                eprintln!("Failed to shutdown tracer provider: {e}");
            }
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
