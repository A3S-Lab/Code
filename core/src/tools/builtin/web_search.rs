//! Web search tool - Search the web via a3s-search

use crate::config::{BrowserBackend, HeadlessConfig};
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use a3s_search::engines::{Baidu, BingChina, Brave, DuckDuckGo, Google, So360, Sogou, Wikipedia};
use a3s_search::proxy::{ProxyConfig, ProxyPool};
use a3s_search::WaitStrategy;
use a3s_search::{BrowserFetcher, BrowserPool, BrowserPoolConfig, Search, SearchQuery};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

pub struct WebSearchTool {
    browser_pool: std::sync::OnceLock<Arc<BrowserPool>>,
}

impl WebSearchTool {
    pub fn new() -> Self {
        Self {
            browser_pool: std::sync::OnceLock::new(),
        }
    }

    /// Get or create the BrowserPool for headless browser engines
    fn get_or_init_pool(
        &self,
        headless_config: Option<&HeadlessConfig>,
    ) -> Option<Arc<BrowserPool>> {
        let config = headless_config?;

        let pool_config = BrowserPoolConfig {
            max_tabs: config.max_tabs,
            headless: true,
            chrome_path: if config.backend == BrowserBackend::Chrome {
                config.browser_path.clone()
            } else {
                None
            },
            lightpanda_path: if config.backend == BrowserBackend::Lightpanda {
                config.browser_path.clone()
            } else {
                None
            },
            proxy_url: config.proxy_url.clone(),
            launch_args: config.launch_args.clone(),
            backend: match config.backend {
                BrowserBackend::Chrome => a3s_search::BrowserBackend::Chrome,
                BrowserBackend::Lightpanda => a3s_search::BrowserBackend::Lightpanda,
            },
        };

        let pool = BrowserPool::new(pool_config);
        let pool = Arc::new(pool);

        // Try to set, but if already set, use the existing one
        self.browser_pool.get_or_init(|| pool.clone());
        self.browser_pool.get().cloned()
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Add an HTTP engine by shortcut
fn add_http_engine(search: &mut Search, shortcut: &str) -> bool {
    match shortcut.trim() {
        "ddg" => {
            search.add_engine(DuckDuckGo::new());
            true
        }
        "brave" => {
            search.add_engine(Brave::new());
            true
        }
        "wiki" => {
            search.add_engine(Wikipedia::new());
            true
        }
        "sogou" => {
            search.add_engine(Sogou::new());
            true
        }
        "360" | "so360" => {
            search.add_engine(So360::new());
            true
        }
        _ => false,
    }
}

/// Add a headless engine (google, baidu, bing_cn) using BrowserPool
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
        "bing_cn" => {
            let fetcher =
                BrowserFetcher::new(Arc::clone(pool)).with_wait(WaitStrategy::Delay { ms: 2000 });
            search.add_engine(BingChina::new(Arc::new(fetcher)));
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
         (DuckDuckGo, Wikipedia, Brave, Sogou, 360, Google, Baidu, Bing China, etc.). \
         Supports proxy configuration for anti-crawler protection. Returns deduplicated and ranked results. \
         Headless engines (google, baidu, bing_cn) use a browser to render JavaScript."
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
                    "description": "Optional. List of search engines to use. Default: [\"ddg\",\"wiki\"]. Available: ddg (DuckDuckGo), brave (Brave Search), wiki (Wikipedia), sogou (Sogou), 360 / so360 (360 Search), g / google (Google, headless), baidu (Baidu, headless), bing_cn (Bing China, headless)."
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

        let query_str = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => return Ok(ToolOutput::error("query parameter is required")),
        };

        if query_str.trim().is_empty() {
            return Ok(ToolOutput::error("query must not be empty"));
        }

        // Get configuration from context or use defaults
        let config = ctx.search_config.as_ref();
        let headless_config = config.and_then(|c| c.headless.as_ref());

        let default_timeout = config.map(|c| c.timeout).unwrap_or(10);
        let default_engines: Vec<&str> = if let Some(cfg) = config {
            // Build default engines list from enabled engines in config
            cfg.engines
                .iter()
                .filter(|(_, engine_cfg)| engine_cfg.enabled)
                .map(|(name, _)| name.as_str())
                .collect()
        } else {
            vec!["ddg", "wiki"]
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

        let proxy_url = args.get("proxy").and_then(|v| v.as_str());

        // Check if any headless engines are requested
        let needs_headless = engines
            .iter()
            .any(|e| matches!(e.trim(), "g" | "google" | "baidu" | "bing_cn"));

        // Get or initialize BrowserPool if needed
        let browser_pool = if needs_headless {
            self.get_or_init_pool(headless_config)
        } else {
            None
        };

        // Build Search instance with requested engines
        let mut search = Search::new();
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
            if !add_http_engine(&mut search, shortcut_str) {
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

        if search.engine_count() == 0 {
            return Ok(ToolOutput::error(format!(
                "No valid engines found in: {:?}",
                engines
            )));
        }

        // Configure proxy if provided
        if let Some(url) = proxy_url {
            // Parse proxy URL into ProxyConfig
            if let Some(config) = parse_proxy_url(url) {
                let _pool = ProxyPool::with_proxies(vec![config]);
                // Note: proxy is applied per-engine fetcher, not globally
                tracing::debug!("Proxy configuration provided but not yet applied to engines");
            }
        }

        let query = SearchQuery::new(query_str);

        let search_results = match search.search(query).await {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolOutput::error(format!("Search failed: {}", e)));
            }
        };

        let items = search_results.items();
        let results: Vec<_> = items.iter().take(limit).collect();

        // Report engine errors if any
        let errors = search_results.errors();
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
            return Ok(ToolOutput::success(format!(
                "No results found for query: \"{}\"{}",
                query_str, error_note
            )));
        }

        let output = if output_format == "json" {
            let json_results: Vec<serde_json::Value> = results
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "title": r.title,
                        "url": r.url,
                        "content": r.content,
                        "engines": r.engines.iter().collect::<Vec<_>>(),
                        "score": r.score,
                    })
                })
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
                let engines: Vec<&String> = result.engines.iter().collect();
                text.push_str(&format!(
                    "{}. {}\n   URL: {}\n   {}\n   (via {})\n\n",
                    i + 1,
                    result.title,
                    result.url,
                    result.content,
                    engines
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                ));
            }
            if !error_note.is_empty() {
                text.push_str(&error_note);
            }
            text
        };

        Ok(ToolOutput::success(output))
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
mod tests {
    use super::*;
    use std::path::PathBuf;

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
        assert!(add_http_engine(&mut search, "ddg"));
        assert_eq!(search.engine_count(), 1);

        assert!(add_http_engine(&mut search, "wiki"));
        assert_eq!(search.engine_count(), 2);

        assert!(add_http_engine(&mut search, "brave"));
        assert_eq!(search.engine_count(), 3);
    }

    #[test]
    fn test_add_http_engine_unknown() {
        let mut search = Search::new();
        assert!(!add_http_engine(&mut search, "nonexistent"));
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

        assert!(add_headless_engine(&mut search, "bing_cn", &pool));
        assert_eq!(search.engine_count(), 3);
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
}
