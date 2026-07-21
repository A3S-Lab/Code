use super::*;
use crate::config::{HeadlessConfig, SearchConfig};
use crate::tools::types::{Tool, ToolOutputKind};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::Notify;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn context(workspace: &TempDir, server: &MockServer) -> ToolContext {
    ToolContext::new(workspace.path().to_path_buf()).with_search_config(SearchConfig {
        timeout: 30,
        health: None,
        engines: HashMap::new(),
        headless: Some(HeadlessConfig {
            proxy_url: Some(server.uri()),
            ..HeadlessConfig::default()
        }),
    })
}

async fn execute(workspace: &TempDir, server: &MockServer, args: serde_json::Value) -> ToolOutput {
    DownloadTool
        .execute(&args, &context(workspace, server))
        .await
        .expect("download tool execution")
}

fn request_range(request: &Request) -> Option<(u64, u64)> {
    let value = request.headers.get("range")?.to_str().ok()?;
    let value = value.strip_prefix("bytes=")?;
    let (start, end) = value.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?))
}

fn range_response(payload: &[u8], start: u64, end: u64) -> ResponseTemplate {
    let total = payload.len() as u64;
    ResponseTemplate::new(206)
        .insert_header("content-range", format!("bytes {start}-{end}/{total}"))
        .insert_header("content-length", (end - start + 1).to_string())
        .insert_header("etag", "\"download-v1\"")
        .set_body_bytes(payload[start as usize..=end as usize].to_vec())
}

fn deterministic_payload(size: usize) -> Vec<u8> {
    (0..size).map(|index| (index % 251) as u8).collect()
}

fn assert_no_download_temps(path: &Path) {
    if !path.exists() {
        return;
    }
    for entry in std::fs::read_dir(path).expect("read test workspace") {
        let entry = entry.expect("workspace entry");
        let file_type = entry.file_type().expect("workspace file type");
        if file_type.is_dir() {
            assert_no_download_temps(&entry.path());
        } else {
            assert!(
                !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".a3s-download-"),
                "temporary download leaked at {}",
                entry.path().display()
            );
        }
    }
}

#[test]
fn definition_is_bounded_and_binary_safe() {
    let parameters = DownloadTool.parameters();
    assert_eq!(parameters["additionalProperties"], false);
    assert_eq!(parameters["required"], json!(["url"]));
    assert_eq!(parameters["properties"]["connections"]["maximum"], 4);
    assert_eq!(
        parameters["properties"]["expected_sha256"]["pattern"],
        "^[A-Fa-f0-9]{64}$"
    );

    let capabilities = DownloadTool.capabilities(&json!({}));
    assert!(!capabilities.read_only);
    assert!(!capabilities.idempotent);
    assert!(capabilities.cancellation_safe);
    assert_eq!(capabilities.max_parallelism, 1);
    assert_eq!(capabilities.output_kind, ToolOutputKind::Mixed);
}

#[tokio::test]
async fn private_literal_urls_are_rejected_before_network_access() {
    let workspace = tempfile::tempdir().unwrap();
    let output = DownloadTool
        .execute(
            &json!({"url": "http://127.0.0.1/private.bin"}),
            &ToolContext::new(workspace.path().to_path_buf()),
        )
        .await
        .unwrap();

    assert!(!output.success);
    assert!(output.content.contains("non-public"));
    assert!(matches!(
        output.error_kind,
        Some(ToolErrorKind::InvalidArgument { .. })
    ));
    assert!(std::fs::read_dir(workspace.path())
        .unwrap()
        .next()
        .is_none());
}

#[tokio::test]
async fn sequential_download_is_binary_exact_and_infers_safe_filename() {
    let server = MockServer::start().await;
    let workspace = tempfile::tempdir().unwrap();
    let payload = vec![0, 1, 2, 0, 255, 128, b'\n'];
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(
                    "content-disposition",
                    "attachment; ignored; filename*=UTF-8''payload%20%E4%B8%AD.bin",
                )
                .insert_header("content-type", "application/octet-stream")
                .set_body_bytes(payload.clone()),
        )
        .mount(&server)
        .await;

    let output = execute(
        &workspace,
        &server,
        json!({"url": "http://example.test/file?signature=top-secret"}),
    )
    .await;

    assert!(output.success, "{}", output.content);
    assert_eq!(
        std::fs::read(workspace.path().join("payload 中.bin")).unwrap(),
        payload
    );
    let metadata = output.metadata.unwrap();
    assert_eq!(metadata["file_path"], "payload 中.bin");
    assert_eq!(metadata["strategy"], "sequential");
    assert_eq!(metadata["bytes"], 7);
    assert_eq!(
        metadata["source_anchors"],
        json!(["http://example.test/file"])
    );
    assert_no_download_temps(workspace.path());
}

#[tokio::test]
async fn parallel_ranges_reconstruct_the_exact_file() {
    let server = MockServer::start().await;
    let workspace = tempfile::tempdir().unwrap();
    let payload = Arc::new(deterministic_payload(7 * 1024 * 1024 + 123));
    let responder_payload = Arc::clone(&payload);
    Mock::given(method("GET"))
        .respond_with(move |request: &Request| {
            let (start, end) = request_range(request).expect("range request");
            range_response(&responder_payload, start, end)
        })
        .mount(&server)
        .await;

    let output = execute(
        &workspace,
        &server,
        json!({
            "url": "http://example.test/release.bin",
            "file_path": "artifacts/release.bin",
            "connections": 3
        }),
    )
    .await;

    assert!(output.success, "{}", output.content);
    assert_eq!(
        std::fs::read(workspace.path().join("artifacts/release.bin")).unwrap(),
        payload.as_slice()
    );
    let metadata = output.metadata.unwrap();
    assert_eq!(metadata["strategy"], "parallel_range");
    assert_eq!(metadata["connections"], 3);
    assert_eq!(metadata["range_supported"], true);

    let requests = server.received_requests().await.unwrap();
    assert!(requests.len() >= 4);
    assert!(requests.iter().all(|request| {
        request
            .headers
            .get("accept-encoding")
            .and_then(|value| value.to_str().ok())
            == Some("identity")
    }));
    assert!(requests
        .iter()
        .filter(|request| request_range(request).is_some_and(|(_, end)| end > 0))
        .all(|request| request.headers.get("if-range").is_some()));
    assert_no_download_temps(workspace.path());
}

#[tokio::test]
async fn range_support_without_a_validator_uses_one_coherent_response() {
    let server = MockServer::start().await;
    let workspace = tempfile::tempdir().unwrap();
    let payload = Arc::new(deterministic_payload(5 * 1024 * 1024 + 9));
    let responder_payload = Arc::clone(&payload);
    Mock::given(method("GET"))
        .respond_with(move |request: &Request| match request_range(request) {
            Some((0, 0)) => {
                let total = responder_payload.len();
                ResponseTemplate::new(206)
                    .insert_header("content-range", format!("bytes 0-0/{total}"))
                    .set_body_bytes(vec![responder_payload[0]])
            }
            Some(_) => panic!("unvalidated parallel ranges must not be requested"),
            None => ResponseTemplate::new(200).set_body_bytes((*responder_payload).clone()),
        })
        .mount(&server)
        .await;

    let output = execute(
        &workspace,
        &server,
        json!({
            "url": "http://example.test/coherent.bin",
            "file_path": "coherent.bin",
            "connections": 4
        }),
    )
    .await;

    assert!(output.success, "{}", output.content);
    assert_eq!(
        std::fs::read(workspace.path().join("coherent.bin")).unwrap(),
        payload.as_slice()
    );
    let metadata = output.metadata.unwrap();
    assert_eq!(metadata["strategy"], "sequential");
    assert_eq!(metadata["connections"], 1);
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn invalid_range_responses_fall_back_to_a_full_download() {
    let server = MockServer::start().await;
    let workspace = tempfile::tempdir().unwrap();
    let payload = Arc::new(deterministic_payload(4 * 1024 * 1024 + 17));
    let responder_payload = Arc::clone(&payload);
    Mock::given(method("GET"))
        .respond_with(move |request: &Request| match request_range(request) {
            Some((0, 0)) => range_response(&responder_payload, 0, 0),
            Some((start, end)) => ResponseTemplate::new(206)
                .insert_header(
                    "content-range",
                    format!("bytes {}-{end}/{}", start + 1, responder_payload.len()),
                )
                .set_body_bytes(responder_payload[start as usize..=end as usize].to_vec()),
            None => ResponseTemplate::new(200).set_body_bytes((*responder_payload).clone()),
        })
        .mount(&server)
        .await;

    let output = execute(
        &workspace,
        &server,
        json!({
            "url": "http://example.test/fallback.bin",
            "file_path": "fallback.bin",
            "connections": 2
        }),
    )
    .await;

    assert!(output.success, "{}", output.content);
    assert_eq!(
        std::fs::read(workspace.path().join("fallback.bin")).unwrap(),
        payload.as_slice()
    );
    assert_eq!(output.metadata.unwrap()["strategy"], "sequential_fallback");
    assert_no_download_temps(workspace.path());
}

#[tokio::test]
async fn max_bytes_rejects_before_creating_a_temporary_file() {
    let server = MockServer::start().await;
    let workspace = tempfile::tempdir().unwrap();
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![42; 32]))
        .mount(&server)
        .await;

    let output = execute(
        &workspace,
        &server,
        json!({
            "url": "http://example.test/too-large.bin",
            "file_path": "too-large.bin",
            "max_bytes": 8
        }),
    )
    .await;

    assert!(!output.success);
    assert!(output.content.contains("exceeds max_bytes"));
    assert!(matches!(
        output.error_kind,
        Some(ToolErrorKind::InvalidArgument { .. })
    ));
    assert!(!workspace.path().join("too-large.bin").exists());
    assert_no_download_temps(workspace.path());
}

#[tokio::test]
async fn checksum_failure_preserves_an_existing_destination() {
    let server = MockServer::start().await;
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("existing.bin"), b"original").unwrap();
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"replacement".to_vec()))
        .mount(&server)
        .await;

    let output = execute(
        &workspace,
        &server,
        json!({
            "url": "http://example.test/existing.bin",
            "file_path": "existing.bin",
            "overwrite": true,
            "expected_sha256": "0000000000000000000000000000000000000000000000000000000000000000"
        }),
    )
    .await;

    assert!(!output.success);
    assert!(output.content.contains("SHA-256 mismatch"));
    assert_eq!(
        std::fs::read(workspace.path().join("existing.bin")).unwrap(),
        b"original"
    );
    assert_no_download_temps(workspace.path());
}

#[tokio::test]
async fn verified_overwrite_replaces_the_destination_after_validation() {
    let server = MockServer::start().await;
    let workspace = tempfile::tempdir().unwrap();
    let payload = b"verified replacement".to_vec();
    let expected = format!(
        "{:x}",
        <sha2::Sha256 as sha2::Digest>::digest(payload.as_slice())
    );
    std::fs::write(workspace.path().join("existing.bin"), b"original").unwrap();
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(payload.clone()))
        .mount(&server)
        .await;

    let output = execute(
        &workspace,
        &server,
        json!({
            "url": "http://example.test/existing.bin",
            "file_path": "existing.bin",
            "overwrite": true,
            "expected_sha256": expected
        }),
    )
    .await;

    assert!(output.success, "{}", output.content);
    assert_eq!(
        std::fs::read(workspace.path().join("existing.bin")).unwrap(),
        payload
    );
    let metadata = output.metadata.unwrap();
    assert_eq!(metadata["overwritten"], true);
    assert_eq!(metadata["sha256"], expected);
    assert_no_download_temps(workspace.path());
}

#[tokio::test]
async fn cancellation_removes_the_partial_file() {
    let server = MockServer::start().await;
    let workspace = tempfile::tempdir().unwrap();
    let started = Arc::new(Notify::new());
    let responder_started = Arc::clone(&started);
    let payload = Arc::new(vec![7_u8; 1024]);
    let responder_payload = Arc::clone(&payload);
    Mock::given(method("GET"))
        .respond_with(move |request: &Request| match request_range(request) {
            Some((0, 0)) => range_response(&responder_payload, 0, 0),
            Some(_) => unreachable!("one connection must use a full request"),
            None => {
                responder_started.notify_one();
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(5))
                    .set_body_bytes((*responder_payload).clone())
            }
        })
        .mount(&server)
        .await;

    let cancellation = CancellationToken::new();
    let ctx = context(&workspace, &server).with_cancellation(cancellation.clone());
    let task = tokio::spawn(async move {
        DownloadTool
            .execute(
                &json!({
                    "url": "http://example.test/cancel.bin",
                    "file_path": "cancel.bin",
                    "connections": 1
                }),
                &ctx,
            )
            .await
            .unwrap()
    });
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .expect("full request started");
    cancellation.cancel();
    let output = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("download cancelled promptly")
        .unwrap();

    assert!(!output.success);
    assert!(matches!(
        output.error_kind,
        Some(ToolErrorKind::Cancelled { .. })
    ));
    assert!(!workspace.path().join("cancel.bin").exists());
    assert_no_download_temps(workspace.path());
}
