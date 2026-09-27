//! This binary owns the process-global tracer.

use std::time::Duration;

use a3s_code_telemetry::config::{TelemetryConfig, TelemetryMode};
use a3s_code_test_support::{MockInferenceServer, MockOtelServer, OtelSpan};
use serde_json::Value;

const SILENT: &str = "018f6b6c-7b3a-7c3a-8c3a-000000000001";
const ENABLED: &str = "018f6b6c-7b3a-7c3a-8c3a-000000000002";
const CANARY_PATH: &str = "/tmp/secret-project/note.txt";
const CANARY_BODY: &str = "CANARY_BODY";

fn set_env(key: &str, value: &str) {
    // SAFETY: this binary has one test, and env is set before other threads start.
    unsafe { std::env::set_var(key, value) }
}

fn unset_env(key: &str) {
    // SAFETY: see [`set_env`].
    unsafe { std::env::remove_var(key) }
}

fn install_tracing(traces: &str) {
    set_env("GROK_INTERNAL_OTLP_TRACES_ENDPOINT", traces);
    set_env("GROK_INSTRUMENTATION", "server");
    set_env("GROK_OTEL_FILTER", "info");
    set_env("OTEL_BSP_SCHEDULE_DELAY", "50");
    set_env("OTEL_EXPORTER_OTLP_TIMEOUT", "2000");
    set_env("OTEL_TRACES_EXPORTER", "otlp");
    set_env("GROK_TELEMETRY_ENABLED", "true");
    unset_env("DISABLE_TELEMETRY");
    unset_env("GROK_EXTERNAL_OTEL");
    let config = a3s_code_shell::agent::init::build_default_otel_layer_config();
    a3s_code_shell::auth::credential_provider::wire_otel_deployment_key("test-key".into());
    let layer = a3s_code_telemetry::otel_layer::build_otel_layer(
        a3s_code_telemetry::otel_layer::OtelClientInfo {
            client_name: "grok-test",
            client_version: "test",
            service_version: "test",
            app_entrypoint: "cli",
        },
        config,
    );
    use tracing_subscriber::layer::SubscriberExt as _;
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(layer))
        .expect("install subscriber");
}

fn init_product(server: &MockInferenceServer, mode: TelemetryMode) {
    let config = TelemetryConfig {
        events_url: Some(format!("{}/events", server.url())),
        events_api_key: Some("test-key".into()),
        mixpanel_enabled: false,
        mixpanel_token: None,
        ..TelemetryConfig::default()
    };
    a3s_code_telemetry::init(
        config,
        mode,
        None,
        None,
        None,
        None,
        "test".into(),
        None,
        a3s_code_shell::http::shared_client(),
    );
}

fn projected(invocation: &str, exit_code: i32) -> a3s_code_telemetry::events::ToolCallCompleted {
    let parent = tracing::info_span!("tools.execute");
    let span = a3s_code_shell::session::tool_execution_span(
        &parent,
        "session",
        "grep",
        "grep",
        "grep-call",
        12,
        false,
    );
    let output = a3s_code_shell::session::grep_output(exit_code);
    a3s_code_shell::session::complete_projected_call(span, invocation, &output, "success")
}

fn attr<'a>(span: &'a OtelSpan, key: &str) -> Option<&'a str> {
    span.attributes.get(key).and_then(Value::as_str)
}

fn product_row<'a>(rows: &'a [Value], invocation: &str) -> Option<&'a Value> {
    rows.iter().find(|event| {
        event.get("event_name").and_then(Value::as_str) == Some("grok-shell-tool_call_completed")
            && event
                .get("event_metadata")
                .and_then(|metadata| metadata.get("invocation_id"))
                .and_then(Value::as_str)
                == Some(invocation)
    })
}

#[tokio::test]
async fn span_and_product_row_agree_and_product_gate_is_independent() {
    let home = std::env::temp_dir().join(format!("tool-call-trace-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    set_env("GROK_HOME", home.to_str().unwrap());
    let traces = MockOtelServer::start().await.expect("traces");
    let product = MockInferenceServer::start().await.expect("product");
    install_tracing(&format!("{}/v1/traces", traces.origin()));
    init_product(&product, TelemetryMode::SessionMetrics);

    let silent = projected(SILENT, -1);
    a3s_code_telemetry::session_ctx::log_event_now(silent).await;
    traces
        .recorder()
        .wait_for_spans(Duration::from_secs(5), |spans| {
            spans
                .iter()
                .any(|span| attr(span, "invocation_id") == Some(SILENT))
        })
        .await
        .expect("session-metrics export");
    assert_eq!(Vec::<Value>::new(), product.telemetry_events());

    init_product(&product, TelemetryMode::Enabled);
    let event = projected(ENABLED, -1);
    a3s_code_telemetry::session_ctx::log_event_now(event).await;
    a3s_code_telemetry::otel_layer::shutdown_otel();
    let spans = traces
        .recorder()
        .wait_for_spans(Duration::from_secs(5), |spans| {
            spans
                .iter()
                .any(|span| attr(span, "invocation_id") == Some(ENABLED))
        })
        .await
        .expect("enabled export");
    let span = spans
        .iter()
        .find(|span| attr(span, "invocation_id") == Some(ENABLED))
        .expect("matching span");
    assert_eq!(span.name, "tool.execution");
    assert_eq!(attr(span, "session_id"), Some("session"));
    let events = product.telemetry_events();
    let row = product_row(&events, ENABLED).expect("product row");
    let metadata = row.get("event_metadata").expect("metadata");
    assert_eq!(
        metadata.get("model_id").and_then(Value::as_str),
        Some("grok-4.6")
    );
    assert_eq!(
        metadata.get("invocation_id").and_then(Value::as_str),
        Some(ENABLED)
    );
    assert_eq!(
        metadata.get("tool_id").and_then(Value::as_str),
        Some("GrokBuild:grep")
    );
    assert_eq!(
        metadata.get("tool_version").and_then(Value::as_str),
        Some("current")
    );
    assert_eq!(
        metadata.get("source_status").and_then(Value::as_str),
        Some("failed")
    );
    assert_eq!(
        metadata.get("source_reason").and_then(Value::as_str),
        Some("search.unclassified_exit")
    );
    assert_eq!(
        metadata.get("outcome").and_then(Value::as_str),
        Some("error")
    );
    assert_eq!(attr(span, "model_id"), Some("grok-4.6"));
    assert_eq!(attr(span, "invocation_id"), Some(ENABLED));
    assert_eq!(attr(span, "tool_id"), Some("GrokBuild:grep"));
    assert_eq!(attr(span, "tool_version"), Some("current"));
    assert_eq!(attr(span, "source_status"), Some("failed"));
    assert_eq!(
        attr(span, "source_reason"),
        Some("search.unclassified_exit")
    );
    assert_eq!(attr(span, "outcome"), Some("error"));
    assert!(!span.attributes.contains_key("path_scope"));
    let rendered = format!(
        "{metadata} {span:?} {}",
        traces.recorder().body_text().unwrap()
    );
    assert!(!rendered.contains(CANARY_PATH));
    assert!(!rendered.contains("secret-project"));
    assert!(!rendered.contains("CANARY_PATTERN"));
    assert!(!rendered.contains("CANARY_STDOUT"));
    assert!(!rendered.contains("CANARY_STDERR"));
    assert!(!rendered.contains(CANARY_BODY));
}
