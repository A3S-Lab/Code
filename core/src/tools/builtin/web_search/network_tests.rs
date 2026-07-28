use super::*;
use crate::config::{SearchConfig, SearchEngineConfig};
use std::collections::HashMap;
use std::path::PathBuf;

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

    let payload: serde_json::Value = serde_json::from_str(&result.content)
        .unwrap_or_else(|error| panic!("JSON search results ({error}): {}", result.content));
    let metadata = result.metadata.expect("default search metadata");
    let quality_met = metadata["search_fallback"]["successful"]
        .as_bool()
        .expect("quality outcome");
    assert_eq!(result.success, quality_met, "{}", result.content);
    let items = payload
        .as_array()
        .or_else(|| payload.get("results").and_then(serde_json::Value::as_array))
        .expect("search result array or quality diagnostic envelope");
    assert!(!items.is_empty(), "{}", result.content);
    assert_eq!(metadata["engine_selection_source"], "builtin_default");
    assert!(metadata["selected_engines"]
        .as_array()
        .is_some_and(|engines| !engines.is_empty()));
    let summaries = items
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
                "query_match_score": item.get("query_match_score"),
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
#[ignore = "requires external network, a headless browser, and an intentionally unavailable AnySearch credential"]
async fn real_headless_tier_takes_over_after_api_failure() {
    let query = std::env::var("A3S_WEB_SEARCH_PROBE_QUERY")
        .expect("set A3S_WEB_SEARCH_PROBE_QUERY for an external diagnostic query");
    let engine = |enabled| SearchEngineConfig {
        enabled,
        weight: 1.0,
        timeout: None,
    };
    let ctx = ToolContext::new(PathBuf::from("/tmp")).with_search_config(SearchConfig {
        timeout: 45,
        health: None,
        engines: HashMap::from([
            ("anysearch".to_string(), engine(true)),
            ("ddg".to_string(), engine(false)),
            ("brave".to_string(), engine(false)),
            ("bing".to_string(), engine(false)),
            ("wiki".to_string(), engine(false)),
        ]),
        headless: None,
    });
    let result = WebSearchTool::new()
        .execute(
            &serde_json::json!({
                "query": query,
                "limit": 5,
                "timeout": 45,
                "format": "json"
            }),
            &ctx,
        )
        .await
        .expect("headless cascade execution");

    let metadata = result.metadata.clone().expect("headless search metadata");
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "metadata": metadata,
            "results": serde_json::from_str::<serde_json::Value>(&result.content).ok(),
        }))
        .expect("diagnostic JSON")
    );
    assert!(result.success, "{}", result.content);
    let tiers = metadata["search_tiers"]
        .as_array()
        .expect("search tier reports")
        .iter()
        .filter_map(|tier| tier["tier"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(tiers, ["api", "headless"]);
    assert!(serde_json::from_str::<serde_json::Value>(&result.content)
        .ok()
        .and_then(|items| items.as_array().map(|items| !items.is_empty()))
        .unwrap_or(false));
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
