//! OpenTelemetry Telemetry Module
//!
//! Provides centralized observability for the A3S Code agent:
//! - Distributed tracing with structured spans
//! - LLM cost tracking (model, tokens, cost per call)
//! - Tool execution metrics (duration, success/failure)
//! - Multi-agent trace context propagation
//!
//! ## Span Hierarchy
//!
//! ```text
//! a3s.agent.execute
//!   ├── a3s.agent.context_resolve
//!   ├── a3s.agent.turn (repeated)
//!   │   ├── a3s.llm.completion
//!   │   └── a3s.tool.execute (repeated)
//!   └── a3s.agent.turn_notify
//! ```

use opentelemetry::global;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use std::time::Instant;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

// ============================================================================
// Constants
// ============================================================================

/// Service name for OpenTelemetry resource
const SERVICE_NAME: &str = "a3s-code";

// Span name constants
pub const SPAN_AGENT_EXECUTE: &str = "a3s.agent.execute";
pub const SPAN_AGENT_TURN: &str = "a3s.agent.turn";
pub const SPAN_LLM_COMPLETION: &str = "a3s.llm.completion";
pub const SPAN_TOOL_EXECUTE: &str = "a3s.tool.execute";
pub const SPAN_CONTEXT_RESOLVE: &str = "a3s.agent.context_resolve";

// Attribute key constants
pub const ATTR_SESSION_ID: &str = "a3s.session.id";
pub const ATTR_TURN_NUMBER: &str = "a3s.agent.turn_number";
pub const ATTR_MAX_TURNS: &str = "a3s.agent.max_turns";
pub const ATTR_TOOL_CALLS_COUNT: &str = "a3s.agent.tool_calls_count";

pub const ATTR_LLM_MODEL: &str = "a3s.llm.model";
pub const ATTR_LLM_PROVIDER: &str = "a3s.llm.provider";
pub const ATTR_LLM_STREAMING: &str = "a3s.llm.streaming";
pub const ATTR_LLM_PROMPT_TOKENS: &str = "a3s.llm.prompt_tokens";
pub const ATTR_LLM_COMPLETION_TOKENS: &str = "a3s.llm.completion_tokens";
pub const ATTR_LLM_TOTAL_TOKENS: &str = "a3s.llm.total_tokens";
pub const ATTR_LLM_STOP_REASON: &str = "a3s.llm.stop_reason";

pub const ATTR_TOOL_NAME: &str = "a3s.tool.name";
pub const ATTR_TOOL_ID: &str = "a3s.tool.id";
pub const ATTR_TOOL_EXIT_CODE: &str = "a3s.tool.exit_code";
pub const ATTR_TOOL_SUCCESS: &str = "a3s.tool.success";
pub const ATTR_TOOL_DURATION_MS: &str = "a3s.tool.duration_ms";
pub const ATTR_TOOL_PERMISSION: &str = "a3s.tool.permission";

pub const ATTR_CONTEXT_PROVIDERS: &str = "a3s.context.providers";
pub const ATTR_CONTEXT_ITEMS: &str = "a3s.context.items";
pub const ATTR_CONTEXT_TOKENS: &str = "a3s.context.tokens";

// ============================================================================
// Initialization
// ============================================================================

/// Configuration for telemetry initialization
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    /// OTLP endpoint (e.g., "http://localhost:4317")
    pub otlp_endpoint: Option<String>,
    /// Service version
    pub service_version: String,
    /// Enable console tracing output
    pub console_output: bool,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            otlp_endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok(),
            service_version: env!("CARGO_PKG_VERSION").to_string(),
            console_output: true,
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
                    tracing_subscriber::registry()
                        .with(env_filter)
                        .with(tracing_subscriber::fmt::layer())
                        .with(tracing_opentelemetry::layer().with_tracer(tracer))
                        .init();
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
                tracing_subscriber::fmt().with_env_filter(env_filter).init();
                tracing::warn!(
                    "Failed to initialize OTLP exporter: {}. Using console only.",
                    e
                );
            }
        }
    } else {
        // No OTLP endpoint — console tracing only (backward compatible)
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
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

// ============================================================================
// Span Helpers
// ============================================================================

/// Record LLM token usage on the current span
pub fn record_llm_usage(
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
    stop_reason: Option<&str>,
) {
    let span = tracing::Span::current();
    span.record(ATTR_LLM_PROMPT_TOKENS, prompt_tokens as i64);
    span.record(ATTR_LLM_COMPLETION_TOKENS, completion_tokens as i64);
    span.record(ATTR_LLM_TOTAL_TOKENS, total_tokens as i64);
    if let Some(reason) = stop_reason {
        span.record(ATTR_LLM_STOP_REASON, reason);
    }
}

/// Record tool execution result on the current span
pub fn record_tool_result(exit_code: i32, duration: std::time::Duration) {
    let span = tracing::Span::current();
    span.record(ATTR_TOOL_EXIT_CODE, exit_code as i64);
    span.record(ATTR_TOOL_SUCCESS, exit_code == 0);
    span.record(ATTR_TOOL_DURATION_MS, duration.as_millis() as i64);
}

/// A guard that measures elapsed time and records it when dropped
pub struct TimedSpan {
    start: Instant,
    span_field: &'static str,
}

impl TimedSpan {
    pub fn new(span_field: &'static str) -> Self {
        Self {
            start: Instant::now(),
            span_field,
        }
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
}

impl Drop for TimedSpan {
    fn drop(&mut self) {
        let elapsed_ms = self.elapsed_ms();
        let span = tracing::Span::current();
        span.record(self.span_field, elapsed_ms as i64);
    }
}

// ============================================================================
// LLM Cost Tracking
// ============================================================================

/// Cost record for a single LLM call
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LlmCostRecord {
    /// Model identifier
    pub model: String,
    /// Provider name
    pub provider: String,
    /// Input tokens
    pub prompt_tokens: usize,
    /// Output tokens
    pub completion_tokens: usize,
    /// Total tokens
    pub total_tokens: usize,
    /// Estimated cost in USD (if pricing is configured)
    pub cost_usd: Option<f64>,
    /// Timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Session ID
    pub session_id: Option<String>,
}

/// Pricing table for LLM models (cost per 1M tokens)
#[derive(Debug, Clone)]
pub struct ModelPricing {
    /// Cost per 1M input tokens in USD
    pub input_per_million: f64,
    /// Cost per 1M output tokens in USD
    pub output_per_million: f64,
}

impl ModelPricing {
    pub fn new(input_per_million: f64, output_per_million: f64) -> Self {
        Self {
            input_per_million,
            output_per_million,
        }
    }

    /// Calculate cost for given token counts
    pub fn calculate_cost(&self, prompt_tokens: usize, completion_tokens: usize) -> f64 {
        let input_cost = (prompt_tokens as f64 / 1_000_000.0) * self.input_per_million;
        let output_cost = (completion_tokens as f64 / 1_000_000.0) * self.output_per_million;
        input_cost + output_cost
    }
}

/// Registry of known model pricing
pub fn default_model_pricing() -> std::collections::HashMap<String, ModelPricing> {
    let mut pricing = std::collections::HashMap::new();

    // Anthropic Claude models
    pricing.insert(
        "claude-sonnet-4-20250514".to_string(),
        ModelPricing::new(3.0, 15.0),
    );
    pricing.insert(
        "claude-3-5-sonnet-20241022".to_string(),
        ModelPricing::new(3.0, 15.0),
    );
    pricing.insert(
        "claude-3-haiku-20240307".to_string(),
        ModelPricing::new(0.25, 1.25),
    );
    pricing.insert(
        "claude-3-opus-20240229".to_string(),
        ModelPricing::new(15.0, 75.0),
    );

    // OpenAI models
    pricing.insert("gpt-4o".to_string(), ModelPricing::new(2.5, 10.0));
    pricing.insert("gpt-4o-mini".to_string(), ModelPricing::new(0.15, 0.6));
    pricing.insert("gpt-4-turbo".to_string(), ModelPricing::new(10.0, 30.0));

    pricing
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_telemetry_config_default() {
        let config = TelemetryConfig::default();
        assert!(config.console_output);
        assert_eq!(config.service_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn test_telemetry_config_custom() {
        let config = TelemetryConfig {
            otlp_endpoint: Some("http://localhost:4317".to_string()),
            service_version: "1.0.0".to_string(),
            console_output: false,
        };
        assert_eq!(
            config.otlp_endpoint,
            Some("http://localhost:4317".to_string())
        );
        assert!(!config.console_output);
    }

    #[test]
    fn test_model_pricing_calculation() {
        let pricing = ModelPricing::new(3.0, 15.0); // Claude Sonnet pricing

        // 1000 input tokens + 500 output tokens
        let cost = pricing.calculate_cost(1000, 500);
        let expected = (1000.0 / 1_000_000.0) * 3.0 + (500.0 / 1_000_000.0) * 15.0;
        assert!((cost - expected).abs() < f64::EPSILON);
    }

    #[test]
    fn test_model_pricing_zero_tokens() {
        let pricing = ModelPricing::new(3.0, 15.0);
        let cost = pricing.calculate_cost(0, 0);
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn test_model_pricing_large_tokens() {
        let pricing = ModelPricing::new(3.0, 15.0);
        // 1M input + 1M output
        let cost = pricing.calculate_cost(1_000_000, 1_000_000);
        assert!((cost - 18.0).abs() < f64::EPSILON); // $3 + $15
    }

    #[test]
    fn test_default_model_pricing_has_known_models() {
        let pricing = default_model_pricing();
        assert!(pricing.contains_key("claude-sonnet-4-20250514"));
        assert!(pricing.contains_key("claude-3-5-sonnet-20241022"));
        assert!(pricing.contains_key("claude-3-haiku-20240307"));
        assert!(pricing.contains_key("gpt-4o"));
        assert!(pricing.contains_key("gpt-4o-mini"));
    }

    #[test]
    fn test_llm_cost_record_serialize() {
        let record = LlmCostRecord {
            model: "claude-sonnet-4-20250514".to_string(),
            provider: "anthropic".to_string(),
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
            cost_usd: Some(0.0105),
            timestamp: chrono::Utc::now(),
            session_id: Some("sess-123".to_string()),
        };

        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("claude-sonnet-4-20250514"));
        assert!(json.contains("anthropic"));
        assert!(json.contains("1000"));

        // Deserialize back
        let deserialized: LlmCostRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.model, "claude-sonnet-4-20250514");
        assert_eq!(deserialized.prompt_tokens, 1000);
    }

    #[test]
    fn test_timed_span_elapsed() {
        let timer = TimedSpan::new(ATTR_TOOL_DURATION_MS);
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(timer.elapsed_ms() >= 10);
    }

    #[test]
    fn test_span_name_constants() {
        // Verify span names follow a3s.* convention
        assert!(SPAN_AGENT_EXECUTE.starts_with("a3s."));
        assert!(SPAN_AGENT_TURN.starts_with("a3s."));
        assert!(SPAN_LLM_COMPLETION.starts_with("a3s."));
        assert!(SPAN_TOOL_EXECUTE.starts_with("a3s."));
        assert!(SPAN_CONTEXT_RESOLVE.starts_with("a3s."));
    }

    #[test]
    fn test_attribute_key_constants() {
        // Verify attribute keys follow a3s.* convention
        assert!(ATTR_SESSION_ID.starts_with("a3s."));
        assert!(ATTR_LLM_MODEL.starts_with("a3s."));
        assert!(ATTR_TOOL_NAME.starts_with("a3s."));
        assert!(ATTR_CONTEXT_PROVIDERS.starts_with("a3s."));
    }

    #[test]
    fn test_record_llm_usage_does_not_panic() {
        // record_llm_usage should not panic even without an active span
        record_llm_usage(100, 50, 150, Some("end_turn"));
        record_llm_usage(0, 0, 0, None);
        record_llm_usage(1_000_000, 500_000, 1_500_000, Some("max_tokens"));
    }

    #[test]
    fn test_record_tool_result_does_not_panic() {
        // record_tool_result should not panic even without an active span
        record_tool_result(0, std::time::Duration::from_millis(100));
        record_tool_result(1, std::time::Duration::from_secs(0));
        record_tool_result(-1, std::time::Duration::from_secs(30));
    }

    #[test]
    fn test_timed_span_measures_duration() {
        let timer = TimedSpan::new(ATTR_TOOL_DURATION_MS);
        // Immediately check — should be very small
        assert!(timer.elapsed_ms() < 1000);
    }

    #[test]
    fn test_model_pricing_registry_completeness() {
        let pricing = default_model_pricing();
        // Verify all expected providers are covered
        let anthropic_models: Vec<&str> = pricing
            .keys()
            .filter(|k| k.starts_with("claude"))
            .map(|k| k.as_str())
            .collect();
        assert!(
            anthropic_models.len() >= 3,
            "Expected at least 3 Anthropic models, got {}",
            anthropic_models.len()
        );

        let openai_models: Vec<&str> = pricing
            .keys()
            .filter(|k| k.starts_with("gpt"))
            .map(|k| k.as_str())
            .collect();
        assert!(
            openai_models.len() >= 2,
            "Expected at least 2 OpenAI models, got {}",
            openai_models.len()
        );
    }

    #[test]
    fn test_model_pricing_cost_ordering() {
        let pricing = default_model_pricing();
        // Haiku should be cheaper than Sonnet
        let haiku = pricing.get("claude-3-haiku-20240307").unwrap();
        let sonnet = pricing.get("claude-sonnet-4-20250514").unwrap();
        assert!(
            haiku.input_per_million < sonnet.input_per_million,
            "Haiku should be cheaper than Sonnet"
        );

        // GPT-4o-mini should be cheaper than GPT-4o
        let mini = pricing.get("gpt-4o-mini").unwrap();
        let full = pricing.get("gpt-4o").unwrap();
        assert!(
            mini.input_per_million < full.input_per_million,
            "GPT-4o-mini should be cheaper than GPT-4o"
        );
    }

    #[test]
    fn test_llm_cost_record_fields() {
        let record = LlmCostRecord {
            model: "gpt-4o".to_string(),
            provider: "openai".to_string(),
            prompt_tokens: 500,
            completion_tokens: 200,
            total_tokens: 700,
            cost_usd: None,
            timestamp: chrono::Utc::now(),
            session_id: None,
        };
        assert_eq!(record.total_tokens, record.prompt_tokens + record.completion_tokens);
        assert!(record.cost_usd.is_none());
        assert!(record.session_id.is_none());
    }

    #[test]
    fn test_attribute_keys_are_unique() {
        // All attribute keys should be distinct
        let keys = vec![
            ATTR_SESSION_ID,
            ATTR_TURN_NUMBER,
            ATTR_MAX_TURNS,
            ATTR_TOOL_CALLS_COUNT,
            ATTR_LLM_MODEL,
            ATTR_LLM_PROVIDER,
            ATTR_LLM_STREAMING,
            ATTR_LLM_PROMPT_TOKENS,
            ATTR_LLM_COMPLETION_TOKENS,
            ATTR_LLM_TOTAL_TOKENS,
            ATTR_LLM_STOP_REASON,
            ATTR_TOOL_NAME,
            ATTR_TOOL_ID,
            ATTR_TOOL_EXIT_CODE,
            ATTR_TOOL_SUCCESS,
            ATTR_TOOL_DURATION_MS,
            ATTR_TOOL_PERMISSION,
            ATTR_CONTEXT_PROVIDERS,
            ATTR_CONTEXT_ITEMS,
            ATTR_CONTEXT_TOKENS,
        ];
        let unique: std::collections::HashSet<&str> = keys.iter().copied().collect();
        assert_eq!(
            keys.len(),
            unique.len(),
            "Attribute keys must be unique"
        );
    }
}
