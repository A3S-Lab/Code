//! I-OT-01: a refused OTLP collector must not fail a host tool turn.
//!
//! The batch exporter may log a later export error. That error stays outside
//! the tool result and the in-memory trace.

#![cfg(feature = "telemetry")]

use std::time::{Duration, Instant};

use a3s_code_core::config::CodeConfig;
use a3s_code_core::telemetry_otel::TelemetryConfig;
use a3s_code_core::trace::TraceEventKind;
use a3s_code_core::Agent;

fn test_config() -> CodeConfig {
    CodeConfig::from_acl(
        r#"
default_model = "anthropic/claude-sonnet-4-20250514"
providers "anthropic" {
  api_key = "test-key"
  models "claude-sonnet-4-20250514" { name = "Claude Sonnet 4" }
}
"#,
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "I-OT-01: refused collector must not fail a tool turn"]
async fn collector_down_does_not_fail_the_tool_turn() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let started = Instant::now();
    let endpoint = format!("http://127.0.0.1:{port}");
    let guard = TelemetryConfig::new(endpoint)
        .init()
        .expect("a refused collector must not fail telemetry init");

    let agent = Agent::from_config(test_config()).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("note.txt"), "TRACE-TOOL-NOTE").unwrap();
    let session = agent
        .session_async(workspace.path().display().to_string(), None)
        .await
        .unwrap();
    let result = session
        .tool("read", serde_json::json!({ "file_path": "note.txt" }))
        .await
        .expect("tool turn must succeed while the collector is down");

    assert_eq!(result.exit_code, 0, "{}", result.output);
    assert!(result.output.contains("TRACE-TOOL-NOTE"));
    assert!(
        session
            .trace_events()
            .iter()
            .any(|event| { event.kind == TraceEventKind::ToolExecution && event.name == "read" }),
        "in-memory trace dropped the tool: {:?}",
        session.trace_events()
    );
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "telemetry retry blocked the turn for {:?}",
        started.elapsed()
    );
    drop(guard);
}
