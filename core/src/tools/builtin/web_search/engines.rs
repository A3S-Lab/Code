//! Search-engine registry, aliases, and construction.

use a3s_search::a3s_use_browser::BrowserPool;
use a3s_search::engines::{
    Baidu, BingChina, BingParser, BraveParser, DuckDuckGoParser, Google, So360Parser, SogouParser,
    Wikipedia,
};
use a3s_search::providers::BuiltinProvider;
use a3s_search::{
    BrowserFetcher, EngineFailure, HtmlEngine, HttpFetcher, Search, SearchError, WaitStrategy,
};
use std::sync::Arc;

const BUILTIN_DEFAULT_ENGINES: [&str; 2] = ["ddg", "wiki"];

pub(super) fn canonical_engine_shortcut(shortcut: &str) -> &str {
    match shortcut.trim() {
        "duckduckgo" => "ddg",
        "wikipedia" => "wiki",
        "google" => "g",
        "so360" => "360",
        shortcut => shortcut,
    }
}

pub(super) fn should_fallback_from_unavailable_headless(
    engine_count: usize,
    has_headless_config: bool,
    engines: &[&str],
) -> bool {
    engine_count == 0
        && !has_headless_config
        && !engines.is_empty()
        && engines
            .iter()
            .all(|engine| matches!(canonical_engine_shortcut(engine), "g" | "baidu"))
}

pub(super) fn requires_headless_browser(engines: &[&str]) -> bool {
    engines
        .iter()
        .any(|engine| matches!(canonical_engine_shortcut(engine), "g" | "baidu"))
}

/// Add an HTTP engine by shortcut
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

pub(super) fn should_reject_engine_selection(
    engine_count: usize,
    setup_failures: &[EngineFailure],
) -> bool {
    engine_count == 0 && setup_failures.is_empty()
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
        _ => (BUILTIN_DEFAULT_ENGINES.to_vec(), "builtin_default"),
    }
}

/// Add a headless engine using BrowserPool.
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
