//! Integration tests for web_search tool with headless engines
//!
//! Run basic tests: cargo test -p a3s-code-core --features headless-search --test test_web_search_headless
//! Run with actual browser: cargo test -p a3s-code-core --features headless-search --test test_web_search_headless -- --ignored

#![cfg(feature = "headless-search")]

use a3s_code_core::config::{BrowserBackend, HeadlessConfig, SearchConfig};
use a3s_code_core::tools::{ToolContext, ToolExecutor};

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;

/// Helper to create a ToolContext with headless search config
fn make_context(headless: Option<HeadlessConfig>) -> ToolContext {
    let search_config = headless.map(|h| {
        Arc::new(SearchConfig {
            timeout: 30,
            health: None,
            engines: HashMap::new(),
            headless: Some(h),
        })
    });

    let mut context = ToolContext::new(PathBuf::from("/tmp")).with_session_id("test-session");
    context.search_config = search_config;
    context
}

#[tokio::test]
async fn test_web_search_tool_creation() {
    let executor = ToolExecutor::new("/tmp".to_string());
    let definitions = executor.definitions();

    assert!(
        definitions.iter().any(|t| t.name == "web_search"),
        "web_search tool should be registered"
    );
}

#[tokio::test]
async fn test_web_search_http_engine() {
    let executor = ToolExecutor::new("/tmp".to_string());

    // HTTP engine (duckduckgo) doesn't need headless config
    let args = serde_json::json!({
        "query": "test",
        "engines": "duckduckgo"
    });

    let result = executor.execute("web_search", &args).await;

    match result {
        Ok(output) => {
            println!("✅ DuckDuckGo search succeeded!");
            println!("Output length: {}", output.output.len());
            println!("Exit code: {}", output.exit_code);
        }
        Err(e) => {
            println!("⚠️  DuckDuckGo search failed: {:?}", e);
        }
    }
}

#[tokio::test]
async fn test_web_search_with_baidu_headless_engine() {
    let executor = ToolExecutor::new("/tmp".to_string());

    let headless_config = HeadlessConfig {
        backend: BrowserBackend::Chrome,
        max_tabs: 2,
        browser_path: None,
        launch_args: Vec::new(),
        proxy_url: None,
        ..HeadlessConfig::default()
    };

    let context = make_context(Some(headless_config));

    let args = serde_json::json!({
        "query": "test query",
        "engines": "baidu",
        "timeout": 3
    });

    // This will attempt to use baidu with headless browser
    let result = executor
        .execute_with_context("web_search", &args, &context)
        .await;

    match result {
        Ok(output) => {
            println!("✅ Baidu headless search succeeded!");
            println!("Output length: {}", output.output.len());
            println!("Exit code: {:?}", output.exit_code);
        }
        Err(e) => {
            // If a browser is not available, this will fail - which is expected in CI
            println!(
                "⚠️  Baidu headless search failed (browser may not be available): {:?}",
                e
            );
        }
    }
}

#[tokio::test]
async fn test_moli_backend_fails_closed_without_download_or_executable() {
    let executor = ToolExecutor::new("/tmp".to_string());
    let context = make_context(Some(HeadlessConfig {
        backend: BrowserBackend::Moli,
        browser_path: Some("/definitely/missing/a3s-code-moli".to_string()),
        auto_download_moli: false,
        ..HeadlessConfig::default()
    }));

    let output = executor
        .execute_with_context(
            "web_search",
            &serde_json::json!({
                "query": "moli deterministic unavailable fixture",
                "engines": ["google"],
                "timeout": 2,
                "format": "json"
            }),
            &context,
        )
        .await
        .expect("tool dispatch should return a typed result");

    assert_ne!(output.exit_code, 0);
    let metadata = output.metadata.expect("failure metadata");
    assert_eq!(metadata["status"], "failed");
    assert!(metadata["engine_failures"]
        .as_array()
        .expect("engine failure list")
        .iter()
        .any(|failure| failure["kind"] == "headless_unavailable"));
}

#[tokio::test]
#[ignore = "requires the release-bundled Moli executable and external search access"]
async fn test_moli_headless_search_actual() {
    let executor = ToolExecutor::new("/tmp".to_string());
    let context = make_context(Some(HeadlessConfig {
        backend: BrowserBackend::Moli,
        max_tabs: 2,
        moli_download_timeout_secs: 180,
        ..HeadlessConfig::default()
    }));

    let output = executor
        .execute_with_context(
            "web_search",
            &serde_json::json!({
                "query": "Rust programming language",
                // Google may deliberately return an anti-automation challenge
                // for shared CI/developer egress IPs. Bing's browser-rendered
                // result page gives us a real Moli process boundary and a
                // deterministic parser contract without weakening the test to
                // accept an empty result set.
                "engines": ["bing_browser"],
                // A single-result contract keeps this test focused on the
                // Moli process/render/parser path. Multi-source quorum and
                // tier fallback are covered by the deterministic suites.
                "limit": 1,
                "timeout": 20,
                "format": "json"
            }),
            &context,
        )
        .await
        .expect("Moli search should dispatch");

    assert_eq!(output.exit_code, 0, "{}", output.output);
    let payload: serde_json::Value =
        serde_json::from_str(&output.output).expect("Moli search should return JSON");
    let results = payload
        .as_array()
        .or_else(|| payload["results"].as_array())
        .expect("Moli search JSON should contain a results array");
    assert!(!results.is_empty(), "{}", output.output);
    assert!(
        results.iter().any(|result| {
            result["url"]
                .as_str()
                .is_some_and(|url| url.starts_with("http"))
        }),
        "{}",
        output.output
    );
}

#[tokio::test]
#[ignore] // Requires a browser to be installed
async fn test_baidu_headless_search_actual() {
    let executor = ToolExecutor::new("/tmp".to_string());

    let headless_config = HeadlessConfig {
        backend: BrowserBackend::Chrome,
        max_tabs: 2,
        browser_path: None,
        launch_args: Vec::new(),
        proxy_url: None,
        ..HeadlessConfig::default()
    };

    let context = make_context(Some(headless_config));

    let args = serde_json::json!({
        "query": "rust programming language",
        "engines": "baidu"
    });

    let result = executor
        .execute_with_context("web_search", &args, &context)
        .await
        .unwrap();

    assert_eq!(
        result.exit_code, 0,
        "Search should succeed, got exit_code={}, output={}",
        result.exit_code, result.output
    );
    assert!(
        !result.output.trim().is_empty(),
        "Search output should not be empty"
    );
    println!("Search output: {}", result.output);
}

#[tokio::test]
#[ignore] // Requires a browser to be installed
async fn test_google_headless_search_actual() {
    let executor = ToolExecutor::new("/tmp".to_string());

    let headless_config = HeadlessConfig {
        backend: BrowserBackend::Chrome,
        max_tabs: 2,
        browser_path: None,
        launch_args: Vec::new(),
        proxy_url: None,
        ..HeadlessConfig::default()
    };

    let context = make_context(Some(headless_config));

    let args = serde_json::json!({
        "query": "rust programming language",
        "engines": "google"
    });

    let result = executor
        .execute_with_context("web_search", &args, &context)
        .await
        .unwrap();

    assert_eq!(
        result.exit_code, 0,
        "Search should succeed, got exit_code={}, output={}",
        result.exit_code, result.output
    );
    assert!(
        !result.output.trim().is_empty(),
        "Search output should not be empty"
    );
    println!("Search output: {}", result.output);
}

#[tokio::test]
#[ignore = "requires the controlled local HTTPS fixture and Chrome"]
async fn test_google_headless_search_controlled_local_cdp() {
    let browser_path = env::var("A3S_HEADLESS_TEST_BROWSER")
        .expect("A3S_HEADLESS_TEST_BROWSER must identify the workflow-managed Chrome binary");
    assert_eq!(
        env::var("A3S_HEADLESS_TEST_FIXTURE")
            .expect("A3S_HEADLESS_TEST_FIXTURE must select the controlled fixture"),
        "controlled-local-https-v1",
        "the ignored qualification must never run against the public Google endpoint"
    );

    let marker = "controlled-cdp-v1";
    let headless_config = HeadlessConfig {
        backend: BrowserBackend::Chrome,
        max_tabs: 1,
        browser_path: Some(browser_path),
        launch_args: vec![
            "--ignore-certificate-errors".to_string(),
            "--disable-background-networking".to_string(),
            "--disable-quic".to_string(),
            "--no-proxy-server".to_string(),
            "--host-resolver-rules=MAP www.google.com 127.0.0.1".to_string(),
            format!("--a3s-headless-fixture={marker}"),
        ],
        proxy_url: None,
        ..HeadlessConfig::default()
    };
    let context = make_context(Some(headless_config));
    let executor = ToolExecutor::new("/tmp".to_string());
    let output = executor
        .execute_with_context(
            "web_search",
            &serde_json::json!({
                "query": "a3s controlled cdp fixture",
                "engines": ["google"],
                "limit": 1,
                "timeout": 15,
                "format": "json"
            }),
            &context,
        )
        .await
        .expect("controlled headless search must dispatch");

    assert_eq!(output.exit_code, 0, "{}", output.output);
    assert!(
        output.output.contains("A3S Controlled CDP Fixture"),
        "the production Google parser must return the controlled result: {}",
        output.output
    );
    assert!(
        output
            .output
            .contains("https://docs.a3s.dev/controlled-cdp-fixture"),
        "the controlled result must retain its independently asserted source URL: {}",
        output.output
    );
}
