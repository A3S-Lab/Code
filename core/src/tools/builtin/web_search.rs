//! Web search tool - Search the web via a3s-search

mod engines;
mod fallback;

use crate::config::{BrowserBackend, HeadlessConfig};
use crate::tools::types::{Tool, ToolContext, ToolErrorKind, ToolOutput};
use a3s_search::a3s_use_browser::{BrowserPool, BrowserPoolConfig, BrowserProvider};
use a3s_search::proxy::ProxyConfig;
use a3s_search::{
    EngineFailure, Metrics, MetricsSnapshot, Search, SearchCascade, SearchCoalescerSnapshot,
    SearchQualityFloor, SearchQuery, SearchResult, SearchResults,
};
use anyhow::Result;
use async_trait::async_trait;
use regex::Regex;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const MIN_FULL_TEXT_BYTES: usize = 512;
const MAX_FULL_TEXT_BYTES: usize = 32 * 1024;
const MAX_JSON_TITLE_BYTES: usize = 2 * 1024;
const MAX_JSON_CONTENT_BYTES: usize = 4 * 1024;
const JSON_OUTPUT_RESERVE_BYTES: usize = 4 * 1024;
const MAX_JSON_OUTPUT_BYTES: usize = crate::tools::MAX_OUTPUT_SIZE - JSON_OUTPUT_RESERVE_BYTES;

use engines::{add_headless_engine, add_http_engine, default_engine_selection};
use fallback::{
    failure_metadata, failure_summary, outcome_metadata, text_notice_note, tier_timeout,
    tiered_engine_plan, tool_error_kind_for_failures, usable_result_count,
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
    managed_headless_config_from_statuses(&crate::search_runtime::browser_statuses())
}

fn managed_headless_config_from_statuses(
    statuses: &[crate::search_runtime::BrowserRuntimeStatus],
) -> Option<HeadlessConfig> {
    let status = statuses
        .iter()
        .find(|status| status.available && status.path.is_some())?;
    let backend = match status.browser {
        crate::search_runtime::ManagedBrowser::Chrome => BrowserBackend::Chrome,
        crate::search_runtime::ManagedBrowser::Lightpanda => BrowserBackend::Lightpanda,
    };
    Some(HeadlessConfig {
        backend,
        browser_path: status
            .path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        ..HeadlessConfig::default()
    })
}

fn effective_headless_config(
    configured: Option<&HeadlessConfig>,
    proxy_url: Option<&str>,
) -> Option<HeadlessConfig> {
    let mut config = configured.cloned().or_else(managed_headless_config)?;
    if let Some(proxy_url) = proxy_url {
        config.proxy_url = Some(proxy_url.to_string());
    }
    Some(config)
}

fn search_result_json(result: &SearchResult, full_text_bytes: Option<usize>) -> serde_json::Value {
    let engines = sorted_search_engines(result);
    let safe_url = safe_search_result_url(result);
    let safe_title = sanitize_http_urls(&result.title);
    let safe_title = crate::text::truncate_utf8(&safe_title, MAX_JSON_TITLE_BYTES);
    let safe_content = sanitize_http_urls(&result.content);
    let safe_content = crate::text::truncate_utf8(&safe_content, MAX_JSON_CONTENT_BYTES);
    let mut value = serde_json::json!({
        "title": safe_title,
        "url": safe_url,
        "content": safe_content,
        "engines": engines,
        "score": result.score,
        "query_match_score": result.query_match_score,
        "published_date": result.published_date,
    });
    if let (Some(maximum), Some(full_text)) = (full_text_bytes, result.full_text.as_deref()) {
        let sanitized = sanitize_http_urls(full_text);
        let bounded = crate::text::truncate_utf8(&sanitized, maximum);
        if !bounded.trim().is_empty() {
            value["full_text"] = serde_json::Value::String(bounded.to_string());
        }
    }
    value
}

fn bounded_json_search_results(
    results: &[&SearchResult],
    full_text_bytes: Option<usize>,
) -> Vec<serde_json::Value> {
    let mut bounded = Vec::with_capacity(results.len());
    for result in results {
        bounded.push(search_result_json(result, full_text_bytes));
        if serde_json::to_vec(&bounded).is_ok_and(|encoded| encoded.len() <= MAX_JSON_OUTPUT_BYTES)
        {
            continue;
        }
        bounded.pop();
        break;
    }
    bounded
}

fn json_search_payload(
    results: Vec<serde_json::Value>,
    quality_met: bool,
    quality: &a3s_search::SearchQuality,
    quality_floor: &SearchQualityFloor,
) -> serde_json::Value {
    if quality_met {
        serde_json::Value::Array(results)
    } else {
        serde_json::json!({
            "status": "quality_not_met",
            "message": "Search exhausted the available tiers without meeting the generic quality floor; reformulate the query before treating these candidates as evidence.",
            "search_quality": quality,
            "search_quality_floor": quality_floor,
            "results": results,
        })
    }
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

fn search_coalescer_json(snapshot: &SearchCoalescerSnapshot) -> serde_json::Value {
    serde_json::json!({
        "max_in_flight": snapshot.max_in_flight,
        "in_flight": snapshot.in_flight,
        "leader_requests": snapshot.leader_requests,
        "shared_requests": snapshot.shared_requests,
        "bypassed_requests": snapshot.bypassed_requests,
        "abandoned_requests": snapshot.abandoned_requests,
    })
}

fn tier_search(ctx: &ToolContext, metrics: Arc<Metrics>) -> Search {
    Search::new()
        .with_metrics(metrics)
        .with_circuit_breaker(ctx.search_circuit_breaker())
        .with_bulkhead(ctx.search_bulkhead())
        .with_request_coalescer(ctx.search_request_coalescer())
}

fn search_error_failure(engine: &str, error: &a3s_search::SearchError) -> EngineFailure {
    let mut failure = EngineFailure::new(engine, error.kind(), error.to_string())
        .with_transient(error.is_transient());
    if let Some(retry_after_seconds) = error.retry_after_seconds() {
        failure = failure.with_retry_after(retry_after_seconds);
    }
    failure
}

async fn execute_search_stage(
    mut search: Search,
    mut results: SearchResults,
    query: &str,
    stage_name: &str,
    deadline: Instant,
    remaining_tiers: usize,
) -> SearchResults {
    if search.engine_count() == 0 {
        return results;
    }

    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        results.add_failure(
            EngineFailure::new(
                stage_name,
                "timeout",
                "search deadline was exhausted before this tier could start",
            )
            .with_transient(true),
        );
        return results;
    }

    let stage_budget = tier_timeout(remaining, remaining_tiers);
    let engine_budget = stage_budget
        .saturating_sub(Duration::from_millis(100))
        .max(Duration::from_millis(1));
    search.set_timeout(engine_budget);
    match tokio::time::timeout(stage_budget, search.search(SearchQuery::new(query))).await {
        Ok(Ok(stage_results)) => results.merge(stage_results),
        Ok(Err(error)) => results.add_failure(search_error_failure(stage_name, &error)),
        Err(_) => results.add_failure(
            EngineFailure::new(stage_name, "timeout", "search tier timed out").with_transient(true),
        ),
    }
    results
        .items_mut()
        .retain(|result| !safe_search_result_url(result).is_empty());
    results.count = results.items().len();
    results
}

struct SearchStageContext<'a> {
    tool_context: &'a ToolContext,
    query: &'a str,
    proxy_url: Option<&'a str>,
    metrics: &'a Arc<Metrics>,
    deadline: Instant,
}

async fn execute_network_stage(
    context: &SearchStageContext<'_>,
    shortcuts: &[String],
    stage_name: &str,
    remaining_tiers: usize,
) -> SearchResults {
    let mut search = tier_search(context.tool_context, Arc::clone(context.metrics));
    let mut results = SearchResults::new();
    for shortcut in shortcuts {
        match add_http_engine(&mut search, shortcut, context.proxy_url) {
            Ok(true) => {}
            Ok(false) => results.add_failure(EngineFailure::new(
                shortcut,
                "unsupported_engine",
                "engine is not available in this search tier",
            )),
            Err(failure) => {
                context
                    .metrics
                    .record_failure(&failure.kind, failure.transient);
                results.add_failure(failure);
            }
        }
    }
    execute_search_stage(
        search,
        results,
        context.query,
        stage_name,
        context.deadline,
        remaining_tiers,
    )
    .await
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web through a quality-gated cascade: native APIs first, HTTP/RSS engines only \
         when needed, and headless Google/Baidu only when earlier tiers remain insufficient. \
         Unavailable engines are skipped through session-scoped circuit state, and all executed tiers \
         are deduplicated and ranked together. An explicit engines list runs only those requested tiers. \
         Supports proxy configuration for conventional and headless search transports."
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
                    "description": "Optional. List of search engines or native providers to use. Without explicit configuration, all built-in providers that advertise anonymous access are combined with the public HTTP defaults. Available: anysearch (anonymous or authenticated native provider), tavily (keyless or authenticated native provider), ddg (DuckDuckGo), brave (Brave Search), bing (Bing RSS), wiki (Wikipedia), sogou (Sogou), 360 / so360 (360 Search), bing_cn (Bing China RSS), g / google (Google, headless), baidu (Baidu, headless)."
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
                },
                "full_text_bytes": {
                    "type": "integer",
                    "minimum": MIN_FULL_TEXT_BYTES,
                    "maximum": MAX_FULL_TEXT_BYTES,
                    "description": "Optional. For JSON output, include at most this many UTF-8 bytes of provider-returned full source text per result. Omitted by default."
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
            let valid_fields = [
                "query",
                "engines",
                "limit",
                "timeout",
                "proxy",
                "format",
                "full_text_bytes",
            ];
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
        let total_timeout = Duration::from_secs(timeout_secs.max(1));
        let search_started = Instant::now();
        let search_deadline = search_started + total_timeout;

        let output_format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("text");
        let full_text_bytes = match args.get("full_text_bytes") {
            None => None,
            Some(value) => match value.as_u64().and_then(|value| usize::try_from(value).ok()) {
                Some(value) if (MIN_FULL_TEXT_BYTES..=MAX_FULL_TEXT_BYTES).contains(&value) => {
                    Some(value)
                }
                _ => {
                    return Ok(ToolOutput::error(format!(
                        "full_text_bytes must be an integer between {MIN_FULL_TEXT_BYTES} and {MAX_FULL_TEXT_BYTES}"
                    )))
                }
            },
        };

        let mut proxy_url = args
            .get("proxy")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                config
                    .and_then(|config| config.headless.as_ref())
                    .and_then(|config| config.proxy_url.clone())
            })
            .or_else(super::web_fetch::explicit_web_proxy_from_env);
        if proxy_url.is_none() {
            let remaining = search_deadline.saturating_duration_since(Instant::now());
            if !remaining.is_zero() {
                proxy_url = tokio::time::timeout(remaining, super::web_fetch::system_web_proxy())
                    .await
                    .ok()
                    .flatten();
            }
        }
        if let Some(proxy) = proxy_url.as_deref() {
            if parse_proxy_url(proxy).is_none() {
                let message = "proxy must include a supported scheme, host, and port".to_string();
                return Ok(ToolOutput::error(&message)
                    .with_error_kind(ToolErrorKind::InvalidArgument { message }));
            }
        }

        let search_metrics = Arc::new(Metrics::new());
        let automatic_fallback = engine_selection_source != "request";
        let tier_plan = tiered_engine_plan(&engines, config.map(Arc::as_ref), automatic_fallback);
        if tier_plan.is_empty() {
            let message = format!("No valid engines found in: {:?}", engines);
            return Ok(ToolOutput::error(&message)
                .with_error_kind(ToolErrorKind::InvalidArgument { message })
                .with_metadata(serde_json::json!({
                    "status": "failed",
                    "engine_selection_source": engine_selection_source,
                    "selected_engines": &selected_engines,
                })));
        }

        let quality_floor = SearchQualityFloor::for_limit(limit);
        let mut cascade = SearchCascade::new(SearchQuery::new(&query_str), quality_floor);
        let stage_context = SearchStageContext {
            tool_context: ctx,
            query: &query_str,
            proxy_url: proxy_url.as_deref(),
            metrics: &search_metrics,
            deadline: search_deadline,
        };

        if !tier_plan.api.is_empty() {
            let remaining_tiers = usize::from(!tier_plan.http.is_empty())
                + usize::from(!tier_plan.headless.is_empty());
            cascade
                .run_tier_if_needed("api", || {
                    execute_network_stage(
                        &stage_context,
                        &tier_plan.api,
                        "API search tier",
                        remaining_tiers,
                    )
                })
                .await;
        }

        if !tier_plan.http.is_empty() {
            cascade
                .run_tier_if_needed("http", || {
                    execute_network_stage(
                        &stage_context,
                        &tier_plan.http,
                        "HTTP search tier",
                        usize::from(!tier_plan.headless.is_empty()),
                    )
                })
                .await;
        }

        if !tier_plan.headless.is_empty() {
            cascade
                .run_tier_if_needed("headless", || async {
                    let mut results = SearchResults::new();
                    if search_deadline
                        .saturating_duration_since(Instant::now())
                        .is_zero()
                    {
                        results.add_failure(
                            EngineFailure::new(
                                "Headless search tier",
                                "timeout",
                                "search deadline was exhausted before the headless tier could start",
                            )
                            .with_transient(true),
                        );
                    } else {
                        let headless_config = effective_headless_config(
                            config.and_then(|config| config.headless.as_ref()),
                            proxy_url.as_deref(),
                        );
                        match Self::create_pool(headless_config.as_ref()) {
                            Some(pool) => {
                                let mut cleanup =
                                    BrowserPoolCleanup::new(Some(Arc::clone(&pool)));
                                let mut search = tier_search(ctx, Arc::clone(&search_metrics));
                                for shortcut in &tier_plan.headless {
                                    if !add_headless_engine(
                                        &mut search,
                                        shortcut,
                                        &pool,
                                        &ctx.search_retry_budget(),
                                    ) {
                                        results.add_failure(EngineFailure::new(
                                            shortcut,
                                            "unsupported_engine",
                                            "headless engine is not available",
                                        ));
                                    }
                                }
                                results = execute_search_stage(
                                    search,
                                    results,
                                    &query_str,
                                    "Headless search tier",
                                    search_deadline,
                                    0,
                                )
                                .await;
                                cleanup.shutdown().await;
                            }
                            None => results.add_failure(EngineFailure::new(
                                "Headless search tier",
                                "headless_unavailable",
                                "no managed headless browser is available",
                            )),
                        }
                    }
                    results
                })
                .await;
        }

        let final_quality = cascade.quality();
        let quality_met = quality_floor.is_met(&final_quality);
        let tier_reports = cascade.reports().to_vec();
        let mut search_results = cascade.into_results();
        search_results
            .set_duration(u64::try_from(search_started.elapsed().as_millis()).unwrap_or(u64::MAX));

        let mut notices = Vec::new();
        let failure_summary = failure_summary(search_results.failures());
        if usable_result_count(&search_results) > 0 && !failure_summary.is_empty() {
            notices.push(format!(
                "Search completed with degraded engines: {failure_summary}."
            ));
        }
        if !quality_met {
            notices.push(
                "Search exhausted the available tiers before the result quality floor was met."
                    .to_string(),
            );
        }
        let executed_engines = search_results
            .outcomes()
            .iter()
            .map(|outcome| outcome.shortcut.clone())
            .collect::<Vec<_>>();
        let search_fallback = serde_json::json!({
            "trigger": "quality_floor",
            "mode": "tiered",
            "attempted": tier_reports.len() > 1,
            "engines": executed_engines,
            "successful": quality_met,
            "failures": failure_metadata(search_results.failures()),
        });
        let metrics = search_metrics.snapshot().await;
        let metrics_json = search_metrics_json(&metrics);
        let coalescer_json = search_coalescer_json(&ctx.search_request_coalescer().snapshot());

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
                "status": if quality_met && errors.is_empty() { "complete" } else { "failed" },
                "engine_selection_source": engine_selection_source,
                "selected_engines": &selected_engines,
                "engine_fallback": (tier_reports.len() > 1).then_some("quality_gated_tiers"),
                "notices": &notices,
                "search_fallback": &search_fallback,
                "search_quality": &final_quality,
                "search_quality_floor": &quality_floor,
                "search_tiers": &tier_reports,
                "engine_outcomes": outcome_metadata(search_results.outcomes()),
                "search_metrics": metrics_json,
                "search_coalescing": coalescer_json,
                "engine_errors": engine_errors,
                "engine_failures": engine_failures,
            });
            let message = format!(
                "No results found for query: \"{}\"{}{}",
                query_str, notice_note, error_note
            );
            if quality_met && errors.is_empty() {
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

        let (output, source_anchors, returned_result_count) = if output_format == "json" {
            let json_results = bounded_json_search_results(&results, full_text_bytes);
            let returned_result_count = json_results.len();
            if returned_result_count < results.len() {
                notices.push(format!(
                    "JSON output retained {returned_result_count} of {} usable results within the bounded tool transport; use a narrower query or lower limit to retrieve additional results.",
                    results.len()
                ));
            }
            let source_anchors = json_results
                .iter()
                .filter_map(|result| result.get("url").and_then(serde_json::Value::as_str))
                .map(str::to_string)
                .collect::<Vec<_>>();
            (
                serde_json::to_string_pretty(&json_search_payload(
                    json_results,
                    quality_met,
                    &final_quality,
                    &quality_floor,
                ))
                .unwrap_or_default(),
                source_anchors,
                returned_result_count,
            )
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
            let source_anchors = results
                .iter()
                .map(|result| safe_search_result_url(result))
                .filter(|url| !url.is_empty())
                .collect::<Vec<_>>();
            (text, source_anchors, results.len())
        };

        let tool_output = if quality_met {
            ToolOutput::success(output)
        } else {
            ToolOutput::error(output)
        };
        Ok(
            tool_output.with_metadata(serde_json::json!({
                "status": if !quality_met { "failed" } else if errors.is_empty() { "complete" } else { "partial" },
                "engine_selection_source": engine_selection_source,
                "selected_engines": &selected_engines,
                "source_anchors": source_anchors,
                "engine_fallback": (tier_reports.len() > 1).then_some("quality_gated_tiers"),
                "notices": &notices,
                "search_fallback": &search_fallback,
                "search_quality": &final_quality,
                "search_quality_floor": &quality_floor,
                "search_tiers": &tier_reports,
                "engine_outcomes": outcome_metadata(search_results.outcomes()),
                "search_metrics": metrics_json,
                "search_coalescing": coalescer_json,
                "engine_errors": engine_errors,
                "engine_failures": engine_failures,
                "available_result_count": results.len(),
                "returned_result_count": returned_result_count,
                "output_limited": returned_result_count < results.len(),
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

#[cfg(test)]
#[path = "web_search/network_tests.rs"]
mod network_tests;
