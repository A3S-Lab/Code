use super::*;
use crate::config::{SearchConfig, SearchEngineConfig};
use a3s_search::{EngineFailure, RetrievalHealth, RetrievalRequirements, Search};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;

#[cfg(feature = "headless-search")]
#[test]
fn default_headless_backend_is_bundled_moli() {
    assert_eq!(HeadlessConfig::default().backend, BrowserBackend::Moli);
    assert!(HeadlessConfig::default().auto_download_moli);
}

#[cfg(feature = "headless-search")]
#[test]
fn request_proxy_is_applied_to_the_headless_tier() {
    let configured = HeadlessConfig {
        proxy_url: Some("http://configured.example:8080".to_string()),
        ..HeadlessConfig::default()
    };

    let effective =
        effective_headless_config(Some(&configured), Some("socks5://request.example:1080"));

    assert_eq!(effective.backend, BrowserBackend::Moli);
    assert_eq!(
        effective.proxy_url.as_deref(),
        Some("socks5://request.example:1080")
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

#[test]
fn latest_request_coalescing_state_is_exposed_as_stable_metadata() {
    let mut snapshot = a3s_search::SearchCoalescerSnapshot::default();
    snapshot.max_in_flight = 128;
    snapshot.in_flight = 2;
    snapshot.leader_requests = 7;
    snapshot.shared_requests = 5;
    snapshot.bypassed_requests = 1;
    snapshot.abandoned_requests = 1;

    let metadata = search_coalescer_json(&snapshot);

    assert_eq!(metadata["max_in_flight"], 128);
    assert_eq!(metadata["in_flight"], 2);
    assert_eq!(metadata["leader_requests"], 7);
    assert_eq!(metadata["shared_requests"], 5);
    assert_eq!(metadata["bypassed_requests"], 1);
    assert_eq!(metadata["abandoned_requests"], 1);
}

struct CoalescingProbeEngine {
    config: a3s_search::EngineConfig,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl a3s_search::Engine for CoalescingProbeEngine {
    fn config(&self) -> &a3s_search::EngineConfig {
        &self.config
    }

    async fn search(
        &self,
        query: &a3s_search::SearchQuery,
    ) -> a3s_search::Result<Vec<a3s_search::SearchResult>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        Ok(vec![a3s_search::SearchResult::new(
            "https://example.test/coalesced",
            query.query.clone(),
            "shared result",
        )])
    }
}

#[tokio::test]
async fn tier_searches_share_the_session_request_coalescer() {
    let context = ToolContext::new(PathBuf::from("."));
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let [mut first, mut second] = [
        cascade_controls(&context, Arc::new(Metrics::new())),
        cascade_controls(&context, Arc::new(Metrics::new())),
    ]
    .map(|controls| Search::new().with_request_coalescer(controls.coalescer));
    for search in [&mut first, &mut second] {
        search.add_engine(CoalescingProbeEngine {
            config: a3s_search::EngineConfig {
                name: "Coalescing Probe".to_string(),
                shortcut: "coalescing_probe".to_string(),
                ..a3s_search::EngineConfig::default()
            },
            calls: Arc::clone(&calls),
        });
    }
    let query = SearchQuery::new("same concurrent request");

    let (first_result, second_result) =
        tokio::join!(first.search(query.clone()), second.search(query));

    assert!(first_result.is_ok());
    assert!(second_result.is_ok());
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn search_navigation_rows_are_not_admitted_to_the_structural_gate() {
    let context = ToolContext::new(PathBuf::from("."));
    let admission = cascade_controls(&context, Arc::new(Metrics::new()))
        .admission
        .expect("Code admits only safe source URLs");

    assert!(admission(&SearchResult::new(
        "https://example.com/source",
        "Source",
        "Evidence"
    )));
    assert!(!admission(&SearchResult::new(
        "https://search.brave.com/search?q=rust",
        "Navigation",
        "Not a source"
    )));
    assert!(!admission(&SearchResult::new(
        "javascript:alert(1)",
        "Unsafe",
        "Unsafe"
    )));
}

fn search_config(engines: HashMap<String, SearchEngineConfig>) -> SearchConfig {
    SearchConfig {
        timeout: 10,
        cascade_order: None,
        health: None,
        engines,
        headless: None,
    }
}

#[test]
fn builtin_default_plan_is_api_first_and_leaves_billed_providers_out() {
    let plan = EngineTierPlan::new(None, None, None).unwrap();

    assert_eq!(plan.tiers()[0].0, a3s_search::EngineTier::Api);
    assert_eq!(plan.api(), ["anysearch", "tavily"]);
    assert!(!plan.shortcuts().iter().any(|shortcut| shortcut == "a3s"));
}

#[test]
fn config_acl_engines_bridge_into_the_search_plan() {
    let config = crate::config::CodeConfig::from_acl(
        r#"
search {
  engine {
    anysearch {
      enabled = true
      weight = 1.0
    }
    duckduckgo {
      enabled = true
    }
    ddg {
      enabled = true
    }
    wiki {
      enabled = false
    }
  }
}
"#,
    )
    .expect("valid search config");
    let search = config.search.as_ref().expect("search config");

    let (bridged, failures) = bridge_search_config(search);
    assert!(failures.is_empty(), "{failures:?}");
    let plan = EngineTierPlan::new(None, Some(&bridged), None).unwrap();

    assert_eq!(plan.api(), ["anysearch"]);
    assert_eq!(plan.http_rss(), ["ddg"]);
    assert!(plan.headless().is_empty());
}

#[test]
fn cascade_order_lists_tiers_first_and_keeps_the_rest_in_default_order() {
    use crate::config::SearchCascadeTier;
    use a3s_search::EngineTier;

    assert_eq!(super::engines::tier_order(&[]), EngineTier::default_order());
    assert_eq!(
        super::engines::tier_order(&[SearchCascadeTier::Headless]),
        [EngineTier::Headless, EngineTier::Api, EngineTier::HttpRss]
    );
    assert_eq!(
        super::engines::tier_order(&[
            SearchCascadeTier::Api,
            SearchCascadeTier::Http,
            SearchCascadeTier::Api,
        ]),
        EngineTier::default_order()
    );
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

#[tokio::test]
async fn a_provider_with_a_rejected_endpoint_never_runs_with_its_defaults() {
    let tool = WebSearchTool::new();
    let ctx =
        ToolContext::new(PathBuf::from("/tmp")).with_search_config(search_config(HashMap::from([
            (
                "tavily".to_string(),
                SearchEngineConfig {
                    enabled: true,
                    weight: 1.0,
                    timeout: None,
                    api_key: None,
                    project: None,
                    endpoint: Some("http://example.com/search".to_string()),
                    authoritative_only: None,
                    query_rewrite: None,
                },
            ),
        ])));

    let result = tool
        .execute(&serde_json::json!({"query": "test"}), &ctx)
        .await
        .unwrap();

    assert!(!result.success);
    assert!(
        result.content.contains("No valid engines"),
        "{}",
        result.content
    );
    let metadata = result.metadata.expect("search selection metadata");
    assert_eq!(metadata["engine_selection_source"], "config");
    assert_eq!(metadata["engine_failures"][0]["engine"], "tavily");
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
        cascade_order: None,
        health: None,
        engines: HashMap::from([(
            "private-search".to_string(),
            SearchEngineConfig {
                enabled: true,
                weight: 1.0,
                timeout: None,
                api_key: None,
                project: None,
                endpoint: None,
                authoritative_only: None,
                query_rewrite: None,
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
fn explicit_selection_does_not_invent_other_tiers() {
    let http = EngineTierPlan::new(Some(&["bing_cn".to_string()]), None, None).unwrap();
    assert_eq!(http.http_rss(), ["bing_cn"]);
    assert!(http.api().is_empty() && http.headless().is_empty());

    let headless = EngineTierPlan::new(Some(&["google".to_string()]), None, None).unwrap();
    assert_eq!(headless.headless(), ["g"]);
    assert!(headless.api().is_empty() && headless.http_rss().is_empty());
}

#[cfg(not(feature = "headless-search"))]
fn browser_process_ids() -> std::collections::BTreeSet<(String, u32)> {
    let mut command = if cfg!(windows) {
        let mut command = std::process::Command::new("tasklist");
        command.args(["/FO", "CSV", "/NH"]);
        command
    } else {
        let mut command = std::process::Command::new("ps");
        command.args(["-A", "-o", "pid=,comm="]);
        command
    };
    let output = command.output().expect("list processes");
    let text = String::from_utf8_lossy(&output.stdout);
    let mut ids = std::collections::BTreeSet::new();
    for line in text.lines() {
        let (name, pid) = if cfg!(windows) {
            let mut parts = line.split("\",\"");
            let Some(name) = parts.next() else { continue };
            let Some(pid) = parts.next() else { continue };
            (
                name.trim_matches('"').to_ascii_lowercase(),
                pid.trim_matches('"').parse::<u32>().ok(),
            )
        } else {
            let mut parts = line.split_whitespace();
            let Some(pid) = parts.next() else { continue };
            let Some(name) = parts.next() else { continue };
            (name.to_ascii_lowercase(), pid.parse::<u32>().ok())
        };
        let Some(pid) = pid else { continue };
        if name.contains("moli") || name.contains("lightpanda") || name.contains("chrom") {
            ids.insert((name, pid));
        }
    }
    ids
}

#[cfg(not(feature = "headless-search"))]
#[tokio::test]
async fn headless_engine_request_does_not_spawn_moli() {
    let before = browser_process_ids();
    let tool = WebSearchTool::new();
    let context = ToolContext::new(PathBuf::from("."));
    let output = tool
        .execute(
            &serde_json::json!({
                "query": "kernel bound",
                "engines": "google"
            }),
            &context,
        )
        .await
        .expect("web_search returns a typed result");

    assert!(!output.success);
    let metadata = output.metadata.as_ref().expect("search metadata");
    assert_eq!(
        metadata["engine_failures"][0]["kind"], "headless_unavailable",
        "headless engines must be a typed failure without the feature: {output:?}"
    );
    let spawned = browser_process_ids()
        .difference(&before)
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        spawned.is_empty(),
        "a headless request spawned a browser process: {spawned:?}"
    );
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
    #[cfg(feature = "headless-search")]
    assert!(params["properties"]["engines"]["description"]
        .as_str()
        .is_some_and(|description| {
            description.contains("bing_browser")
                && description.contains("brave_browser")
                && description.contains("Moli")
        }));
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
fn every_named_provider_bridges_from_code_config() {
    let providers = [
        "anysearch",
        "tavily",
        "tinyfish",
        "bocha",
        "aliyun",
        "tencent",
        "firecrawl",
        "a3s",
    ];
    let config = search_config(
        providers
            .iter()
            .map(|provider| {
                (
                    provider.to_string(),
                    SearchEngineConfig {
                        enabled: true,
                        weight: 1.0,
                        timeout: None,
                        api_key: None,
                        project: None,
                        endpoint: None,
                        authoritative_only: None,
                        query_rewrite: None,
                    },
                )
            })
            .collect(),
    );

    let (bridged, failures) = bridge_search_config(&config);

    assert!(failures.is_empty(), "{failures:?}");
    for provider in providers {
        assert!(
            bridged.provider_entry(provider).is_some(),
            "{provider} must be constructible when named"
        );
    }
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
    assert!(json.get("query_match_score").is_none());
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
fn json_search_payload_preserves_the_array_contract_when_requirements_are_met() {
    let results = vec![serde_json::json!({
        "title": "Portable evidence",
        "url": "https://example.test/evidence"
    })];

    let payload = json_search_payload(results.clone());

    assert_eq!(payload, serde_json::Value::Array(results));
}

#[test]
fn json_search_payload_keeps_the_array_contract_below_retrieval_requirements() {
    let health = RetrievalHealth::default();
    let requirements = RetrievalRequirements::for_limit(1);
    assert!(
        !requirements.is_met(&health),
        "an empty health snapshot cannot pass the structural gate"
    );

    let results = vec![serde_json::json!({
        "title": "Weak candidate",
        "url": "https://example.test/candidate"
    })];
    let payload = json_search_payload(results.clone());

    assert_eq!(payload, serde_json::Value::Array(results));
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

#[test]
fn unconfigured_search_leaves_a3s_out_of_the_default_cascade() {
    let plan = EngineTierPlan::builtin_default();
    assert!(!plan.shortcuts().iter().any(|shortcut| shortcut == "a3s"));
}

#[tokio::test]
async fn configured_agent_search_posts_to_the_gateway_route() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let endpoint = format!("http://{address}/v1/search");
    let (capture_tx, capture_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let header_end = loop {
            let read = stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                return;
            }
            request.extend_from_slice(&buffer[..read]);
            if let Some(offset) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break offset + 4;
            }
        };
        let headers = String::from_utf8_lossy(&request[..header_end]).into_owned();
        let _ = capture_tx.send(headers);
        let body = r#"{"success":true,"data":{"results":{"web":[{"url":"https://www.rust-lang.org/","title":"Rust","description":"language"}]}}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
    });

    let config = SearchConfig {
        timeout: 5,
        cascade_order: None,
        health: None,
        engines: HashMap::from([(
            "a3s".to_string(),
            SearchEngineConfig {
                enabled: true,
                weight: 1.0,
                timeout: None,
                api_key: Some("caller-secret".to_string()),
                project: None,
                endpoint: Some(endpoint),
                authoritative_only: None,
                query_rewrite: None,
            },
        )]),
        headless: None,
    };
    let (bridged, failures) = bridge_search_config(&config);
    assert!(failures.is_empty(), "{failures:?}");
    let engine = bridged
        .create_provider_engine("a3s")
        .expect("a3s setup")
        .expect("named a3s must register against the gateway endpoint");
    let mut search = Search::new();
    search.add_engine(engine);
    let results = search
        .search(a3s_search::SearchQuery::new("rust"))
        .await
        .expect("a3s search");
    assert!(
        results.failures().is_empty(),
        "a3s search failed: {:?}",
        results.failures()
    );
    assert_eq!(results.items().len(), 1);
    let headers = capture_rx.await.expect("gateway capture");
    assert!(headers.contains("POST /v1/search"), "{headers}");
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("authorization: bearer caller-secret"),
        "{headers}"
    );
}

#[test]
fn configured_api_endpoint_rejects_non_loopback_http() {
    let config = search_config(HashMap::from([(
        "tavily".to_string(),
        SearchEngineConfig {
            enabled: true,
            weight: 1.0,
            timeout: None,
            api_key: None,
            project: None,
            endpoint: Some("http://example.com/search".to_string()),
            authoritative_only: None,
            query_rewrite: None,
        },
    )]));

    let (bridged, failures) = bridge_search_config(&config);
    let failure = failures
        .first()
        .expect("non-loopback http endpoint must fail closed");
    assert!(bridged.provider_entry("tavily").is_none());
    assert_eq!(failure.engine, "tavily");
    assert_eq!(failure.provider.as_deref(), Some("tavily"));
    assert!(
        failure.kind.contains("invalid") || failure.message.to_lowercase().contains("https"),
        "{failure:?}"
    );
}

#[test]
fn configured_doubao_engine_bridges_to_a_provider() {
    let config = search_config(HashMap::from([(
        "doubao".to_string(),
        SearchEngineConfig {
            enabled: true,
            weight: 1.0,
            timeout: None,
            api_key: Some("doubao-test-key".to_string()),
            project: None,
            endpoint: None,
            authoritative_only: Some(true),
            query_rewrite: Some(true),
        },
    )]));

    let (bridged, failures) = bridge_search_config(&config);
    assert!(failures.is_empty(), "doubao must bridge: {failures:?}");
    assert!(bridged.provider_entry("doubao").is_some());
}

#[test]
fn configured_api_endpoint_accepts_loopback_http() {
    let config = search_config(HashMap::from([(
        "tavily".to_string(),
        SearchEngineConfig {
            enabled: true,
            weight: 1.0,
            timeout: None,
            api_key: None,
            project: None,
            endpoint: Some("http://127.0.0.1:9/search".to_string()),
            authoritative_only: None,
            query_rewrite: None,
        },
    )]));

    let (bridged, failures) = bridge_search_config(&config);
    assert!(
        failures.is_empty(),
        "loopback fixture endpoint must bridge: {failures:?}"
    );
    assert!(bridged.provider_entry("tavily").is_some());
}

/// S-WS-01: 100 searches against a loopback Tavily fixture stay connection-
/// and byte-bounded. Endpoint override stays SSRF-safe (loopback HTTP only).
#[tokio::test]
#[ignore = "S-WS-01 soak: loopback search fixture connection and byte caps"]
async fn soak_web_search_loopback_fixture_stays_connection_and_byte_capped() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const CYCLES: usize = 100;
    const POOL_IDLE_CAP: usize = 4;

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let addr = listener.local_addr().expect("addr");
    let endpoint = format!("http://127.0.0.1:{}/search", addr.port());

    let open = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let hits = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));

    let serve_open = Arc::clone(&open);
    let serve_peak = Arc::clone(&peak);
    let serve_hits = Arc::clone(&hits);
    let serve_stop = Arc::clone(&stop);
    let server = tokio::spawn(async move {
        loop {
            if serve_stop.load(Ordering::SeqCst) {
                break;
            }
            let accept = tokio::time::timeout(Duration::from_millis(50), listener.accept()).await;
            let Ok(Ok((mut stream, _))) = accept else {
                continue;
            };
            let open = Arc::clone(&serve_open);
            let peak = Arc::clone(&serve_peak);
            let hits = Arc::clone(&serve_hits);
            tokio::spawn(async move {
                let current = open.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(current, Ordering::SeqCst);
                let mut buf = vec![0u8; 16 * 1024];
                let _ = stream.read(&mut buf).await;
                hits.fetch_add(1, Ordering::SeqCst);
                let body = serde_json::json!({
                    "results": [{
                        "title": "Fixture",
                        "url": "https://example.com/fixture",
                        "content": "X".repeat(64 * 1024)
                    }]
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
                open.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });

    let tool = WebSearchTool::new();
    let ctx = ToolContext::new(std::env::temp_dir()).with_search_config(SearchConfig {
        timeout: 10,
        cascade_order: None,
        health: None,
        engines: HashMap::from([(
            "tavily".to_string(),
            SearchEngineConfig {
                enabled: true,
                weight: 1.0,
                timeout: None,
                api_key: None,
                project: None,
                endpoint: Some(endpoint),
                authoritative_only: None,
                query_rewrite: None,
            },
        )]),
        headless: None,
    });

    for index in 0..CYCLES {
        let output = tool
            .execute(
                &serde_json::json!({
                    "query": format!("soak-{index}"),
                    "engines": ["tavily"],
                    "format": "json",
                    "limit": 1
                }),
                &ctx,
            )
            .await
            .expect("typed result");
        assert!(output.success, "cycle {index} failed: {}", output.content);
        assert!(
            output.content.len() <= MAX_JSON_OUTPUT_BYTES,
            "cycle {index} exceeded output cap: {} > {}",
            output.content.len(),
            MAX_JSON_OUTPUT_BYTES
        );
        assert!(
            open.load(Ordering::SeqCst) <= POOL_IDLE_CAP,
            "open connections exceeded pool cap mid-soak: {}",
            open.load(Ordering::SeqCst)
        );
    }

    tokio::time::sleep(Duration::from_millis(250)).await;
    let idle_open = open.load(Ordering::SeqCst);
    stop.store(true, Ordering::SeqCst);
    let _ = server.await;

    assert_eq!(hits.load(Ordering::SeqCst), CYCLES, "fixture hit count");
    assert!(
        idle_open <= POOL_IDLE_CAP,
        "idle open connections {idle_open} exceeded pool cap {POOL_IDLE_CAP} (fail if == {CYCLES})"
    );
    assert_ne!(
        idle_open, CYCLES,
        "each search must not leave a live socket"
    );
    assert!(
        peak.load(Ordering::SeqCst) <= POOL_IDLE_CAP,
        "peak open {} exceeded pool cap",
        peak.load(Ordering::SeqCst)
    );
}

struct WikiInvoker {
    output: String,
    exit_code: i32,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl crate::tools::ToolInvoker for WikiInvoker {
    async fn invoke(
        &self,
        _invocation: crate::tools::ToolInvocation,
        _ctx: &ToolContext,
    ) -> crate::tools::ToolResult {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.exit_code == 0 {
            crate::tools::ToolResult::success("search_wiki", self.output.clone())
        } else {
            crate::tools::ToolResult::error("search_wiki", self.output.clone())
        }
    }

    fn available_tools(&self) -> Vec<String> {
        vec!["search_wiki".to_string()]
    }
}

#[test]
fn saved_notes_decide_whether_the_web_runs() {
    assert!(matches!(
        local_first_outcome("no-match", false),
        LocalFirst::SearchWeb
    ));
    assert!(matches!(
        local_first_outcome("not-ready", false),
        LocalFirst::SearchWeb
    ));
    assert!(matches!(
        local_first_outcome("denied", true),
        LocalFirst::SearchWeb
    ));
    assert!(matches!(
        local_first_outcome("index failed", true),
        LocalFirst::Stop(_)
    ));
    match local_first_outcome("林夏\n百科", false) {
        LocalFirst::Answer(text) => assert!(text.contains("林夏")),
        other => panic!("expected a local answer, got {other:?}"),
    }
    let session = "local-first-unit";
    let _ = claim_web_escalation(session, "林夏");
    assert!(!claim_web_escalation(session, "林夏"));
    remember_web_escalation(session, "林夏");
    assert!(claim_web_escalation(session, "林夏"));
    assert!(!claim_web_escalation(session, "林夏"));
}

#[tokio::test]
async fn web_search_returns_saved_notes_without_searching_the_web() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let session = "local-first-execute";
    let query = "谁是林夏";
    let _ = claim_web_escalation(session, query);
    let ctx = ToolContext::new(PathBuf::from("/tmp"))
        .with_session_id(session)
        .with_tool_invoker(Arc::new(WikiInvoker {
            output: "林夏只在本地笔记".to_string(),
            exit_code: 0,
            calls: Arc::clone(&calls),
        }));
    let output = WebSearchTool::new()
        .execute(&serde_json::json!({ "query": query }), &ctx)
        .await
        .expect("local knowledge answers");
    assert!(output.success);
    assert!(output.content.contains("林夏只在本地笔记"));
    assert!(output.content.contains("local-knowledge"));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unawaited_provisioning_is_aborted_on_drop() {
    let task = tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    });
    drop(AbortOnDrop(task.abort_handle()));
    let joined = tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .expect("an aborted prep must not block the fast path");
    assert!(joined.unwrap_err().is_cancelled());
}
