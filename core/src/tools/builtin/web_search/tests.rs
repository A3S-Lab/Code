use super::*;
use crate::config::{SearchConfig, SearchEngineConfig};
use std::collections::HashMap;
use std::path::PathBuf;

#[cfg(feature = "headless-search")]
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

#[cfg(feature = "headless-search")]
#[tokio::test]
async fn dropped_cleanup_guard_schedules_background_shutdown() {
    let config = HeadlessConfig::default();
    let pool = WebSearchTool::create_pool(Some(&config)).expect("pool");
    drop(BrowserPoolCleanup::new(Some(Arc::clone(&pool))));

    tokio::task::yield_now().await;
    let error = pool.warm_up().await.unwrap_err();
    assert!(
        error.message.contains("shut down"),
        "dropping a cancelled tool future must schedule pool shutdown: {error:?}"
    );
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

fn search_config(engines: HashMap<String, SearchEngineConfig>) -> SearchConfig {
    SearchConfig {
        timeout: 10,
        health: None,
        engines,
        headless: None,
    }
}

#[test]
fn default_engine_selection_uses_builtin_defaults_without_engine_configuration() {
    let (engines, source) = default_engine_selection(None);
    assert_eq!(engines, ["anysearch", "tavily", "ddg", "wiki"]);
    assert_eq!(source, "builtin_default");

    let config = search_config(HashMap::new());
    let (engines, source) = default_engine_selection(Some(&config));
    assert_eq!(engines, ["anysearch", "tavily", "ddg", "wiki"]);
    assert_eq!(source, "builtin_default");
}

#[test]
fn configured_default_engine_selection_can_enable_anysearch_explicitly() {
    let config = search_config(HashMap::from([(
        "anysearch".to_string(),
        SearchEngineConfig {
            enabled: true,
            weight: 1.0,
            timeout: None,
        },
    )]));

    let (engines, source) = default_engine_selection(Some(&config));

    assert_eq!(engines, ["anysearch"]);
    assert_eq!(source, "config");
}

#[test]
fn config_acl_controls_the_default_engine_selection() {
    let config = crate::config::CodeConfig::from_acl(
        r#"
search {
  engine {
    anysearch {
      enabled = true
      weight = 1.0
    }
  }
}
"#,
    )
    .expect("valid search config");
    let search = config.search.as_ref().expect("search config");

    let (engines, source) = default_engine_selection(Some(search));

    assert_eq!(engines, ["anysearch"]);
    assert_eq!(source, "config");
}

#[test]
fn automatic_tier_plan_is_stable_and_deduplicated() {
    let plan = tiered_engine_plan(&["anysearch", "duckduckgo"], None, true);

    assert_eq!(plan.api, ["anysearch"]);
    assert_eq!(plan.http, ["ddg", "brave", "bing", "wiki"]);
    #[cfg(feature = "headless-search")]
    assert_eq!(plan.headless, ["g", "baidu"]);
    #[cfg(not(feature = "headless-search"))]
    assert!(plan.headless.is_empty());
}

#[test]
fn tier_plan_normalizes_aliases_and_respects_disabled_configuration() {
    let config = search_config(HashMap::from([
        (
            "duckduckgo".to_string(),
            SearchEngineConfig {
                enabled: false,
                weight: 1.0,
                timeout: None,
            },
        ),
        (
            "wikipedia".to_string(),
            SearchEngineConfig {
                enabled: false,
                weight: 1.0,
                timeout: None,
            },
        ),
    ]));

    let automatic = tiered_engine_plan(&["AnySearch"], Some(&config), true);
    assert_eq!(automatic.api, ["anysearch"]);
    assert_eq!(automatic.http, ["brave", "bing"]);
    #[cfg(feature = "headless-search")]
    assert_eq!(automatic.headless, ["g", "baidu"]);
    #[cfg(not(feature = "headless-search"))]
    assert!(automatic.headless.is_empty());

    let explicit = tiered_engine_plan(&["duckduckgo", "wikipedia"], None, false);
    assert!(explicit.api.is_empty());
    assert_eq!(explicit.http, ["ddg", "wiki"]);
    assert!(explicit.headless.is_empty());
}

#[test]
fn fallback_notice_uses_structured_failure_kinds_for_every_provider() {
    let failures = vec![
        EngineFailure::new("AnySearch", "provider_quota", "redacted").with_provider("anysearch"),
        EngineFailure::new("Tavily", "provider_rate_limited", "redacted")
            .with_provider("tavily")
            .with_transient(true),
    ];

    assert_eq!(
        failure_summary(&failures),
        "AnySearch quota is exhausted; Tavily was rate limited"
    );
    assert_eq!(failure_metadata(&failures)[0]["kind"], "provider_quota");
    assert_eq!(failure_metadata(&failures)[1]["provider"], "tavily");
}

#[test]
fn tool_error_kind_uses_structured_failure_kinds_instead_of_messages() {
    let rate_limited = [
        EngineFailure::new("Provider A", "provider_rate_limited", "opaque"),
        EngineFailure::new("Provider B", "rate_limited", "unrelated text"),
    ];
    assert_eq!(
        tool_error_kind_for_failures(&rate_limited, Duration::from_secs(10)),
        Some(ToolErrorKind::RateLimited {
            retry_after_ms: None,
        })
    );

    let timed_out = [
        EngineFailure::new("Provider A", "timeout", "opaque"),
        EngineFailure::new("Provider B", "http_timeout", "unrelated text"),
    ];
    assert_eq!(
        tool_error_kind_for_failures(&timed_out, Duration::from_secs(10)),
        Some(ToolErrorKind::Timeout {
            op: "web_search".to_string(),
            duration_ms: 10_000,
        })
    );

    let mixed = [
        EngineFailure::new("Provider A", "provider_quota", "rate limit"),
        EngineFailure::new("Provider B", "provider_rate_limited", "rate limit"),
    ];
    assert_eq!(
        tool_error_kind_for_failures(&mixed, Duration::from_secs(10)),
        None,
        "quota exhaustion must remain distinguishable in engine_failures metadata"
    );
}

#[test]
fn tier_timeout_preserves_a_share_for_each_remaining_tier() {
    assert_eq!(
        tier_timeout(Duration::from_secs(12), 0),
        Duration::from_secs(12)
    );
    assert_eq!(
        tier_timeout(Duration::from_secs(12), 1),
        Duration::from_secs(6)
    );
    assert_eq!(
        tier_timeout(Duration::from_secs(12), 2),
        Duration::from_secs(4)
    );
    assert_eq!(
        tier_timeout(Duration::from_millis(2), 2),
        Duration::from_millis(1)
    );
}

#[test]
fn default_engine_selection_respects_explicit_configuration() {
    let config = search_config(HashMap::from([
        (
            "enabled".to_string(),
            SearchEngineConfig {
                enabled: true,
                weight: 1.0,
                timeout: None,
            },
        ),
        (
            "disabled".to_string(),
            SearchEngineConfig {
                enabled: false,
                weight: 1.0,
                timeout: None,
            },
        ),
    ]));
    let (engines, source) = default_engine_selection(Some(&config));
    assert_eq!(engines, ["enabled"]);
    assert_eq!(source, "config");

    let config = search_config(HashMap::from([(
        "disabled".to_string(),
        SearchEngineConfig {
            enabled: false,
            weight: 1.0,
            timeout: None,
        },
    )]));
    let (engines, source) = default_engine_selection(Some(&config));
    assert!(engines.is_empty());
    assert_eq!(source, "config");
}

#[test]
fn configured_default_engine_selection_deduplicates_aliases() {
    let config = search_config(HashMap::from([
        (
            "ddg".to_string(),
            SearchEngineConfig {
                enabled: true,
                weight: 1.0,
                timeout: None,
            },
        ),
        (
            "duckduckgo".to_string(),
            SearchEngineConfig {
                enabled: true,
                weight: 1.0,
                timeout: None,
            },
        ),
    ]));

    let (engines, source) = default_engine_selection(Some(&config));
    assert_eq!(engines, ["ddg"]);
    assert_eq!(source, "config");
}

#[test]
fn configured_engine_aliases_are_executable() {
    let mut search = Search::new();
    assert!(add_http_engine(&mut search, "duckduckgo", None).expect("engine setup"));
    assert!(add_http_engine(&mut search, "wikipedia", None).expect("engine setup"));
    assert_eq!(search.engine_count(), 2);
}

#[test]
fn provider_setup_failures_remain_typed_for_the_cascade() {
    let error = a3s_search::SearchError::Other("provider setup failed".to_string());
    let failure = super::engines::provider_setup_failure(
        a3s_search::providers::BuiltinProvider::AnySearch,
        &error,
    );

    assert_eq!(failure.engine, "anysearch");
    assert_eq!(failure.provider.as_deref(), Some("anysearch"));
    assert_eq!(failure.kind, "other");
    assert!(!failure.transient);
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
    let metadata = result.metadata.expect("search selection metadata");
    assert_eq!(metadata["status"], "failed");
    assert_eq!(metadata["engine_selection_source"], "request");
    assert_eq!(
        metadata["selected_engines"],
        serde_json::json!(["nonexistent"])
    );
}

#[tokio::test]
async fn configured_engine_selection_is_identified_in_failure_metadata() {
    let tool = WebSearchTool::new();
    let ctx = ToolContext::new(PathBuf::from("/tmp")).with_search_config(SearchConfig {
        timeout: 10,
        health: None,
        engines: HashMap::from([(
            "private-search".to_string(),
            SearchEngineConfig {
                enabled: true,
                weight: 1.0,
                timeout: None,
            },
        )]),
        headless: None,
    });

    let result = tool
        .execute(&serde_json::json!({"query": "test"}), &ctx)
        .await
        .unwrap();

    assert!(!result.success);
    let metadata = result.metadata.expect("search selection metadata");
    assert_eq!(metadata["status"], "failed");
    assert_eq!(metadata["engine_selection_source"], "config");
    assert_eq!(
        metadata["selected_engines"],
        serde_json::json!(["private-search"])
    );
}

#[tokio::test]
#[ignore = "requires external network"]
async fn real_builtin_default_search_uses_external_probe_query() {
    let query = std::env::var("A3S_WEB_SEARCH_PROBE_QUERY")
        .expect("set A3S_WEB_SEARCH_PROBE_QUERY for an external diagnostic query");
    let tool = WebSearchTool::new();
    let ctx = ToolContext::new(PathBuf::from("/tmp"));
    let result = tool
        .execute(
            &serde_json::json!({
                "query": query,
                "limit": 10,
                "timeout": 30,
                "format": "json",
                "full_text_bytes": 8192
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(result.success, "{}", result.content);
    let items: serde_json::Value = serde_json::from_str(&result.content)
        .unwrap_or_else(|error| panic!("JSON search results ({error}): {}", result.content));
    assert!(
        items.as_array().is_some_and(|items| !items.is_empty()),
        "{}",
        result.content
    );
    let metadata = result.metadata.expect("default search metadata");
    assert_eq!(metadata["engine_selection_source"], "builtin_default");
    assert!(metadata["selected_engines"]
        .as_array()
        .is_some_and(|engines| !engines.is_empty()));
    let summaries = items
        .as_array()
        .expect("search result array")
        .iter()
        .map(|item| {
            let full_text = item
                .get("full_text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let content = item
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            serde_json::json!({
                "title": item.get("title"),
                "url": item.get("url"),
                "engines": item.get("engines"),
                "published_date": item.get("published_date"),
                "content_preview": crate::text::truncate_utf8(content, 480),
                "full_text_bytes": full_text.len(),
                "full_text_preview": crate::text::truncate_utf8(full_text, 240),
            })
        })
        .collect::<Vec<_>>();
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "metadata": metadata,
            "full_text_result_count": summaries.iter().filter(|item| {
                item["full_text_bytes"].as_u64().unwrap_or_default() > 0
            }).count(),
            "results": summaries,
        }))
        .unwrap()
    );
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
    assert_eq!(
        super::engines::engine_tier("bing_cn"),
        Some(super::engines::EngineTier::Http)
    );
    #[cfg(feature = "headless-search")]
    assert_eq!(
        super::engines::engine_tier("google"),
        Some(super::engines::EngineTier::Headless)
    );
    #[cfg(not(feature = "headless-search"))]
    assert_eq!(super::engines::engine_tier("google"), None);
}

#[cfg(feature = "headless-search")]
#[test]
fn explicit_headless_selection_does_not_invent_earlier_tiers() {
    let plan = tiered_engine_plan(&["google"], None, false);
    assert!(plan.api.is_empty());
    assert!(plan.http.is_empty());
    assert_eq!(plan.headless, ["g"]);
}

#[cfg(not(feature = "headless-search"))]
#[test]
fn headless_selection_is_unavailable_without_the_feature() {
    let plan = tiered_engine_plan(&["google", "baidu"], None, false);
    assert!(plan.is_empty());
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
    assert!(params["properties"]["engines"]["description"]
        .as_str()
        .is_some_and(|description| {
            description.contains("bing (Bing RSS)")
                && description.contains("anysearch")
                && description.contains("tavily")
        }));
    #[cfg(feature = "headless-search")]
    assert!(params["properties"]["engines"]["description"]
        .as_str()
        .is_some_and(|description| description.contains("Google, headless")));
    #[cfg(not(feature = "headless-search"))]
    assert!(params["properties"]["engines"]["description"]
        .as_str()
        .is_some_and(|description| !description.contains("Google")));
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
    assert!(add_http_engine(&mut search, "ddg", None).expect("engine setup"));
    assert_eq!(search.engine_count(), 1);

    assert!(add_http_engine(&mut search, "wiki", None).expect("engine setup"));
    assert_eq!(search.engine_count(), 2);

    assert!(add_http_engine(&mut search, "brave", None).expect("engine setup"));
    assert_eq!(search.engine_count(), 3);

    assert!(add_http_engine(&mut search, "bing", None).expect("engine setup"));
    assert_eq!(search.engine_count(), 4);

    assert!(add_http_engine(&mut search, "bing_cn", None).expect("engine setup"));
    assert_eq!(search.engine_count(), 5);

    assert!(add_http_engine(&mut search, "anysearch", None).expect("engine setup"));
    assert_eq!(search.engine_count(), 6);

    assert!(add_http_engine(&mut search, "tavily", None).expect("engine setup"));
    assert_eq!(search.engine_count(), 7);
}

#[test]
fn test_add_http_engine_unknown() {
    let mut search = Search::new();
    assert!(!add_http_engine(&mut search, "nonexistent", None).expect("engine setup"));
    assert_eq!(search.engine_count(), 0);
}

#[cfg(feature = "headless-search")]
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

#[cfg(feature = "headless-search")]
#[test]
fn test_add_headless_engine_aliases() {
    let mut search = Search::new();
    let pool_config = BrowserPoolConfig::default();
    let pool = Arc::new(BrowserPool::new(pool_config));

    assert!(add_headless_engine(&mut search, "g", &pool));
    assert_eq!(search.engine_count(), 1);
}

#[cfg(feature = "headless-search")]
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
    let valid_fields = [
        "query",
        "engines",
        "limit",
        "timeout",
        "proxy",
        "format",
        "full_text_bytes",
    ];
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

    let json = search_result_json(&result, None);

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
fn json_search_result_includes_only_requested_bounded_sanitized_full_text() {
    let mut result = SearchResult::new("https://example.com/source", "Source", "Summary");
    result.full_text = Some(format!(
        "Evidence at https://reader:password@example.com/private?token=secret#fragment {}",
        "x".repeat(2_000)
    ));

    let omitted = search_result_json(&result, None);
    assert!(omitted.get("full_text").is_none());

    let included = search_result_json(&result, Some(MIN_FULL_TEXT_BYTES));
    let full_text = included["full_text"].as_str().unwrap();
    assert!(full_text.len() <= MIN_FULL_TEXT_BYTES);
    assert!(full_text.contains("https://example.com/private"));
    for secret in ["reader", "password", "token", "secret", "fragment"] {
        assert!(!full_text.contains(secret), "leaked {secret}: {full_text}");
    }
}

#[test]
fn json_search_result_bounds_provider_title_and_summary_fields() {
    let result = SearchResult::new(
        "https://example.com/source",
        "t".repeat(MAX_JSON_TITLE_BYTES * 2),
        "c".repeat(MAX_JSON_CONTENT_BYTES * 2),
    );

    let json = search_result_json(&result, None);

    assert!(json["title"].as_str().unwrap().len() <= MAX_JSON_TITLE_BYTES);
    assert!(json["content"].as_str().unwrap().len() <= MAX_JSON_CONTENT_BYTES);
}

#[test]
fn json_search_result_collection_stays_valid_below_tool_transport_limit() {
    let results = (0..16)
        .map(|index| {
            let mut result = SearchResult::new(
                format!("https://example.com/source-{index}"),
                "title".repeat(600),
                "summary".repeat(1_200),
            );
            result.full_text = Some("evidence".repeat(8_000));
            result
        })
        .collect::<Vec<_>>();
    let references = results.iter().collect::<Vec<_>>();

    let bounded = bounded_json_search_results(&references, Some(MAX_FULL_TEXT_BYTES));
    let encoded = serde_json::to_vec(&bounded).unwrap();

    assert!(!bounded.is_empty());
    assert!(bounded.len() < results.len());
    assert!(encoded.len() <= MAX_JSON_OUTPUT_BYTES);
    assert!(serde_json::from_slice::<serde_json::Value>(&encoded).is_ok());
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
