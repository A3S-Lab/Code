//! This binary owns the process-global tracer. Trace export off does not silence product posts.

use std::time::Duration;

use a3s_code_telemetry::config::{TelemetryConfig, TelemetryMode};
use a3s_code_test_support::{MockInferenceServer, MockOtelServer};
use serde_json::Value;

const INVOCATION: &str = "018f6b6c-7b3a-7c3a-8c3a-000000000003";
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

#[tokio::test]
async fn disabled_trace_export_stays_silent_while_product_posts() {
    let home = std::env::temp_dir().join(format!("tool-call-trace-off-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    set_env("GROK_HOME", home.to_str().unwrap());
    set_env("GROK_TELEMETRY_ENABLED", "true");
    set_env("GROK_INSTRUMENTATION", "server");
    set_env("OTEL_TRACES_EXPORTER", "none");
    unset_env("DISABLE_TELEMETRY");
    unset_env("GROK_EXTERNAL_OTEL");
    let traces = MockOtelServer::start().await.expect("traces");
    let product = MockInferenceServer::start().await.expect("product");
    set_env(
        "GROK_INTERNAL_OTLP_TRACES_ENDPOINT",
        &format!("{}/v1/traces", traces.origin()),
    );
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
    let telemetry = TelemetryConfig {
        events_url: Some(format!("{}/events", product.url())),
        events_api_key: Some("test-key".into()),
        mixpanel_enabled: false,
        mixpanel_token: None,
        ..TelemetryConfig::default()
    };
    a3s_code_telemetry::init(
        telemetry,
        TelemetryMode::Enabled,
        None,
        None,
        None,
        None,
        "test".into(),
        None,
        a3s_code_shell::http::shared_client(),
    );

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
    let output = a3s_code_shell::session::grep_output(-1);
    let event =
        a3s_code_shell::session::complete_projected_call(span, INVOCATION, &output, "success");
    a3s_code_telemetry::session_ctx::log_event_now(event).await;
    a3s_code_telemetry::otel_layer::shutdown_otel();
    traces
        .recorder()
        .wait_for_span_silence(Duration::from_millis(300))
        .await
        .expect("disabled export stays silent");
    assert_eq!(traces.recorder().spans(), Vec::new());
    let rows = product.telemetry_events();
    let row = rows
        .iter()
        .find(|event| {
            event.get("event_name").and_then(Value::as_str)
                == Some("grok-shell-tool_call_completed")
        })
        .expect("product row");
    let metadata = row.get("event_metadata").expect("metadata");
    assert_eq!(
        metadata.get("model_id").and_then(Value::as_str),
        Some("grok-4.6")
    );
    assert_eq!(
        metadata.get("tool_id").and_then(Value::as_str),
        Some("GrokBuild:grep")
    );
    assert_eq!(
        metadata.get("source_status").and_then(Value::as_str),
        Some("failed")
    );
    assert_eq!(
        metadata.get("source_reason").and_then(Value::as_str),
        Some("search.unclassified_exit")
    );
    let rendered = metadata.to_string();
    assert!(!rendered.contains(CANARY_PATH));
    assert!(!rendered.contains("secret-project"));
    assert!(!rendered.contains(CANARY_BODY));
}
