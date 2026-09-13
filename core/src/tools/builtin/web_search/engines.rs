//! Search-engine registry, aliases, and construction.

#[cfg(feature = "headless-search")]
use crate::config::BrowserBackend;
use a3s_search::engines::{
    BingChina, BingParser, BraveParser, DuckDuckGoParser, So360Parser, SogouParser, Wikipedia,
};
use a3s_search::providers::{
    AliyunConfig, AliyunProvider, AnySearchConfig, AnySearchProvider, BochaConfig, BochaProvider,
    BuiltinProvider, CredentialSource, FirecrawlConfig, FirecrawlProvider, ProviderEngine,
    TavilyConfig, TavilyProvider, TencentConfig, TencentProvider, TinyFishConfig, TinyFishProvider,
};
#[cfg(feature = "headless-search")]
use a3s_search::{
    a3s_use_browser::PageRenderer,
    engines::{Baidu, BingBrowser, BraveBrowser, Google},
    BrowserFetcher, RetryBudget, WaitStrategy,
};
use a3s_search::{EngineFailure, HtmlEngine, HttpFetcher, Search, SearchError};
use std::sync::Arc;

const PUBLIC_FALLBACK_ENGINES: [&str; 2] = ["ddg", "wiki"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
        "bingbrowser" => "bing_browser",
        "bravebrowser" => "brave_browser",
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
        "g" | "baidu" | "bing_browser" | "brave_browser" => Some(EngineTier::Headless),
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
    config: Option<&crate::config::SearchConfig>,
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
        shortcut if BuiltinProvider::from_id(shortcut).is_some() => {
            let Some(provider) = BuiltinProvider::from_id(shortcut) else {
                return Ok(false);
            };
            match create_api_engine(provider, config) {
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

fn with_api_key<T>(
    config: T,
    api_key: Option<&str>,
    apply: impl FnOnce(T, CredentialSource) -> T,
) -> T {
    match api_key {
        Some(api_key) => apply(config, CredentialSource::value(api_key)),
        None => config,
    }
}

fn create_api_engine(
    provider: BuiltinProvider,
    config: Option<&crate::config::SearchConfig>,
) -> a3s_search::Result<ProviderEngine> {
    let engine_config =
        config.and_then(|config| super::fallback::configured_engine(config, provider.id()));
    let api_key = engine_config
        .and_then(|engine| engine.api_key.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let project = engine_config
        .and_then(|engine| engine.project.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());

    match provider {
        BuiltinProvider::AnySearch => {
            let mut provider_config = AnySearchConfig::new()?;
            if let Some(api_key) = api_key {
                provider_config = provider_config.with_api_key(CredentialSource::value(api_key));
            }
            Ok(ProviderEngine::new(AnySearchProvider::new(
                provider_config,
            )?))
        }
        BuiltinProvider::Tavily => {
            let mut provider_config = TavilyConfig::new()?;
            if let Some(api_key) = api_key {
                provider_config = provider_config.with_api_key(CredentialSource::value(api_key));
            }
            if let Some(project) = project {
                provider_config = provider_config.with_project(CredentialSource::value(project));
            }
            Ok(ProviderEngine::new(TavilyProvider::new(provider_config)?))
        }
        BuiltinProvider::TinyFish => Ok(ProviderEngine::new(TinyFishProvider::new(with_api_key(
            TinyFishConfig::new()?,
            api_key,
            TinyFishConfig::with_api_key,
        ))?)),
        BuiltinProvider::Bocha => Ok(ProviderEngine::new(BochaProvider::new(with_api_key(
            BochaConfig::new()?,
            api_key,
            BochaConfig::with_api_key,
        ))?)),
        BuiltinProvider::Aliyun => Ok(ProviderEngine::new(AliyunProvider::new(with_api_key(
            AliyunConfig::new()?,
            api_key,
            AliyunConfig::with_api_key,
        ))?)),
        BuiltinProvider::Tencent => Ok(ProviderEngine::new(TencentProvider::new(with_api_key(
            TencentConfig::new()?,
            api_key,
            TencentConfig::with_api_key,
        ))?)),
        BuiltinProvider::Firecrawl => {
            Ok(ProviderEngine::new(FirecrawlProvider::new(with_api_key(
                FirecrawlConfig::new()?,
                api_key,
                FirecrawlConfig::with_api_key,
            ))?))
        }
        // BuiltinProvider is non_exhaustive; refuse unknown future providers fail-closed.
        _ => Err(SearchError::Other(
            "unsupported builtin search provider".into(),
        )),
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

/// Add a headless engine using the renderer supplied by the host.
#[cfg(feature = "headless-search")]
pub(super) fn add_headless_engine(
    search: &mut Search,
    shortcut: &str,
    renderer: Arc<dyn PageRenderer>,
    backend: BrowserBackend,
    retry_budget: &RetryBudget,
) -> bool {
    match canonical_engine_shortcut(shortcut) {
        "g" => {
            let fetcher = BrowserFetcher::from_renderer(Arc::clone(&renderer))
                .with_retry_budget(retry_budget.clone())
                .with_wait(headless_wait_strategy(backend, "div.g"));
            search.add_engine(Google::new(Arc::new(fetcher)));
            true
        }
        "baidu" => {
            let fetcher = BrowserFetcher::from_renderer(Arc::clone(&renderer))
                .with_retry_budget(retry_budget.clone())
                .with_wait(headless_wait_strategy(backend, "div.c-container"));
            search.add_engine(Baidu::new(Arc::new(fetcher)));
            true
        }
        "bing_browser" => {
            let fetcher = BrowserFetcher::from_renderer(Arc::clone(&renderer))
                .with_retry_budget(retry_budget.clone())
                .with_wait(headless_wait_strategy(backend, "li.b_algo"));
            search.add_engine(BingBrowser::new(Arc::new(fetcher)));
            true
        }
        "brave_browser" => {
            let fetcher = BrowserFetcher::from_renderer(Arc::clone(&renderer))
                .with_retry_budget(retry_budget.clone())
                .with_wait(headless_wait_strategy(backend, "div.snippet"));
            search.add_engine(BraveBrowser::new(Arc::new(fetcher)));
            true
        }
        _ => false,
    }
}

#[cfg(feature = "headless-search")]
fn headless_wait_strategy(backend: BrowserBackend, selector: &str) -> WaitStrategy {
    if backend.is_lightpanda() || backend.is_moli() {
        WaitStrategy::Load
    } else {
        WaitStrategy::Selector {
            css: selector.to_string(),
            timeout_ms: 5000,
        }
    }
}

#[cfg(all(test, feature = "headless-search"))]
mod tests {
    use super::*;

    #[test]
    fn chrome_keeps_selector_waits() {
        let wait = headless_wait_strategy(BrowserBackend::Chrome, "main.result");
        assert!(matches!(
            wait,
            WaitStrategy::Selector {
                css,
                timeout_ms: 5000
            } if css == "main.result"
        ));
    }

    #[test]
    fn lightpanda_uses_supported_load_wait() {
        assert!(matches!(
            headless_wait_strategy(BrowserBackend::Lightpanda, "main.result"),
            WaitStrategy::Load
        ));
    }
}
