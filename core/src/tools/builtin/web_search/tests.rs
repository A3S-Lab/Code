use super::*;
use std::collections::HashMap;
use std::path::PathBuf;

#[tokio::test]
async fn headless_browser_pool_is_scoped_to_one_tool_execution() {
    let config = HeadlessConfig::default();
    let first = WebSearchTool::create_pool(Some(&config)).expect("first pool");
    let second = WebSearchTool::create_pool(Some(&config)).expect("second pool");

    assert!(
        !Arc::ptr_eq(&first, &second),
        "parallel or cancelled tool calls must not retain one shared Chrome lifecycle"
    );
    first.shutdown().await;
    second.shutdown().await;
}

#[tokio::test]
async fn dropped_cleanup_guard_closes_pool_before_background_shutdown() {
    let config = HeadlessConfig::default();
    let pool = WebSearchTool::create_pool(Some(&config)).expect("pool");
    drop(BrowserPoolCleanup::new(Some(Arc::clone(&pool))));

    assert!(
        pool.tab_semaphore().is_closed(),
        "dropping a cancelled tool future must synchronously reject new tab work"
    );
    tokio::task::yield_now().await;
}

#[test]
fn latest_search_metrics_are_exposed_as_stable_metadata() {
    let snapshot = MetricsSnapshot {
        successes: 3,
        failures: 1,
        transient_failures: 1,
        permanent_failures: 0,
        error_counts: HashMap::from([("timeout".to_string(), 1)]),
        latency_p50_ms: 10,
        latency_p95_ms: 20,
        latency_p99_ms: 30,
    };
    let metadata = search_metrics_json(&snapshot);
    assert_eq!(metadata["total_requests"], 4);
    assert_eq!(metadata["success_rate"], 75.0);
    assert_eq!(metadata["transient_failure_rate"], 100.0);
    assert_eq!(metadata["error_counts"]["timeout"], 1);
    assert_eq!(metadata["latency_p99_ms"], 30);
}

#[tokio::test]
async fn test_web_search_missing_query() {
    let tool = WebSearchTool::new();
    let ctx = ToolContext::new(PathBuf::from("/tmp"));

    let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();
    assert!(!result.success);
}

#[tokio::test]
async fn test_web_search_empty_query() {
    let tool = WebSearchTool::new();
    let ctx = ToolContext::new(PathBuf::from("/tmp"));

    let result = tool
        .execute(&serde_json::json!({"query": ""}), &ctx)
        .await
        .unwrap();
    assert!(!result.success);
}

#[tokio::test]
async fn test_web_search_no_valid_engines() {
    let tool = WebSearchTool::new();
    let ctx = ToolContext::new(PathBuf::from("/tmp"));

    let result = tool
        .execute(
            &serde_json::json!({"query": "test", "engines": ["nonexistent"]}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(!result.success);
    assert!(result.content.contains("No valid engines"));
}

#[tokio::test]
#[ignore = "requires external network"]
async fn real_system_proxy_search_returns_traceable_results() {
    let tool = WebSearchTool::new();
    let ctx = ToolContext::new(PathBuf::from("/tmp"));
    let result = tool
        .execute(
            &serde_json::json!({
                "query": "Tokio Rust async runtime official documentation",
                "engines": ["ddg", "brave", "wiki"],
                "limit": 5,
                "timeout": 15,
                "format": "json"
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert!(result.success, "{}", result.content);
    eprintln!("{}", result.content);
    let items: serde_json::Value = serde_json::from_str(&result.content)
        .unwrap_or_else(|error| panic!("JSON search results ({error}): {}", result.content));
    assert!(
        items.as_array().is_some_and(|items| !items.is_empty()),
        "{}",
        result.content
    );
}

#[tokio::test]
#[ignore = "requires external network"]
async fn real_bing_rss_search_works_without_headless_config() {
    let tool = WebSearchTool::new();
    let ctx = ToolContext::new(PathBuf::from("/tmp"));
    let started = std::time::Instant::now();

    let result = tool
        .execute(
            &serde_json::json!({
                "query": "Typhoon Bavi 2020 NOAA -2026",
                "engines": ["bing_cn"],
                "limit": 5,
                "timeout": 10,
                "format": "json"
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(result.success, "{}", result.content);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(12),
        "Bing RSS exceeded its convergence budget: {:?}",
        started.elapsed()
    );
    let items: serde_json::Value = serde_json::from_str(&result.content)
        .unwrap_or_else(|error| panic!("JSON Bing results ({error}): {}", result.content));
    assert!(
        items.as_array().is_some_and(|items| !items.is_empty()),
        "{}",
        result.content
    );
}

#[test]
fn bing_china_is_http_only() {
    assert!(!requires_headless_browser(&["bing_cn"]));
    assert!(!requires_headless_browser(&["ddg", "bing_cn"]));
    assert!(requires_headless_browser(&["google"]));
    assert!(requires_headless_browser(&["bing_cn", "baidu"]));
}

#[test]
fn unavailable_headless_engines_fall_back_only_when_safe() {
    assert!(should_fallback_from_unavailable_headless(
        0,
        false,
        &["baidu"],
    ));
    assert!(should_fallback_from_unavailable_headless(
        0,
        false,
        &["google"],
    ));
    assert!(!should_fallback_from_unavailable_headless(
        0,
        false,
        &["baidu", "bing_cn"],
    ));
    assert!(!should_fallback_from_unavailable_headless(
        0,
        true,
        &["baidu"],
    ));
    assert!(!should_fallback_from_unavailable_headless(
        1,
        false,
        &["baidu"],
    ));
    assert!(!should_fallback_from_unavailable_headless(
        0,
        false,
        &["baidu", "nonexistent"],
    ));
}

#[tokio::test]
async fn test_web_search_unknown_parameter_engine_returns_error() {
    let tool = WebSearchTool::new();
    let ctx = ToolContext::new(PathBuf::from("/tmp"));

    // Using `engine` (singular) instead of `engines` (plural) should return an error
    let result = tool
        .execute(
            &serde_json::json!({"query": "test", "engine": "google"}),
            &ctx,
        )
        .await
        .unwrap();

    assert!(
        !result.success,
        "Expected error when using 'engine' instead of 'engines'"
    );
    assert!(
        result.content.contains("unknown parameter 'engine'"),
        "Error message should mention the unknown parameter"
    );
    assert!(
        result.content.contains("'engines' (plural)"),
        "Error message should clarify to use 'engines' (plural)"
    );
}

#[tokio::test]
async fn test_web_search_multiple_unknown_parameters() {
    let tool = WebSearchTool::new();
    let ctx = ToolContext::new(PathBuf::from("/tmp"));

    let result = tool
        .execute(
            &serde_json::json!({
                "query": "test",
                "engine": "ddg",
                "source": "web"
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(!result.success);
    assert!(
        result.content.contains("unknown parameter"),
        "Error should mention unknown parameters"
    );
}

#[tokio::test]
async fn test_web_search_engines_param_works() {
    let tool = WebSearchTool::new();
    let ctx = ToolContext::new(PathBuf::from("/tmp"));

    let result = tool
        .execute(
            &serde_json::json!({"query": "test", "engines": ["ddg"]}),
            &ctx,
        )
        .await
        .unwrap();

    // May succeed or fail depending on network, but should NOT have unknown param error
    if !result.success {
        assert!(
            !result.content.contains("unknown parameter"),
            "Should not complain about 'engines' being unknown"
        );
    }
}

#[test]
fn test_web_search_schema_is_canonical() {
    let tool = WebSearchTool::new();
    let params = tool.parameters();
    assert_eq!(params["additionalProperties"], false);
    assert_eq!(params["required"], serde_json::json!(["query"]));
    // engines should be an array type
    assert_eq!(params["properties"]["engines"]["type"], "array");
    let examples = params["examples"].as_array().unwrap();
    assert_eq!(examples[0]["query"], "Rust async trait");
    assert!(examples[0].get("q").is_none());
    // Example with engines should use array format
    assert!(examples[1]["engines"].is_array());
    assert_eq!(examples[1]["engines"].as_array().unwrap(), &["ddg", "wiki"]);
}

#[test]
fn test_parse_proxy_url_http() {
    let config = parse_proxy_url("http://127.0.0.1:8080").unwrap();
    assert_eq!(config.host, "127.0.0.1");
    assert_eq!(config.port, 8080);
}

#[test]
fn test_parse_proxy_url_socks5() {
    let config = parse_proxy_url("socks5://proxy.example.com:1080").unwrap();
    assert_eq!(config.host, "proxy.example.com");
    assert_eq!(config.port, 1080);
}

#[test]
fn test_parse_proxy_url_no_port() {
    assert!(parse_proxy_url("http://127.0.0.1").is_none());
}

#[test]
fn test_parse_proxy_url_empty() {
    assert!(parse_proxy_url("").is_none());
}

#[test]
fn test_add_http_engine_valid() {
    let mut search = Search::new();
    assert!(add_http_engine(&mut search, "ddg", None));
    assert_eq!(search.engine_count(), 1);

    assert!(add_http_engine(&mut search, "wiki", None));
    assert_eq!(search.engine_count(), 2);

    assert!(add_http_engine(&mut search, "brave", None));
    assert_eq!(search.engine_count(), 3);

    assert!(add_http_engine(&mut search, "bing_cn", None));
    assert_eq!(search.engine_count(), 4);
}

#[test]
fn test_add_http_engine_unknown() {
    let mut search = Search::new();
    assert!(!add_http_engine(&mut search, "nonexistent", None));
    assert_eq!(search.engine_count(), 0);
}

#[test]
fn test_add_headless_engine_valid() {
    let mut search = Search::new();
    let pool_config = BrowserPoolConfig::default();
    let pool = Arc::new(BrowserPool::new(pool_config));

    assert!(add_headless_engine(&mut search, "google", &pool));
    assert_eq!(search.engine_count(), 1);

    assert!(add_headless_engine(&mut search, "baidu", &pool));
    assert_eq!(search.engine_count(), 2);

    assert!(!add_headless_engine(&mut search, "bing_cn", &pool));
    assert_eq!(search.engine_count(), 2);
}

#[test]
fn test_add_headless_engine_aliases() {
    let mut search = Search::new();
    let pool_config = BrowserPoolConfig::default();
    let pool = Arc::new(BrowserPool::new(pool_config));

    assert!(add_headless_engine(&mut search, "g", &pool));
    assert_eq!(search.engine_count(), 1);
}

#[test]
fn test_add_headless_engine_unknown() {
    let mut search = Search::new();
    let pool_config = BrowserPoolConfig::default();
    let pool = Arc::new(BrowserPool::new(pool_config));

    assert!(!add_headless_engine(&mut search, "ddg", &pool));
    assert!(!add_headless_engine(&mut search, "nonexistent", &pool));
}

#[tokio::test]
async fn test_web_search_all_valid_parameters_accepted() {
    let tool = WebSearchTool::new();
    let ctx = ToolContext::new(PathBuf::from("/tmp"));

    // All valid parameters should be accepted without unknown param error
    let result = tool
        .execute(
            &serde_json::json!({
                "query": "test",
                "engines": ["ddg", "wiki"],
                "limit": 5,
                "timeout": 30,
                "proxy": "http://127.0.0.1:8080",
                "format": "json"
            }),
            &ctx,
        )
        .await
        .unwrap();

    // Should not have unknown parameter error
    // May fail for other reasons (e.g., network), but not param validation
    if !result.success {
        assert!(
            !result.content.contains("unknown parameter"),
            "All listed parameters should be valid: {}",
            result.content
        );
    }
}

#[test]
fn test_web_search_schema_has_all_valid_fields() {
    let tool = WebSearchTool::new();
    let params = tool.parameters();

    // Verify all valid fields are documented
    let valid_fields = ["query", "engines", "limit", "timeout", "proxy", "format"];
    for field in valid_fields {
        assert!(
            params["properties"]
                .as_object()
                .unwrap()
                .contains_key(field),
            "Schema should document '{}' as a valid field",
            field
        );
    }

    // Verify additionalProperties is false (no extra fields allowed)
    assert_eq!(params["additionalProperties"], false);
}

#[test]
fn json_search_result_preserves_published_date() {
    let result = SearchResult::new(
            "https://result-user:result-password@example.com/release?tracking=secret#fragment",
            "Release notes https://title-user:title-password@example.com/title?title_token=secret#title-fragment",
            "Current release evidence at https://content-user:content-password@example.com/evidence?content_token=secret#content-fragment.",
        )
        .with_engine("ddg", 2)
        .with_engine("brave", 1)
        .with_published_date("2026-07-11");

    let json = search_result_json(&result);

    assert_eq!(json["published_date"], "2026-07-11");
    assert_eq!(json["url"], "https://example.com/release");
    assert_eq!(json["title"], "Release notes https://example.com/title");
    assert_eq!(
        json["content"],
        "Current release evidence at https://example.com/evidence."
    );
    assert_eq!(json["engines"], serde_json::json!(["brave", "ddg"]));
    let serialized = json.to_string();
    for secret in [
        "result-user",
        "result-password",
        "tracking",
        "fragment",
        "title-user",
        "title-password",
        "title_token",
        "content-user",
        "content-password",
        "content_token",
    ] {
        assert!(
            !serialized.contains(secret),
            "leaked {secret}: {serialized}"
        );
    }
}

#[test]
fn text_search_result_preserves_optional_date_and_stable_engines() {
    let dated = SearchResult::new(
            "https://result-user:result-password@example.com/release?tracking=secret#fragment",
            "Release notes https://title-user:title-password@example.com/title?title_token=secret#title-fragment",
            "Current release evidence at https://content-user:content-password@example.com/evidence?content_token=secret#content-fragment.",
        )
        .with_engine("ddg", 2)
        .with_engine("brave", 1)
        .with_published_date(" 2026-07-11 ");
    let text = text_search_result(0, &dated);
    assert!(text.contains("Published: 2026-07-11\n"), "{text}");
    assert!(text.contains("(via brave, ddg)"), "{text}");
    assert!(
        text.contains("URL: https://example.com/release\n"),
        "{text}"
    );
    assert!(
        text.contains("Release notes https://example.com/title"),
        "{text}"
    );
    assert!(
        text.contains("Current release evidence at https://example.com/evidence."),
        "{text}"
    );
    for secret in [
        "result-user",
        "result-password",
        "tracking",
        "fragment",
        "title-user",
        "title-password",
        "title_token",
        "content-user",
        "content-password",
        "content_token",
    ] {
        assert!(!text.contains(secret), "leaked {secret}: {text}");
    }

    let undated = SearchResult::new(
        "https://example.com/reference",
        "Reference",
        "Undated evidence",
    );
    let text = text_search_result(1, &undated);
    assert!(text.starts_with("2. Reference\n"), "{text}");
    assert!(!text.contains("Published:"), "{text}");

    let unsafe_result = SearchResult::new("javascript:alert(1)", "Unsafe", "Unsafe");
    assert!(safe_search_result_url(&unsafe_result).is_empty());
}

#[test]
fn search_query_urls_drop_credentials_query_and_fragment() {
    let query = sanitize_http_urls(
        "compare https://query-user:query-password@example.com/release?api_key=secret#private, now",
    );

    assert_eq!(query, "compare https://example.com/release, now");
    for secret in [
        "query-user",
        "query-password",
        "api_key",
        "secret",
        "private",
    ] {
        assert!(!query.contains(secret), "leaked {secret}: {query}");
    }
}
