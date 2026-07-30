//! Search-engine registry, aliases, and construction.

use a3s_search::engines::{
    BingChina, BingParser, BraveParser, DuckDuckGoParser, So360Parser, SogouParser, Wikipedia,
};
use a3s_search::providers::BuiltinProvider;
#[cfg(feature = "headless-search")]
use a3s_search::{
    a3s_use_browser::BrowserPool,
    engines::{Baidu, Google},
    BrowserFetcher, WaitStrategy,
};
use a3s_search::{EngineFailure, HtmlEngine, HttpFetcher, Search, SearchError};
use std::sync::Arc;

const PUBLIC_FALLBACK_ENGINES: [&str; 2] = ["ddg", "wiki"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EngineTier {
    Api,
    Http,
    #[cfg(feature = "headless-search")]
    Headless,
}

fn builtin_default_engines() -> Vec<&'static str> {
    let mut engines = BuiltinProvider::ALL
        .iter()
        .copied()
        .filter_map(|provider| {
            provider
                .create_engine()
                .ok()
                .filter(|engine| engine.descriptor().capabilities.anonymous)
                .map(|_| provider.id())
        })
        .collect::<Vec<_>>();
    engines.extend(PUBLIC_FALLBACK_ENGINES);
    engines
}

pub(super) fn canonical_engine_shortcut(shortcut: &str) -> &str {
    match shortcut.trim() {
        "duckduckgo" => "ddg",
        "wikipedia" => "wiki",
        "google" => "g",
        "so360" => "360",
        shortcut => shortcut,
    }
}

pub(super) fn engine_tier(shortcut: &str) -> Option<EngineTier> {
    let shortcut = canonical_engine_shortcut(shortcut);
    if BuiltinProvider::from_id(shortcut).is_some() {
        return Some(EngineTier::Api);
    }
    match shortcut {
        "ddg" | "brave" | "bing" | "wiki" | "sogou" | "360" | "bing_cn" => Some(EngineTier::Http),
        #[cfg(feature = "headless-search")]
        "g" | "baidu" => Some(EngineTier::Headless),
        _ => None,
    }
}

/// Adds a native API provider or conventional HTTP/RSS engine by shortcut.
pub(super) fn provider_setup_failure(
    provider: BuiltinProvider,
    error: &SearchError,
) -> EngineFailure {
    EngineFailure::new(provider.id(), error.kind(), error.to_string())
        .with_provider(provider.id())
        .with_transient(error.is_transient())
}

pub(super) fn add_http_engine(
    search: &mut Search,
    shortcut: &str,
    proxy_url: Option<&str>,
) -> std::result::Result<bool, EngineFailure> {
    let fetcher = || {
        proxy_url
            .and_then(|proxy| HttpFetcher::with_proxy(proxy).ok())
            .unwrap_or_default()
    };
    match canonical_engine_shortcut(shortcut) {
        "ddg" => {
            search.add_engine(HtmlEngine::with_fetcher(
                DuckDuckGoParser,
                Arc::new(fetcher()),
            ));
            Ok(true)
        }
        "brave" => {
            search.add_engine(HtmlEngine::with_fetcher(BraveParser, Arc::new(fetcher())));
            Ok(true)
        }
        "bing" => {
            search.add_engine(HtmlEngine::with_fetcher(BingParser, Arc::new(fetcher())));
            Ok(true)
        }
        "wiki" => {
            search.add_engine(Wikipedia::with_http_fetcher(fetcher()));
            Ok(true)
        }
        "sogou" => {
            search.add_engine(HtmlEngine::with_fetcher(SogouParser, Arc::new(fetcher())));
            Ok(true)
        }
        "360" => {
            search.add_engine(HtmlEngine::with_fetcher(So360Parser, Arc::new(fetcher())));
            Ok(true)
        }
        "bing_cn" => {
            search.add_engine(BingChina::new(Arc::new(fetcher())));
            Ok(true)
        }
        "anysearch" | "tavily" => {
            let Some(provider) = BuiltinProvider::from_id(canonical_engine_shortcut(shortcut))
            else {
                return Ok(false);
            };
            match provider.create_engine() {
                Ok(engine) => {
                    search.add_engine(engine);
                    Ok(true)
                }
                Err(error) => Err(provider_setup_failure(provider, &error)),
            }
        }
        _ => Ok(false),
    }
}

pub(super) fn default_engine_selection(
    config: Option<&crate::config::SearchConfig>,
) -> (Vec<&str>, &'static str) {
    match config {
        Some(config) if !config.engines.is_empty() => {
            let mut engines = config
                .engines
                .iter()
                .filter(|(_, engine)| engine.enabled)
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>();
            engines.sort_unstable();
            let mut seen = std::collections::HashSet::new();
            engines.retain(|engine| {
                seen.insert(canonical_engine_shortcut(engine).to_ascii_lowercase())
            });
            (engines, "config")
        }
        _ => (builtin_default_engines(), "builtin_default"),
    }
}

/// Add a headless engine using BrowserPool.
#[cfg(feature = "headless-search")]
pub(super) fn add_headless_engine(
    search: &mut Search,
    shortcut: &str,
    pool: &Arc<BrowserPool>,
) -> bool {
    match canonical_engine_shortcut(shortcut) {
        "g" => {
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
