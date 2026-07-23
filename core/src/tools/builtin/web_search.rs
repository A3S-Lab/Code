//! Web search tool - Search the web via a3s-search

mod engines;
mod fallback;

use crate::config::{BrowserBackend, HeadlessConfig};
use crate::tools::types::{Tool, ToolContext, ToolErrorKind, ToolOutput};
use a3s_search::a3s_use_browser::{BrowserPool, BrowserPoolConfig, BrowserProvider};
use a3s_search::proxy::{ProxyConfig, ProxyPool};
use a3s_search::{
    EngineFailure, Metrics, MetricsSnapshot, Search, SearchQuery, SearchResult, SearchResults,
};
use anyhow::Result;
use async_trait::async_trait;
use regex::Regex;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use engines::{
    add_headless_engine, add_http_engine, default_engine_selection, requires_headless_browser,
    should_fallback_from_unavailable_headless, should_reject_engine_selection,
};
use fallback::{
    configured_engine, failure_metadata, failure_summary, fallback_engine_names,
    fallback_engine_shortcuts, merge_search_results, primary_search_timeout, text_notice_note,
    tool_error_kind_for_failures, usable_result_engines,
};

pub struct WebSearchTool;

impl WebSearchTool {
    pub fn new() -> Self {
        Self
    }

    /// Create an execution-scoped browser pool for headless engines.
    ///
    /// A persistent pool survives a cancelled tool future and can retain the
    /// Chrome process for the rest of the TUI session. Keeping the pool scoped
    /// to one invocation lets the cleanup guard deterministically close it on
    /// success, error, timeout, or caller cancellation.
    fn create_pool(headless_config: Option<&HeadlessConfig>) -> Option<Arc<BrowserPool>> {
        let config = headless_config?;
        let executable = config.browser_path.as_ref().map(std::path::PathBuf::from);
        let provider = match (config.backend, executable) {
            (BrowserBackend::Chrome, Some(path)) => BrowserProvider::ChromeExecutable(path),
            (BrowserBackend::Chrome, None) => BrowserProvider::DiscoveredChrome,
            (BrowserBackend::Lightpanda, Some(path)) => BrowserProvider::LightpandaExecutable(path),
            (BrowserBackend::Lightpanda, None) => BrowserProvider::DiscoveredLightpanda,
        };

        let pool_config = BrowserPoolConfig {
            max_tabs: config.max_tabs,
            headless: true,
            provider,
            proxy_url: config.proxy_url.clone(),
            launch_args: config.launch_args.clone(),
        };

        Some(Arc::new(BrowserPool::new(pool_config)))
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

struct BrowserPoolCleanup {
    pool: Option<Arc<BrowserPool>>,
}

impl BrowserPoolCleanup {
    fn new(pool: Option<Arc<BrowserPool>>) -> Self {
        Self { pool }
    }

    async fn shutdown(&mut self) {
        if let Some(pool) = self.pool.as_ref() {
            if tokio::time::timeout(std::time::Duration::from_secs(2), pool.shutdown())
                .await
                .is_err()
            {
                tracing::warn!(
                    "Headless browser cleanup exceeded the 2s foreground grace; continuing in background"
                );
                return;
            }
        }
        self.pool = None;
    }
}

impl Drop for BrowserPoolCleanup {
    fn drop(&mut self) {
        let Some(pool) = self.pool.take() else {
            return;
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move {
                    pool.shutdown().await;
                });
            }
            Err(error) => tracing::warn!(
                "Could not schedule headless browser cleanup outside a Tokio runtime: {}",
                error
            ),
        }
    }
}

fn managed_headless_config() -> Option<HeadlessConfig> {
    let status =
        crate::search_runtime::browser_status(crate::search_runtime::ManagedBrowser::Chrome);
    let path = status.available.then_some(status.path).flatten()?;
    Some(HeadlessConfig {
        backend: BrowserBackend::Chrome,
        browser_path: Some(path.to_string_lossy().into_owned()),
        ..HeadlessConfig::default()
    })
}

fn search_result_json(result: &SearchResult) -> serde_json::Value {
    let engines = sorted_search_engines(result);
    let safe_url = safe_search_result_url(result);
    let safe_title = sanitize_http_urls(&result.title);
    let safe_content = sanitize_http_urls(&result.content);
    serde_json::json!({
        "title": safe_title,
        "url": safe_url,
        "content": safe_content,
        "engines": engines,
        "score": result.score,
        "published_date": result.published_date,
    })
}

fn safe_search_result_url(result: &SearchResult) -> String {
    let Some(url) = super::safe_http_source_url(&result.url) else {
        return String::new();
    };
    let Ok(parsed) = reqwest::Url::parse(&url) else {
        return String::new();
    };
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    let search_navigation = matches!(
        host.as_str(),
        "search.brave.com" | "duckduckgo.com" | "www.sogou.com" | "www.so.com"
    ) || ((host == "google.com" || host.ends_with(".google.com"))
        && parsed.path().starts_with("/search"));
    if search_navigation {
        String::new()
    } else {
        url
    }
}

fn sorted_search_engines(result: &SearchResult) -> Vec<&str> {
    let mut engines = result
        .engines
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    engines.sort_unstable();
    engines
}

fn text_search_result(index: usize, result: &SearchResult) -> String {
    let safe_url = safe_search_result_url(result);
    let safe_title = sanitize_http_urls(&result.title);
    let safe_content = sanitize_http_urls(&result.content);
    let published = result
        .published_date
        .as_deref()
        .filter(|date| !date.trim().is_empty())
        .map(|date| format!("   Published: {}\n", date.trim()))
        .unwrap_or_default();
    format!(
        "{}. {}\n   URL: {}\n{}   {}\n   (via {})\n\n",
        index + 1,
        safe_title,
        safe_url,
        published,
        safe_content,
        sorted_search_engines(result).join(", "),
    )
}

fn sanitize_http_urls(text: &str) -> String {
    static URL_RE: OnceLock<Regex> = OnceLock::new();
    let url_re = URL_RE.get_or_init(|| {
        Regex::new(r#"(?i)https?://[^\s<>"'`]+"#).expect("static search URL regex")
    });
    url_re
        .replace_all(text, |captures: &regex::Captures<'_>| {
            let mut candidate = captures[0].to_string();
            let mut suffix = String::new();
            while candidate
                .chars()
                .last()
                .is_some_and(|ch| matches!(ch, ')' | ',' | '.' | ';' | ':' | '!' | '?' | ']' | '}'))
            {
                if let Some(ch) = candidate.pop() {
                    suffix.insert(0, ch);
                }
            }
            super::safe_http_source_url(&candidate)
                .map(|safe| format!("{safe}{suffix}"))
                .unwrap_or_default()
        })
        .into_owned()
}

fn search_metrics_json(snapshot: &MetricsSnapshot) -> serde_json::Value {
    let error_counts = snapshot
        .error_counts
        .iter()
        .map(|(kind, count)| (kind.clone(), *count))
        .collect::<std::collections::BTreeMap<_, _>>();
    serde_json::json!({
        "total_requests": snapshot.total_requests(),
        "successes": snapshot.successes,
        "failures": snapshot.failures,
        "transient_failures": snapshot.transient_failures,
        "permanent_failures": snapshot.permanent_failures,
        "success_rate": snapshot.success_rate(),
        "transient_failure_rate": snapshot.transient_failure_rate(),
        "error_counts": error_counts,
        "latency_p50_ms": snapshot.latency_p50_ms,
        "latency_p95_ms": snapshot.latency_p95_ms,
        "latency_p99_ms": snapshot.latency_p99_ms,
    })
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web using multiple search engines. Aggregates results from multiple engines \
         (DuckDuckGo, Wikipedia, Brave, Bing, Sogou, 360, Google, Baidu, Bing China, etc.). \
         Supports proxy configuration for anti-crawler protection. Returns deduplicated and ranked results. \
         Google and Baidu use a headless browser; Bing China uses its HTTP RSS endpoint."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Required. The search query. Always provide this exact field name: 'query'."
                },
                "engines": {
                    "type": "array",
                    "items": {
                        "type": "string"
                    },
                    "description": "Optional. List of search engines or native providers to use. Default: [\"ddg\",\"wiki\"]. Available: anysearch (anonymous or authenticated native provider), tavily (keyless or authenticated native provider), ddg (DuckDuckGo), brave (Brave Search), bing (Bing RSS), wiki (Wikipedia), sogou (Sogou), 360 / so360 (360 Search), bing_cn (Bing China RSS), g / google (Google, headless), baidu (Baidu, headless)."
                },
                "limit": {
                    "type": "integer",
                    "description": "Optional. Maximum number of results to return. Default: 10. Maximum: 50."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Optional. Search timeout in seconds. Default: 10. Maximum: 60."
                },
                "proxy": {
                    "type": "string",
                    "description": "Optional. Proxy URL, for example http://127.0.0.1:8080 or socks5://127.0.0.1:1080."
                },
                "format": {
                    "type": "string",
                    "enum": ["text", "json"],
                    "description": "Optional. Output format. Default: text."
                }
            },
            "required": ["query"],
            "examples": [
                {
                    "query": "Rust async trait"
                },
                {
                    "query": "A3S Code GitHub",
                    "engines": ["ddg", "wiki"],
                    "limit": 5,
                    "format": "json"
                },
                {
                    "query": "最新新闻",
                    "engines": ["baidu", "bing_cn"],
                    "limit": 10
                }
            ]
        })
    }

    fn capabilities(&self, _args: &serde_json::Value) -> crate::tools::ToolCapabilities {
        crate::tools::ToolCapabilities::parallel_safe_read(8)
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        // Validate: return error on unknown fields to catch misconfiguration like `engine` vs `engines`
        if let Some(obj) = args.as_object() {
            let valid_fields = ["query", "engines", "limit", "timeout", "proxy", "format"];
            for key in obj.keys() {
                if !valid_fields.contains(&key.as_str()) {
                    return Ok(ToolOutput::error(format!(
                        "web_search: unknown parameter '{}' - did you mean 'engines'? \
                         Use 'engines' (plural) as the field name, not 'engine' (singular)",
                        key
                    )));
                }
            }
        }

        let raw_query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => return Ok(ToolOutput::error("query parameter is required")),
        };

        if raw_query.trim().is_empty() {
            return Ok(ToolOutput::error("query must not be empty"));
        }
        let query_str = sanitize_http_urls(raw_query);
        if query_str.trim().is_empty() {
            return Ok(ToolOutput::error(
                "query must not be empty after URL sanitization",
            ));
        }

        // Get configuration from context or use defaults
        let config = ctx.search_config.as_ref();
        let default_timeout = config.map(|c| c.timeout).unwrap_or(10);
        let (default_engines, default_engine_selection_source) =
            default_engine_selection(config.map(Arc::as_ref));

        let engine_selection_source = if args.get("engines").is_some() {
            "request"
        } else {
            default_engine_selection_source
        };
        let engines: Vec<&str> = args
            .get("engines")
            .and_then(|v| {
                if let Some(arr) = v.as_array() {
                    Some(arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
                } else {
                    // Handle comma-separated string like "baidu,ddg" or single engine like "baidu"
                    v.as_str().map(|s| {
                        s.split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .collect()
                    })
                }
            })
            .unwrap_or_else(|| default_engines.clone());
        let selected_engines = engines
            .iter()
            .map(|engine| engine.to_string())
            .collect::<Vec<_>>();

        // HTTP-only searches must not probe for or initialize a managed browser.
        let needs_headless = requires_headless_browser(&engines);
        let configured_headless = config.and_then(|config| config.headless.as_ref());
        let implicit_headless_config = if needs_headless {
            configured_headless
                .cloned()
                .or_else(managed_headless_config)
        } else {
            None
        };
        let headless_config = implicit_headless_config.as_ref();

        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .min(50) as usize;

        let timeout_secs = args
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(default_timeout)
            .min(60);

        let output_format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("text");

        let mut proxy_url = args
            .get("proxy")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| configured_headless.and_then(|config| config.proxy_url.clone()))
            .or_else(super::web_fetch::explicit_web_proxy_from_env);
        if proxy_url.is_none() {
            proxy_url = super::web_fetch::system_web_proxy().await;
        }

        // Get or initialize BrowserPool if needed
        let browser_pool = if needs_headless {
            Self::create_pool(headless_config)
        } else {
            None
        };
        let mut browser_cleanup = BrowserPoolCleanup::new(browser_pool.clone());

        // Build Search instance with requested engines
        let search_metrics = Arc::new(Metrics::new());
        let mut search = Search::new().with_metrics(search_metrics.clone());
        let mut setup_failures = Vec::new();

        for shortcut in &engines {
            let shortcut_str = *shortcut;

            // Check if engine is configured and get its settings
            let engine_config = config.and_then(|config| configured_engine(config, shortcut_str));

            // Skip if explicitly disabled in config
            if let Some(engine_cfg) = engine_config {
                if !engine_cfg.enabled {
                    tracing::debug!("Skipping disabled engine: {}", shortcut_str);
                    continue;
                }
            }

            // Try HTTP engine first, then headless. Native provider setup failures
            // remain structured so they enter the same fallback policy as request failures.
            match add_http_engine(&mut search, shortcut_str, proxy_url.as_deref()) {
                Ok(true) => {}
                Ok(false) => {
                    if let Some(ref pool) = browser_pool {
                        if !add_headless_engine(&mut search, shortcut_str, pool) {
                            tracing::warn!(
                                "Unknown or unavailable search engine: {}",
                                shortcut_str
                            );
                        }
                    } else {
                        tracing::warn!(
                            "Unknown or unavailable search engine: {} (headless engines require headless config)",
                            shortcut_str
                        );
                    }
                }
                Err(failure) => {
                    tracing::warn!(
                        provider = failure.provider.as_deref().unwrap_or("unknown"),
                        kind = failure.kind,
                        "Could not initialize native search provider"
                    );
                    search_metrics.record_failure(&failure.kind, failure.transient);
                    setup_failures.push(failure);
                }
            }
        }

        let fell_back_from_headless = should_fallback_from_unavailable_headless(
            search.engine_count(),
            headless_config.is_some(),
            &engines,
        );
        if fell_back_from_headless {
            let _ = add_http_engine(&mut search, "ddg", proxy_url.as_deref());
            let _ = add_http_engine(&mut search, "wiki", proxy_url.as_deref());
        }

        if should_reject_engine_selection(search.engine_count(), &setup_failures) {
            let message = format!("No valid engines found in: {:?}", engines);
            return Ok(ToolOutput::error(&message)
                .with_error_kind(ToolErrorKind::InvalidArgument { message })
                .with_metadata(serde_json::json!({
                    "status": "failed",
                    "engine_selection_source": engine_selection_source,
                    "selected_engines": &selected_engines,
                })));
        }

        let mut attempted_engine_shortcuts = selected_engines.clone();
        if fell_back_from_headless {
            attempted_engine_shortcuts.extend(["ddg".to_string(), "wiki".to_string()]);
        }
        let available_fallback_shortcuts =
            fallback_engine_shortcuts(&attempted_engine_shortcuts, config.map(Arc::as_ref));
        let total_timeout = Duration::from_secs(timeout_secs.max(1));
        let initial_timeout =
            primary_search_timeout(total_timeout, !available_fallback_shortcuts.is_empty());
        search.set_timeout(initial_timeout);

        // Configure proxy if provided
        if let Some(url) = proxy_url.as_deref() {
            // Parse proxy URL into ProxyConfig
            if let Some(config) = parse_proxy_url(url) {
                let _pool = ProxyPool::with_proxies(vec![config]);
                // Note: proxy is applied per-engine fetcher, not globally
                tracing::debug!("Proxy configuration provided but not yet applied to engines");
            }
        }

        let search_started = Instant::now();
        let search_deadline = search_started + total_timeout;
        let initial_outer_timeout = initial_timeout
            .saturating_add(Duration::from_millis(250))
            .min(total_timeout);
        let search_result = if search.engine_count() == 0 {
            None
        } else {
            Some(
                tokio::time::timeout(
                    initial_outer_timeout,
                    search.search(SearchQuery::new(&query_str)),
                )
                .await,
            )
        };
        browser_cleanup.shutdown().await;
        let mut search_results = SearchResults::new();
        for failure in setup_failures {
            search_results.add_failure(failure);
        }
        match search_result {
            Some(Ok(Ok(results))) => merge_search_results(&mut search_results, &results),
            Some(Ok(Err(error))) => {
                search_results.add_failure(
                    EngineFailure::new("Selected search engines", error.kind(), error.to_string())
                        .with_transient(error.is_transient()),
                );
            }
            Some(Err(_)) => {
                search_results.add_failure(
                    EngineFailure::new(
                        "Selected search engines",
                        "timeout",
                        "initial search stage timed out",
                    )
                    .with_transient(true),
                );
            }
            None => {}
        }

        let mut notices = Vec::new();
        let mut search_fallback = None;
        let initial_failures = search_results.failures().to_vec();
        let initial_failure_summary = failure_summary(&initial_failures);
        let initial_failure_metadata = failure_metadata(&initial_failures);
        let initial_result_engines = usable_result_engines(&search_results);
        let has_usable_result = !initial_result_engines.is_empty();
        let fallback_trigger = if !initial_failures.is_empty() {
            "engine_failure"
        } else if fell_back_from_headless {
            "headless_unavailable"
        } else {
            "empty_results"
        };
        let degradation_cause = if !initial_failure_summary.is_empty() {
            initial_failure_summary.clone()
        } else if fell_back_from_headless {
            "the selected headless engines were unavailable".to_string()
        } else {
            "the selected search engines returned no usable results".to_string()
        };

        if has_usable_result && (!initial_failures.is_empty() || fell_back_from_headless) {
            notices.push(format!(
                "Search degraded because {degradation_cause}; continued automatically with the engines that returned results."
            ));
            search_fallback = Some(serde_json::json!({
                "trigger": fallback_trigger,
                "mode": "selected_engines",
                "attempted": true,
                "engines": initial_result_engines,
                "successful": true,
                "failures": initial_failure_metadata,
            }));
        } else if !has_usable_result {
            let mut fallback_search = Search::new().with_metrics(Arc::clone(&search_metrics));
            let mut fallback_engines = Vec::new();
            for shortcut in available_fallback_shortcuts {
                match add_http_engine(&mut fallback_search, shortcut, proxy_url.as_deref()) {
                    Ok(true) => fallback_engines.push(shortcut),
                    Ok(false) => {}
                    Err(failure) => {
                        search_metrics.record_failure(&failure.kind, failure.transient);
                        search_results.add_failure(failure);
                    }
                }
            }

            let remaining = search_deadline.saturating_duration_since(Instant::now());
            if fallback_engines.is_empty() {
                notices.push(format!(
                    "Search degraded because {degradation_cause}; no additional fallback engine was available."
                ));
                search_fallback = Some(serde_json::json!({
                    "trigger": fallback_trigger,
                    "mode": "unavailable",
                    "attempted": false,
                    "engines": fallback_engines,
                    "successful": false,
                    "failures": initial_failure_metadata,
                }));
            } else if remaining.is_zero() {
                search_results.add_failure(
                    EngineFailure::new(
                        "Fallback search",
                        "timeout",
                        "search timeout was exhausted before fallback could start",
                    )
                    .with_transient(true),
                );
                notices.push(format!(
                    "Search degraded because {degradation_cause}; automatic fallback could not start because the search timeout was exhausted."
                ));
                search_fallback = Some(serde_json::json!({
                    "trigger": fallback_trigger,
                    "mode": "additional_engines",
                    "attempted": false,
                    "engines": fallback_engines,
                    "successful": false,
                    "failures": initial_failure_metadata,
                }));
            } else {
                let fallback_names = fallback_engine_names(&fallback_engines);
                let fallback_engine_timeout = remaining
                    .saturating_sub(Duration::from_millis(100))
                    .max(Duration::from_millis(1));
                fallback_search.set_timeout(fallback_engine_timeout);
                let fallback_result = tokio::time::timeout(
                    remaining,
                    fallback_search.search(SearchQuery::new(&query_str)),
                )
                .await;
                match fallback_result {
                    Ok(Ok(results)) => merge_search_results(&mut search_results, &results),
                    Ok(Err(error)) => search_results.add_failure(
                        EngineFailure::new("Fallback search", error.kind(), error.to_string())
                            .with_transient(error.is_transient()),
                    ),
                    Err(_) => search_results.add_failure(
                        EngineFailure::new(
                            "Fallback search",
                            "timeout",
                            "fallback search timed out",
                        )
                        .with_transient(true),
                    ),
                }
                let successful = !usable_result_engines(&search_results).is_empty();
                notices.push(if successful {
                    format!(
                        "Search degraded because {degradation_cause}; automatically fell back to {fallback_names}."
                    )
                } else {
                    format!(
                        "Search degraded because {degradation_cause}; automatic fallback to {fallback_names} returned no usable results."
                    )
                });
                search_fallback = Some(serde_json::json!({
                    "trigger": fallback_trigger,
                    "mode": "additional_engines",
                    "attempted": true,
                    "engines": fallback_engines,
                    "successful": successful,
                    "failures": initial_failure_metadata,
                }));
            }
        }
        search_results
            .set_duration(u64::try_from(search_started.elapsed().as_millis()).unwrap_or(u64::MAX));
        let metrics = search_metrics.snapshot().await;
        let metrics_json = search_metrics_json(&metrics);

        let items = search_results.items();
        let results: Vec<_> = items
            .iter()
            .filter(|result| !safe_search_result_url(result).is_empty())
            .take(limit)
            .collect();

        // Report engine errors if any
        let errors = search_results.errors();
        let engine_errors = errors
            .iter()
            .map(|(engine, error)| {
                serde_json::json!({
                    "engine": engine,
                    "message": crate::text::truncate_utf8(
                        &sanitize_http_urls(&error.to_string()),
                        512,
                    ),
                })
            })
            .collect::<Vec<_>>();
        let engine_failures = failure_metadata(search_results.failures());
        let error_note = if errors.is_empty() {
            String::new()
        } else {
            let mut note = String::from("\nEngine errors:\n");
            for (engine, error) in errors {
                note.push_str(&format!("  - {}: {}\n", engine, error));
            }
            note
        };
        let notice_note = text_notice_note(&notices);

        if results.is_empty() {
            let metadata = serde_json::json!({
                "status": if errors.is_empty() { "complete" } else { "failed" },
                "engine_selection_source": engine_selection_source,
                "selected_engines": &selected_engines,
                "engine_fallback": fell_back_from_headless.then_some("ddg,wiki"),
                "notices": &notices,
                "search_fallback": search_fallback.as_ref(),
                "search_metrics": metrics_json,
                "engine_errors": engine_errors,
                "engine_failures": engine_failures,
            });
            let message = format!(
                "No results found for query: \"{}\"{}{}",
                query_str, notice_note, error_note
            );
            if errors.is_empty() {
                return Ok(ToolOutput::success(message).with_metadata(metadata));
            }
            let mut output = ToolOutput::error(message).with_metadata(metadata);
            if let Some(error_kind) =
                tool_error_kind_for_failures(search_results.failures(), total_timeout)
            {
                output = output.with_error_kind(error_kind);
            }
            return Ok(output);
        }

        let source_anchors = results
            .iter()
            .map(|result| safe_search_result_url(result))
            .filter(|url| !url.is_empty())
            .collect::<Vec<_>>();

        let output = if output_format == "json" {
            let json_results: Vec<serde_json::Value> = results
                .iter()
                .map(|result| search_result_json(result))
                .collect();
            serde_json::to_string_pretty(&json_results).unwrap_or_default()
        } else {
            let mut text = format!(
                "Search results for \"{}\" ({} results, {}ms):\n\n",
                query_str,
                results.len(),
                search_results.duration_ms,
            );
            for (i, result) in results.iter().enumerate() {
                text.push_str(&text_search_result(i, result));
            }
            if !notice_note.is_empty() {
                text.push_str(&notice_note);
            }
            if !error_note.is_empty() {
                text.push_str(&error_note);
            }
            text
        };

        Ok(
            ToolOutput::success(output).with_metadata(serde_json::json!({
                "status": if errors.is_empty() { "complete" } else { "partial" },
                "engine_selection_source": engine_selection_source,
                "selected_engines": &selected_engines,
                "source_anchors": source_anchors,
                "engine_fallback": fell_back_from_headless.then_some("ddg,wiki"),
                "notices": &notices,
                "search_fallback": search_fallback.as_ref(),
                "search_metrics": metrics_json,
                "engine_errors": engine_errors,
                "engine_failures": engine_failures,
            })),
        )
    }
}

/// Parse a proxy URL string like "http://host:port" into a ProxyConfig
fn parse_proxy_url(url: &str) -> Option<ProxyConfig> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    // Parse scheme
    let (scheme, rest) = if let Some(rest) = url.strip_prefix("socks5://") {
        ("socks5", rest)
    } else if let Some(rest) = url.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        ("http", rest)
    } else {
        ("http", url)
    };

    // Parse host:port
    let (host, port) = {
        let colon_pos = rest.rfind(':')?;
        let host = &rest[..colon_pos];
        let port_str = &rest[colon_pos + 1..];
        match port_str.parse::<u16>() {
            Ok(p) => (host, p),
            Err(_) => return None,
        }
    };

    let mut config = ProxyConfig::new(host, port);
    config = match scheme {
        "socks5" => config.with_protocol(a3s_search::proxy::ProxyProtocol::Socks5),
        "https" => config.with_protocol(a3s_search::proxy::ProxyProtocol::Https),
        _ => config, // default is Http
    };

    Some(config)
}

#[cfg(test)]
#[path = "web_search/tests.rs"]
mod tests;
