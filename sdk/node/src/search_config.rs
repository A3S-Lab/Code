use a3s_code_core::config::{
    BrowserBackend as RustBrowserBackend, HeadlessConfig as RustHeadlessConfig,
    SearchConfig as RustSearchConfig, SearchEngineConfig as RustSearchEngineConfig,
    SearchHealthConfig as RustSearchHealthConfig,
};
use std::collections::HashMap;

/// Configuration for a search engine.
#[napi(object)]
#[derive(Clone)]
pub struct SearchEngineConfig {
    pub enabled: bool,
    pub weight: f64,
    pub timeout: Option<u32>,
}

impl From<SearchEngineConfig> for RustSearchEngineConfig {
    fn from(config: SearchEngineConfig) -> Self {
        Self {
            enabled: config.enabled,
            weight: config.weight,
            timeout: config.timeout.map(u64::from),
        }
    }
}

/// Browser backend for headless search.
#[napi]
pub enum BrowserBackend {
    /// Chrome/Chromium headless.
    Chrome,
    /// Lightpanda headless browser (native Linux/macOS; WSL2 on Windows hosts).
    Lightpanda,
}

impl From<BrowserBackend> for RustBrowserBackend {
    fn from(backend: BrowserBackend) -> Self {
        match backend {
            BrowserBackend::Chrome => RustBrowserBackend::Chrome,
            BrowserBackend::Lightpanda => RustBrowserBackend::Lightpanda,
        }
    }
}

/// Headless browser configuration.
#[napi(object)]
#[derive(Clone)]
pub struct HeadlessConfig {
    pub backend: BrowserBackend,
    pub browser_path: Option<String>,
    pub max_tabs: Option<u32>,
    pub launch_args: Option<Vec<String>>,
    pub proxy_url: Option<String>,
}

impl From<HeadlessConfig> for RustHeadlessConfig {
    fn from(config: HeadlessConfig) -> Self {
        Self {
            backend: config.backend.into(),
            browser_path: config.browser_path,
            max_tabs: config.max_tabs.unwrap_or(4) as usize,
            launch_args: config.launch_args.unwrap_or_default(),
            proxy_url: config.proxy_url,
        }
    }
}

/// Health monitor configuration for search engines.
#[napi(object)]
#[derive(Clone)]
pub struct SearchHealthConfig {
    pub max_failures: u32,
    pub suspend_seconds: u32,
}

impl From<SearchHealthConfig> for RustSearchHealthConfig {
    fn from(config: SearchHealthConfig) -> Self {
        Self {
            max_failures: config.max_failures,
            suspend_seconds: u64::from(config.suspend_seconds),
        }
    }
}

/// Search engine configuration (a3s-search integration).
#[napi(object)]
#[derive(Clone)]
pub struct SearchConfig {
    pub timeout: u32,
    pub health: Option<SearchHealthConfig>,
    pub engines: HashMap<String, SearchEngineConfig>,
    pub headless: Option<HeadlessConfig>,
}

impl From<SearchConfig> for RustSearchConfig {
    fn from(config: SearchConfig) -> Self {
        Self {
            timeout: u64::from(config.timeout),
            health: config.health.map(Into::into),
            engines: config
                .engines
                .into_iter()
                .map(|(name, engine)| (name, engine.into()))
                .collect(),
            headless: config.headless.map(Into::into),
        }
    }
}
