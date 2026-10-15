//! Web search tool - Search the web via a3s-search

mod engines;
mod fallback;

#[cfg(feature = "headless-search")]
use crate::config::{BrowserBackend, HeadlessConfig};
#[cfg(feature = "headless-search")]
use crate::moli_runtime::ensure_moli;
use crate::tools::types::{Tool, ToolContext, ToolErrorKind, ToolOutput};
#[cfg(feature = "headless-search")]
use a3s_search::product::PendingHeadlessRuntime;
use a3s_search::product::{execute_cascade, CascadeControls, CascadeRequest, HeadlessBrowser};
use a3s_search::proxy::ProxyConfig;
use a3s_search::{
    EngineTierPlan, Metrics, MetricsSnapshot, SearchCoalescerSnapshot, SearchQuery, SearchResult,
};
use anyhow::Result;
use async_trait::async_trait;
use regex::Regex;
#[cfg(feature = "headless-search")]
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const MIN_FULL_TEXT_BYTES: usize = 512;
const MAX_FULL_TEXT_BYTES: usize = 32 * 1024;
const MAX_JSON_TITLE_BYTES: usize = 2 * 1024;
const MAX_JSON_CONTENT_BYTES: usize = 4 * 1024;
const JSON_OUTPUT_RESERVE_BYTES: usize = 4 * 1024;
const MAX_JSON_OUTPUT_BYTES: usize = crate::tools::MAX_OUTPUT_SIZE - JSON_OUTPUT_RESERVE_BYTES;

#[cfg(feature = "headless-search")]
const WEB_SEARCH_DESCRIPTION: &str = "When saved notes are available, this tool searches them before the network. A matching note is returned and the web is not queried. Call web_search again with the same query if those passages do not answer. \
     Search the web through a structurally gated cascade: native APIs first, HTTP/RSS engines \
     only when needed, and the bundled Moli headless browser only when earlier tiers remain insufficient. \
     Unavailable engines are skipped through session-scoped circuit state, and all executed tiers \
     are deduplicated and ranked together. An explicit engines list runs only those requested tiers. \
     Moli is discovered from the package or installed once in the shared A3S Code cache, then reused by all local Code processes. \
     Supports proxy configuration for conventional and headless search transports.";

#[cfg(not(feature = "headless-search"))]
const WEB_SEARCH_DESCRIPTION: &str = "When saved notes are available, this tool searches them before the network. A matching note is returned and the web is not queried. Call web_search again with the same query if those passages do not answer. \
     Search the web through a structurally gated cascade of native APIs and HTTP/RSS engines. \
     Unavailable engines are skipped through session-scoped circuit state, and all executed tiers \
     are deduplicated and ranked together. An explicit engines list runs only those requested tiers. \
     Supports proxy configuration for conventional search transports.";

#[cfg(feature = "headless-search")]
const ENGINE_CATALOG_DESCRIPTION: &str =
    "Optional. List of search engines or native providers to use. Without explicit configuration, \
     all built-in providers that advertise anonymous access are combined with the public HTTP \
     defaults. Available: anysearch (anonymous or authenticated native provider), tavily (keyless \
     or authenticated native provider), ddg (DuckDuckGo), brave (Brave Search), bing (Bing RSS), \
     wiki (Wikipedia), sogou (Sogou), 360 / so360 (360 Search), bing_cn (Bing China RSS), \
     and opt-in billed providers tinyfish, bocha, aliyun, tencent, firecrawl, and a3s (named only; \
     they are not in the default cascade), \
     g / google (Google, headless), baidu (Baidu, headless), bing_browser (Bing, headless), \
     brave_browser (Brave, headless). The default headless backend is Moli; Chrome and \
     Lightpanda remain explicit compatibility backends.";

#[cfg(not(feature = "headless-search"))]
const ENGINE_CATALOG_DESCRIPTION: &str =
    "Optional. List of search engines or native providers to use. Without explicit configuration, \
     all built-in providers that advertise anonymous access are combined with the public HTTP \
     defaults. Available: anysearch (anonymous or authenticated native provider), tavily (keyless \
     or authenticated native provider), ddg (DuckDuckGo), brave (Brave Search), bing (Bing RSS), \
     wiki (Wikipedia), sogou (Sogou), 360 / so360 (360 Search), bing_cn (Bing China RSS), \
     and opt-in billed providers tinyfish, bocha, aliyun, tencent, firecrawl, and a3s (named only; \
     they are not in the default cascade).";

#[cfg(test)]
pub(crate) use engines::bridge_search_config;
#[cfg(not(test))]
use engines::bridge_search_config;
use fallback::{
    failure_metadata, failure_summary, outcome_metadata, text_notice_note,
    tool_error_kind_for_failures, usable_result_count,
};

/// Browser render retries per request, the `BrowserFetcher` default Code used before.
const BROWSER_MAX_RETRIES: u32 = 3;

pub struct WebSearchTool;

impl WebSearchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "headless-search")]
fn effective_headless_config(
    configured: Option<&HeadlessConfig>,
    proxy_url: Option<&str>,
) -> HeadlessConfig {
    // Moli is the product default. Legacy managed Chrome/Lightpanda discovery
    // is still available through an explicit backend in ACL; it must not
    // silently replace the bundled/downloadable default.
    let mut config = configured.cloned().unwrap_or_default();
    if let Some(proxy_url) = proxy_url {
        config.proxy_url = Some(proxy_url.to_string());
    }
    config
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

fn json_search_payload(results: Vec<serde_json::Value>) -> serde_json::Value {
    // Structural requirements decide whether another tier runs. Usable rows
    // stay a result array; `partial` vs `complete` lives on tool metadata.
    serde_json::Value::Array(results)
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

fn cascade_controls(ctx: &ToolContext, metrics: Arc<Metrics>) -> CascadeControls {
    CascadeControls {
        circuit: ctx.search_circuit_breaker(),
        bulkhead: ctx.search_bulkhead(),
        coalescer: ctx.search_request_coalescer(),
        metrics: Some(metrics),
        retry_budget: Some(ctx.search_retry_budget()),
        headless_runtime: None,
        admission: Some(Arc::new(|result: &SearchResult| {
            !safe_search_result_url(result).is_empty()
        })),
    }
}

/// Aborts provisioning the cascade never awaited, so a satisfied fast path
/// never pays for the slow one.
#[cfg(any(test, feature = "headless-search"))]
struct AbortOnDrop(tokio::task::AbortHandle);

#[cfg(any(test, feature = "headless-search"))]
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(feature = "headless-search")]
async fn prepare_moli(config: &HeadlessConfig) -> std::result::Result<PathBuf, String> {
    let timeout = Duration::from_secs(config.moli_download_timeout_secs.clamp(1, 600));
    match tokio::time::timeout(timeout, ensure_moli(config, timeout)).await {
        Ok(Ok(path)) => Ok(path),
        Ok(Err(error)) => Err(sanitize_http_urls(&error.to_string())),
        Err(_) => Err(format!(
            "Moli provisioning exceeded its {} second budget",
            timeout.as_secs()
        )),
    }
}

/// The browser backend and the runtime Code prepares for it. Moli
/// provisioning starts now on its own budget and overlaps the earlier tiers.
#[cfg(feature = "headless-search")]
fn headless_runtime(
    config: HeadlessConfig,
    needed: bool,
) -> (
    HeadlessBrowser,
    Option<PendingHeadlessRuntime>,
    Option<AbortOnDrop>,
) {
    use futures::FutureExt;

    let browser = match config.backend {
        BrowserBackend::Moli => HeadlessBrowser::Moli,
        BrowserBackend::Chrome => HeadlessBrowser::Chrome,
        BrowserBackend::Lightpanda => HeadlessBrowser::Lightpanda,
    };
    if !needed {
        return (browser, None, None);
    }
    let max_tabs = Some(config.max_tabs);
    if !config.backend.is_moli() {
        let runtime = a3s_search::product::HeadlessRuntime {
            executable: config.browser_path.as_ref().map(PathBuf::from),
            max_tabs,
            launch_args: config.launch_args.clone(),
        };
        return (
            browser,
            Some(futures::future::ready(Ok(runtime)).boxed().shared()),
            None,
        );
    }
    let task = tokio::spawn(async move { prepare_moli(&config).await });
    let guard = AbortOnDrop(task.abort_handle());
    let pending = async move {
        match task.await {
            Ok(Ok(path)) => Ok(a3s_search::product::HeadlessRuntime {
                executable: Some(path),
                max_tabs,
                launch_args: Vec::new(),
            }),
            Ok(Err(error)) => Err(error),
            Err(error) => Err(format!("Moli provisioning task failed: {error}")),
        }
    }
    .boxed()
    .shared();
    (browser, Some(pending), Some(guard))
}

fn web_escalations() -> &'static std::sync::Mutex<std::collections::VecDeque<(String, String)>> {
    static ESCALATIONS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::VecDeque<(String, String)>>,
    > = std::sync::OnceLock::new();
    ESCALATIONS.get_or_init(|| std::sync::Mutex::new(std::collections::VecDeque::new()))
}

const WEB_ESCALATION_LIMIT: usize = 32;

fn claim_web_escalation(session: &str, query: &str) -> bool {
    let mut queue = web_escalations()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let Some(index) = queue
        .iter()
        .position(|(saved_session, saved_query)| saved_session == session && saved_query == query)
    else {
        return false;
    };
    queue.remove(index);
    true
}

fn remember_web_escalation(session: &str, query: &str) {
    let mut queue = web_escalations()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    queue.retain(|(saved_session, saved_query)| saved_session != session || saved_query != query);
    queue.push_back((session.to_string(), query.to_string()));
    while queue.len() > WEB_ESCALATION_LIMIT {
        queue.pop_front();
    }
}

#[derive(Debug)]
enum LocalFirst {
    SearchWeb,
    Answer(String),
    Stop(String),
}

fn local_first_outcome(output: &str, is_error: bool) -> LocalFirst {
    let text = output.trim();
    if !is_error && (text.is_empty() || text == "no-match" || text == "not-ready") {
        return LocalFirst::SearchWeb;
    }
    if is_error && text == "denied" {
        return LocalFirst::SearchWeb;
    }
    if is_error {
        return LocalFirst::Stop(text.to_string());
    }
    LocalFirst::Answer(text.to_string())
}

async fn saved_notes(
    ctx: &crate::tools::ToolContext,
    query: &str,
) -> Option<crate::tools::ToolResult> {
    let invoker = ctx.tool_invoker()?;
    if !invoker
        .available_tools()
        .iter()
        .any(|name| name == "search_wiki")
    {
        return None;
    }
    let invocation = crate::tools::ToolInvocation::runtime_internal(
        "search_wiki",
        serde_json::json!({ "query": query, "scope": "local" }),
    );
    Some(invoker.invoke(invocation, ctx).await)
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        WEB_SEARCH_DESCRIPTION
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
                    "description": ENGINE_CATALOG_DESCRIPTION
                },
                "limit": {
                    "type": "integer",
                    "description": "Optional. Maximum number of results to return. Default: 10. Maximum: 50."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Optional. Search timeout in seconds. Default: 20. Maximum: 60."
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
                    "engines": ["sogou", "bing_cn"],
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
        let query_str = query_str.trim();
        let session = ctx.session_id.clone().unwrap_or_default();
        if !claim_web_escalation(&session, query_str) {
            if let Some(found) = saved_notes(ctx, query_str).await {
                match local_first_outcome(&found.output, found.exit_code != 0) {
                    LocalFirst::SearchWeb => {}
                    LocalFirst::Answer(passages) => {
                        remember_web_escalation(&session, query_str);
                        return Ok(ToolOutput::success(format!(
                            "{passages}\n\nlocal-knowledge: saved notes matched this query, so the web was not searched. Call web_search again with the same query if these passages do not answer."
                        )));
                    }
                    LocalFirst::Stop(message) => {
                        return Ok(ToolOutput::error(format!("local-knowledge: {message}")));
                    }
                }
            }
        }

        // Get configuration from context or use defaults
        let config = ctx.search_config.as_ref();
        let default_timeout = config.map(|c| c.timeout).unwrap_or(10);
        let (bridged_config, setup_failures) = match config {
            Some(config) => {
                let (bridged, failures) = bridge_search_config(config);
                (Some(bridged), failures)
            }
            None => (None, Vec::new()),
        };
        let requested = args.get("engines").map(|value| match value.as_array() {
            Some(items) => items
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::to_string)
                .collect::<Vec<_>>(),
            // Comma-separated string like "baidu,ddg" or a single engine like "baidu"
            None => value
                .as_str()
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|engine| !engine.is_empty())
                .map(str::to_string)
                .collect(),
        });
        let configured = config
            .filter(|config| !config.engines.is_empty())
            .map(|config| {
                let mut names = config
                    .engines
                    .iter()
                    .filter(|(_, engine)| engine.enabled)
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>();
                names.sort_unstable();
                names
            });
        let engine_selection_source = if requested.is_some() {
            "request"
        } else if configured.is_some() {
            "config"
        } else {
            "builtin_default"
        };
        let selected_engines = requested.or(configured);
        // A provider whose configuration failed to bridge must not run with its defaults.
        let selection = selected_engines.as_ref().map(|names| {
            names
                .iter()
                .filter(|name| {
                    !setup_failures
                        .iter()
                        .any(|failure| failure.engine.eq_ignore_ascii_case(name.trim()))
                })
                .cloned()
                .collect::<Vec<_>>()
        });

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
        // The request deadline below bounds only validation and proxy
        // discovery; the cascade gets a fresh `total_timeout`, and Moli
        // provisioning runs on its own budget.
        let request_deadline = search_started + total_timeout;

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
            .or_else(super::safe_http::explicit_web_proxy_from_env);
        if proxy_url.is_none() {
            let remaining = request_deadline.saturating_duration_since(Instant::now());
            if !remaining.is_zero() {
                proxy_url = tokio::time::timeout(remaining, super::safe_http::system_web_proxy())
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
        let tier_order = config
            .and_then(|config| config.cascade_order.as_deref())
            .map(engines::tier_order);
        let plan = match EngineTierPlan::new(
            selection.as_deref(),
            bridged_config.as_ref(),
            tier_order.as_deref(),
        ) {
            Ok(plan) => plan,
            Err(message) => {
                return Ok(ToolOutput::error(&message)
                    .with_error_kind(ToolErrorKind::InvalidArgument { message }))
            }
        };
        let selected_engines = selected_engines.unwrap_or_else(|| plan.shortcuts());
        if plan.is_empty() {
            let mut message = format!("No valid engines found in: {:?}", selected_engines);
            let setup_summary = failure_summary(&setup_failures);
            if !setup_summary.is_empty() {
                message.push_str(&format!(" ({setup_summary})"));
            }
            return Ok(ToolOutput::error(&message)
                .with_error_kind(ToolErrorKind::InvalidArgument { message })
                .with_metadata(serde_json::json!({
                    "status": "failed",
                    "engine_selection_source": engine_selection_source,
                    "selected_engines": &selected_engines,
                    "engine_failures": failure_metadata(&setup_failures),
                })));
        }

        #[cfg_attr(not(feature = "headless-search"), allow(unused_mut))]
        let mut controls = cascade_controls(ctx, Arc::clone(&search_metrics));
        #[cfg(feature = "headless-search")]
        let (browser, _moli_provision) = {
            let headless = effective_headless_config(
                config.and_then(|config| config.headless.as_ref()),
                proxy_url.as_deref(),
            );
            let (browser, runtime, guard) = headless_runtime(headless, !plan.headless().is_empty());
            controls.headless_runtime = runtime;
            (browser, guard)
        };
        #[cfg(not(feature = "headless-search"))]
        let browser = HeadlessBrowser::default();

        let request = CascadeRequest {
            query: SearchQuery::new(query_str),
            limit,
            timeout: total_timeout,
            proxy: proxy_url.as_deref(),
            config: bridged_config.as_ref(),
            browser,
            browser_max_retries: BROWSER_MAX_RETRIES,
        };
        let outcome = match execute_cascade(&plan, request, controls).await {
            Ok(outcome) => outcome,
            Err(error) => {
                let message = format!(
                    "web_search failed: {}",
                    sanitize_http_urls(&error.to_string())
                );
                return Ok(ToolOutput::error(message).with_metadata(serde_json::json!({
                    "status": "failed",
                    "engine_selection_source": engine_selection_source,
                    "selected_engines": &selected_engines,
                })));
            }
        };
        let receipt = outcome.receipt;
        let mut search_results = outcome.results;
        for failure in setup_failures {
            search_results.add_failure(failure);
        }
        let retrieval_health = receipt.final_health;
        let retrieval_requirements = receipt.retrieval_requirements;
        let requirements_met = receipt.retrieval_requirements_met;
        let tier_reports = receipt.executed_tiers;
        search_results
            .set_duration(u64::try_from(search_started.elapsed().as_millis()).unwrap_or(u64::MAX));

        let mut notices = Vec::new();
        let failure_summary = failure_summary(search_results.failures());
        if usable_result_count(&search_results) > 0 && !failure_summary.is_empty() {
            notices.push(format!(
                "Search completed with degraded engines: {failure_summary}."
            ));
        }
        if !requirements_met {
            notices.push(
                "Search exhausted the available tiers before the structural retrieval requirements were met."
                    .to_string(),
            );
        }
        let executed_engines = search_results
            .outcomes()
            .iter()
            .map(|outcome| outcome.shortcut.clone())
            .collect::<Vec<_>>();
        let search_fallback = serde_json::json!({
            "trigger": "retrieval_requirements",
            "mode": "tiered",
            "attempted": tier_reports.len() > 1,
            "engines": executed_engines,
            "successful": requirements_met,
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
                "status": if requirements_met && errors.is_empty() { "complete" } else { "failed" },
                "engine_selection_source": engine_selection_source,
                "selected_engines": &selected_engines,
                "engine_fallback": (tier_reports.len() > 1).then_some("structurally_gated_tiers"),
                "notices": &notices,
                "search_fallback": &search_fallback,
                "retrieval_health": &retrieval_health,
                "retrieval_requirements": &retrieval_requirements,
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
            if requirements_met && errors.is_empty() {
                return Ok(ToolOutput::success_external(message).with_metadata(metadata));
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
                serde_json::to_string_pretty(&json_search_payload(json_results))
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

        let status = if requirements_met && errors.is_empty() {
            "complete"
        } else {
            "partial"
        };
        Ok(
            ToolOutput::success_external(output).with_metadata(serde_json::json!({
                "status": status,
                "engine_selection_source": engine_selection_source,
                "selected_engines": &selected_engines,
                "source_anchors": source_anchors,
                "engine_fallback": (tier_reports.len() > 1).then_some("structurally_gated_tiers"),
                "notices": &notices,
                "search_fallback": &search_fallback,
                "retrieval_health": &retrieval_health,
                "retrieval_requirements": &retrieval_requirements,
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
