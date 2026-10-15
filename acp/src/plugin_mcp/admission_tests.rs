#![cfg(test)]

use super::*;
use a3s_use_reconcile::SurfaceKind;
use serde_json::json;

fn surface(value: Value) -> PublishedSurface {
    PublishedSurface {
        kind: SurfaceKind::Mcp,
        id: "fixture-server".into(),
        sha256: "1".repeat(64),
        content_sha256: "2".repeat(64),
        directory: std::path::PathBuf::from("/unused-admission-fixture"),
        entry: None,
        mcp: Some(value),
    }
}

#[test]
fn boxed_http_admission_preserves_config_without_opening_a_transport() {
    let surface = surface(
        json!({"type":"http","url":"https://fixture.invalid/mcp","headers":{"x-fixture":"kept"},"env":{"FIXTURE":"kept"},"enabled":false,"tool_timeout_secs":17}),
    );
    let McpAdmission::Ready(config) = admit(&surface).expect("admission") else {
        panic!("ready config")
    };
    assert_eq!(config.name, "fixture-server");
    assert!(!config.enabled);
    assert_eq!(config.tool_timeout_secs, 17);
    assert_eq!(config.env.get("FIXTURE").map(String::as_str), Some("kept"));
    match config.transport {
        McpTransportConfig::StreamableHttp { url, headers } => {
            assert_eq!(url, "https://fixture.invalid/mcp");
            assert_eq!(headers.get("x-fixture").map(String::as_str), Some("kept"));
        }
        _ => panic!("HTTP transport changed"),
    }
}

#[test]
fn external_stdio_and_invalid_timeout_still_refuse_admission() {
    assert!(matches!(
        admit(&surface(json!({"type":"stdio","command":"external"}))).expect("external runtime"),
        McpAdmission::ExternalRuntime
    ));
    assert!(admit(&surface(
        json!({"type":"http","url":"https://fixture.invalid/mcp","tool_timeout_secs":0})
    ))
    .is_err());
}
