//! Web search tool - Search the web via a3s-search

use crate::config::{BrowserBackend, HeadlessConfig};
use crate::tools::types::{Tool, ToolContext, ToolErrorKind, ToolOutput};
use a3s_search::a3s_use_browser::{BrowserPool, BrowserPoolConfig, BrowserProvider};
use a3s_search::engines::{
    Baidu, BingChina, BingParser, BraveParser, DuckDuckGoParser, Google, So360Parser, SogouParser,
    Wikipedia,
};
use a3s_search::providers::BuiltinProvider;
use a3s_search::proxy::{ProxyConfig, ProxyPool};
use a3s_search::WaitStrategy;
use a3s_search::{
    BrowserFetcher, HtmlEngine, HttpFetcher, Metrics, MetricsSnapshot, Search, SearchQuery,
    SearchResult,
};
use anyhow::Result;
use async_trait::async_trait;
use regex::Regex;
use std::sync::Arc;
use std::sync::OnceLock;

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

fn should_fallback_from_unavailable_headless(
    engine_count: usize,
    has_headless_config: bool,
    engines: &[&str],
) -> bool {
    engine_count == 0
        && !has_headless_config
        && !engines.is_empty()
        && engines
            .iter()
            .all(|engine| matches!(engine.trim(), "g" | "google" | "baidu"))
}

fn requires_headless_browser(engines: &[&str]) -> bool {
    engines
        .iter()
        .any(|engine| matches!(engine.trim(), "g" | "google" | "baidu"))
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

/// Add an HTTP engine by shortcut
fn add_http_engine(search: &mut Search, shortcut: &str, proxy_url: Option<&str>) -> bool {
    let fetcher = || {
        proxy_url
            .and_then(|proxy| HttpFetcher::with_proxy(proxy).ok())
            .unwrap_or_default()
    };
    match shortcut.trim() {
        "ddg" => {
            search.add_engine(HtmlEngine::with_fetcher(
                DuckDuckGoParser,
                Arc::new(fetcher()),
            ));
            true
        }
        "brave" => {
            search.add_engine(HtmlEngine::with_fetcher(BraveParser, Arc::new(fetcher())));
            true
        }
        "bing" => {
            search.add_engine(HtmlEngine::with_fetcher(BingParser, Arc::new(fetcher())));
            true
        }
        "wiki" => {
            search.add_engine(Wikipedia::with_http_fetcher(fetcher()));
            true
        }
        "sogou" => {
            search.add_engine(HtmlEngine::with_fetcher(SogouParser, Arc::new(fetcher())));
            true
        }
        "360" | "so360" => {
            search.add_engine(HtmlEngine::with_fetcher(So360Parser, Arc::new(fetcher())));
            true
        }
        "bing_cn" => {
            search.add_engine(BingChina::new(Arc::new(fetcher())));
            true
        }
        "anysearch" | "tavily" => {
            let provider = BuiltinProvider::from_id(shortcut.trim())
                .expect("matched built-in search provider");
            match provider.create_engine() {
                Ok(engine) => {
                    search.add_engine(engine);
                    true
                }
                Err(error) => {
                    tracing::warn!(
                        provider = shortcut.trim(),
                        %error,
                        "Could not initialize native search provider"
                    );
                    false
                }
            }
        }
        _ => false,
    }
}

fn default_engine_selection(
    config: Option<&crate::config::SearchConfig>,
) -> (Vec<&str>, &'static str) {
    match config {
        Some(config) if !config.engines.is_empty() => (
            config
                .engines
                .iter()
                .filter(|(_, engine)| engine.enabled)
                .map(|(name, _)| name.as_str())
                .collect(),
            "config",
        ),
        _ => (vec!["ddg", "wiki"], "builtin_default"),
    }
}

/// Add a headless engine using BrowserPool.
fn add_headless_engine(search: &mut Search, shortcut: &str, pool: &Arc<BrowserPool>) -> bool {
    match shortcut.trim() {
        "g" | "google" => {
            let fetcher = BrowserFetcher::new(Arc::clone(pool)).with_wait(WaitStrategy::Selector {
                css: "div.g".to_string(),
                timeout_ms: 5000,
            });
            search.add_engine(Google::new(Arc::new(fetcher)));
            true
        }
        "baidu" => {
            let fetcher = BrowserFetcher::new(Arc::clone(pool)).with_wait(WaitStrategy::Selector {
                css: "div.c-container".to_string(),
                timeout_ms: 5000,
            });
            search.add_engine(Baidu::new(Arc::new(fetcher)));
            true
        }
        _ => false,
    }
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
        search.set_timeout(std::time::Duration::from_secs(timeout_secs));

        for shortcut in &engines {
            let shortcut_str = *shortcut;

            // Check if engine is configured and get its settings
            let engine_config = config.and_then(|c| c.engines.get(shortcut_str));

            // Skip if explicitly disabled in config
            if let Some(engine_cfg) = engine_config {
                if !engine_cfg.enabled {
                    tracing::debug!("Skipping disabled engine: {}", shortcut_str);
                    continue;
                }
            }

            // Try HTTP engine first, then headless
            if !add_http_engine(&mut search, shortcut_str, proxy_url.as_deref()) {
                if let Some(ref pool) = browser_pool {
                    if !add_headless_engine(&mut search, shortcut_str, pool) {
                        tracing::warn!("Unknown or unavailable search engine: {}", shortcut_str);
                    }
                } else {
                    tracing::warn!(
                        "Unknown or unavailable search engine: {} (headless engines require headless config)",
                        shortcut_str
                    );
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

        if search.engine_count() == 0 {
            let message = format!("No valid engines found in: {:?}", engines);
            return Ok(ToolOutput::error(&message)
                .with_error_kind(ToolErrorKind::InvalidArgument { message })
                .with_metadata(serde_json::json!({
                    "status": "failed",
                    "engine_selection_source": engine_selection_source,
                    "selected_engines": &selected_engines,
                })));
        }

        // Configure proxy if provided
        if let Some(url) = proxy_url.as_deref() {
            // Parse proxy URL into ProxyConfig
            if let Some(config) = parse_proxy_url(url) {
                let _pool = ProxyPool::with_proxies(vec![config]);
                // Note: proxy is applied per-engine fetcher, not globally
                tracing::debug!("Proxy configuration provided but not yet applied to engines");
            }
        }

        let query = SearchQuery::new(&query_str);

        let search_result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs.max(1)),
            search.search(query),
        )
        .await;
        browser_cleanup.shutdown().await;
        let search_results = match search_result {
            Ok(Ok(results)) => results,
            Ok(Err(e)) => {
                let metrics = search_metrics.snapshot().await;
                let message = e.to_string();
                let output = ToolOutput::error(format!("Search failed: {message}")).with_metadata(
                    serde_json::json!({
                        "status": "failed",
                        "engine_selection_source": engine_selection_source,
                        "selected_engines": &selected_engines,
                        "search_metrics": search_metrics_json(&metrics),
                    }),
                );
                return Ok(if looks_rate_limited(&message) {
                    output.with_error_kind(ToolErrorKind::RateLimited {
                        retry_after_ms: None,
                    })
                } else {
                    output
                });
            }
            Err(_) => {
                let metrics = search_metrics.snapshot().await;
                return Ok(ToolOutput::error(format!(
                    "Search timed out after {timeout_secs} seconds"
                ))
                .with_error_kind(ToolErrorKind::Timeout {
                    op: "web_search".to_string(),
                    duration_ms: timeout_secs.saturating_mul(1_000),
                })
                .with_metadata(serde_json::json!({
                    "status": "failed",
                    "engine_selection_source": engine_selection_source,
                    "selected_engines": &selected_engines,
                    "search_metrics": search_metrics_json(&metrics),
                })));
            }
        };
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
        let error_note = if errors.is_empty() {
            String::new()
        } else {
            let mut note = String::from("\nEngine errors:\n");
            for (engine, error) in errors {
                note.push_str(&format!("  - {}: {}\n", engine, error));
            }
            note
        };

        if results.is_empty() {
            let metadata = serde_json::json!({
                "status": if errors.is_empty() { "complete" } else { "failed" },
                "engine_selection_source": engine_selection_source,
                "selected_engines": &selected_engines,
                "engine_fallback": fell_back_from_headless.then_some("ddg,wiki"),
                "search_metrics": metrics_json,
                "engine_errors": engine_errors,
            });
            let message = format!(
                "No results found for query: \"{}\"{}",
                query_str, error_note
            );
            if errors.is_empty() {
                return Ok(ToolOutput::success(message).with_metadata(metadata));
            }
            let output = ToolOutput::error(message).with_metadata(metadata);
            return Ok(
                if errors.iter().all(|(_, error)| looks_rate_limited(error)) {
                    output.with_error_kind(ToolErrorKind::RateLimited {
                        retry_after_ms: None,
                    })
                } else {
                    output
                },
            );
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
                "search_metrics": metrics_json,
                "engine_errors": engine_errors,
            })),
        )
    }
}

fn looks_rate_limited(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("rate limit")
        || message.contains("too many requests")
        || message.contains("http 429")
        || message.contains("status 429")
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
