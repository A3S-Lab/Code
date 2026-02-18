//! Web search tool - Search the web via a3s-search

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use a3s_search::engines::{Brave, DuckDuckGo, So360, Sogou, Wikipedia};
use a3s_search::proxy::{ProxyConfig, ProxyPool};
use a3s_search::{Search, SearchQuery};
use anyhow::Result;
use async_trait::async_trait;

pub struct WebSearchTool;

/// Map a shortcut string to an engine and add it to the Search instance
fn add_engine_by_shortcut(search: &mut Search, shortcut: &str) {
    match shortcut.trim() {
        "ddg" => search.add_engine(DuckDuckGo::new()),
        "brave" => search.add_engine(Brave::new()),
        "wiki" => search.add_engine(Wikipedia::new()),
        "sogou" => search.add_engine(Sogou::new()),
        "360" | "so360" => search.add_engine(So360::new()),
        // google, baidu, bing_cn require the headless feature (browser pool);
        // silently skip when headless is not compiled in.
        other => {
            tracing::warn!("Unknown or unavailable search engine: {}", other);
        }
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web using multiple search engines. Aggregates results from multiple engines \
         (DuckDuckGo, Wikipedia, Brave, Sogou, 360, etc.). Supports proxy configuration for \
         anti-crawler protection. Returns deduplicated and ranked results."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "engines": {
                    "type": "string",
                    "description": "Comma-separated list of engines to use (default: ddg,wiki). Available: ddg, brave, wiki, sogou, 360"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 10, max: 50)"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Search timeout in seconds (default: 10, max: 60)"
                },
                "proxy": {
                    "type": "string",
                    "description": "Proxy URL (e.g., http://127.0.0.1:8080 or socks5://127.0.0.1:1080)"
                },
                "format": {
                    "type": "string",
                    "enum": ["text", "json"],
                    "description": "Output format (default: text)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let query_str = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => return Ok(ToolOutput::error("query parameter is required")),
        };

        if query_str.trim().is_empty() {
            return Ok(ToolOutput::error("query must not be empty"));
        }

        // Get configuration from context or use defaults
        let config = ctx.search_config.as_ref();

        let default_timeout = config.map(|c| c.timeout).unwrap_or(10);
        let default_engines = if let Some(cfg) = config {
            // Build default engines list from enabled engines in config
            cfg.engines
                .iter()
                .filter(|(_, engine_cfg)| engine_cfg.enabled)
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>()
                .join(",")
        } else {
            "ddg,wiki".to_string()
        };

        let engines_str = args
            .get("engines")
            .and_then(|v| v.as_str())
            .unwrap_or(&default_engines);

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

        // Build Search instance with requested engines
        let mut search = Search::new();
        search.set_timeout(std::time::Duration::from_secs(timeout_secs));

        for shortcut in engines_str.split(',') {
            let shortcut = shortcut.trim();

            // Check if engine is configured and get its settings
            let engine_config = config.and_then(|c| c.engines.get(shortcut));

            // Skip if explicitly disabled in config
            if let Some(engine_cfg) = engine_config {
                if !engine_cfg.enabled {
                    tracing::debug!("Skipping disabled engine: {}", shortcut);
                    continue;
                }
            }

            add_engine_by_shortcut(&mut search, shortcut);
        }

        if search.engine_count() == 0 {
            return Ok(ToolOutput::error(format!(
                "No valid engines found in: {}",
                engines_str
            )));
        }

        // Configure proxy if provided
        if let Some(url) = proxy_url {
            // Parse proxy URL into ProxyConfig
            if let Some(config) = parse_proxy_url(url) {
                let pool = ProxyPool::with_proxies(vec![config]);
                // TODO: a3s-search API changed, need to update
                // search.set_proxy_pool(pool);
                tracing::warn!("Proxy configuration is temporarily disabled due to API changes");
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
    let (host, port) = if let Some(colon_pos) = rest.rfind(':') {
        let host = &rest[..colon_pos];
        let port_str = &rest[colon_pos + 1..];
        match port_str.parse::<u16>() {
            Ok(p) => (host, p),
            Err(_) => return None,
        }
    } else {
        return None;
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
        let tool = WebSearchTool;
        let ctx = ToolContext::new(PathBuf::from("/tmp"));

        let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_web_search_empty_query() {
        let tool = WebSearchTool;
        let ctx = ToolContext::new(PathBuf::from("/tmp"));

        let result = tool
            .execute(&serde_json::json!({"query": ""}), &ctx)
            .await
            .unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_web_search_no_valid_engines() {
        let tool = WebSearchTool;
        let ctx = ToolContext::new(PathBuf::from("/tmp"));

        let result = tool
            .execute(
                &serde_json::json!({"query": "test", "engines": "nonexistent"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.content.contains("No valid engines"));
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
    fn test_add_engine_by_shortcut_valid() {
        let mut search = Search::new();
        add_engine_by_shortcut(&mut search, "ddg");
        assert_eq!(search.engine_count(), 1);

        add_engine_by_shortcut(&mut search, "wiki");
        assert_eq!(search.engine_count(), 2);

        add_engine_by_shortcut(&mut search, "brave");
        assert_eq!(search.engine_count(), 3);
    }

    #[test]
    fn test_add_engine_by_shortcut_unknown() {
        let mut search = Search::new();
        add_engine_by_shortcut(&mut search, "nonexistent");
        assert_eq!(search.engine_count(), 0);
    }
}
