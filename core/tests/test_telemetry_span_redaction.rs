//! U-OT-02: an exported OTLP span must not carry tool-argument secrets.
//!
//! The collector is a loopback HTTP server. `TelemetryConfig::with_otlp_http`
//! posts the same `ExportTraceServiceRequest` protobuf the gRPC exporter
//! builds. `TraceEvent` has no trace id, so the in-memory trace is matched
//! on the tool name that the span also carries.

#![cfg(feature = "telemetry")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

use a3s_code_core::config::CodeConfig;
use a3s_code_core::telemetry_otel::TelemetryConfig;
use a3s_code_core::trace::TraceEventKind;
use a3s_code_core::Agent;

const SECRET: &str = "OTLP-SECRET-91";

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

fn read_http_body(stream: &mut std::net::TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("read timeout");
    let mut raw = Vec::new();
    let mut buf = [0_u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if http_body_complete(&raw) {
                    break;
                }
            }
        }
    }
    http_body(&raw)
}

fn http_body_complete(raw: &[u8]) -> bool {
    let Some(split) = raw.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let header = String::from_utf8_lossy(&raw[..split]);
    let length = header.lines().find_map(|line| {
        let rest = line
            .strip_prefix("Content-Length:")
            .or_else(|| line.strip_prefix("content-length:"))?;
        rest.trim().parse::<usize>().ok()
    });
    match length {
        Some(length) => raw.len() >= split + 4 + length,
        None => true,
    }
}

fn http_body(raw: &[u8]) -> Vec<u8> {
    raw.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|split| raw[split + 4..].to_vec())
        .unwrap_or_default()
}

#[tokio::test]
async fn exported_span_omits_tool_argument_secrets() {
    let _env = EnvGuard::clear(&[
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "OTEL_EXPORTER_OTLP_PROTOCOL",
        "RUST_LOG",
    ]);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hits_thread = std::sync::Arc::clone(&hits);
    std::thread::spawn(move || {
        for _ in 0..4 {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            hits_thread.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let body = read_http_body(&mut stream);
            let _ = tx.send(body);
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    });

    let guard = TelemetryConfig::new(format!("http://127.0.0.1:{port}"))
        .with_otlp_http()
        .with_export_delay(Duration::from_millis(50))
        .init()
        .expect("hermetic OTLP init");

    let agent = Agent::from_config(test_config()).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let secret_path = format!("{SECRET}.txt");
    std::fs::write(workspace.path().join(&secret_path), "VISIBLE-NOTE-91").unwrap();
    let session = agent
        .session_async(workspace.path().display().to_string(), None)
        .await
        .unwrap();
    let result = session
        .tool("read", serde_json::json!({ "file_path": secret_path }))
        .await
        .expect("tool turn");
    assert_eq!(result.exit_code, 0, "{}", result.output);
    assert!(
        session
            .trace_events()
            .iter()
            .any(|event| { event.kind == TraceEventKind::ToolExecution && event.name == "read" }),
        "in-memory trace dropped the tool: {:?}",
        session.trace_events()
    );

    let payload = tokio::task::spawn_blocking(move || {
        let mut payload = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(body) => payload.extend(body),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if !payload.is_empty() {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        payload
    })
    .await
    .expect("collector wait");
    drop(guard);

    let exported = String::from_utf8_lossy(&payload);
    assert!(
        exported.contains("a3s.tool.execute"),
        "collector did not receive the tool span (connections={}): {exported}",
        hits.load(std::sync::atomic::Ordering::Relaxed)
    );
    assert!(
        exported.contains("read"),
        "exported span dropped the tool name recorded in session.trace_events"
    );
    assert!(
        !exported.contains(SECRET),
        "tool argument secret leaked into the OTLP payload"
    );
}

struct EnvGuard {
    saved: Vec<(&'static str, Option<String>)>,
}

impl EnvGuard {
    fn clear(keys: &[&'static str]) -> Self {
        let saved = keys
            .iter()
            .map(|key| (*key, std::env::var(key).ok()))
            .collect::<Vec<_>>();
        for key in keys {
            std::env::remove_var(key);
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}
