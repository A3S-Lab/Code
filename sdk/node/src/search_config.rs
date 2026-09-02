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
    /// Whether the engine is enabled. Omission keeps Core's default (`true`).
    pub enabled: Option<bool>,
    /// Ranking weight. Omission keeps Core's default (`1.0`).
    pub weight: Option<f64>,
    pub timeout: Option<u32>,
}

impl From<SearchEngineConfig> for RustSearchEngineConfig {
    fn from(config: SearchEngineConfig) -> Self {
        Self {
            enabled: config.enabled.unwrap_or(true),
            weight: config.weight.unwrap_or(1.0),
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
    /// Moli standalone JavaScript-capable headless browser (the default).
    Moli,
}

impl From<BrowserBackend> for RustBrowserBackend {
    fn from(backend: BrowserBackend) -> Self {
        match backend {
            BrowserBackend::Moli => RustBrowserBackend::Moli,
            BrowserBackend::Chrome => RustBrowserBackend::Chrome,
            BrowserBackend::Lightpanda => RustBrowserBackend::Lightpanda,
        }
    }
}

/// Headless browser configuration.
#[napi(object)]
#[derive(Clone)]
pub struct HeadlessConfig {
    /// Browser backend. Omission selects Moli, the release default.
    pub backend: Option<BrowserBackend>,
    pub browser_path: Option<String>,
    pub max_tabs: Option<u32>,
    pub auto_download_moli: Option<bool>,
    pub moli_version: Option<String>,
    pub moli_sha256: Option<String>,
    pub moli_cache_dir: Option<String>,
    pub moli_download_timeout_secs: Option<u32>,
    pub launch_args: Option<Vec<String>>,
    pub proxy_url: Option<String>,
}

impl From<HeadlessConfig> for RustHeadlessConfig {
    fn from(config: HeadlessConfig) -> Self {
        Self {
            backend: config.backend.unwrap_or(BrowserBackend::Moli).into(),
            browser_path: config.browser_path,
            max_tabs: config.max_tabs.unwrap_or(4) as usize,
            auto_download_moli: config.auto_download_moli.unwrap_or(true),
            moli_version: config.moli_version,
            moli_sha256: config.moli_sha256,
            moli_cache_dir: config.moli_cache_dir.map(std::path::PathBuf::from),
            moli_download_timeout_secs: config
                .moli_download_timeout_secs
                .map(u64::from)
                .unwrap_or(120),
            launch_args: config.launch_args.unwrap_or_default(),
            proxy_url: config.proxy_url,
        }
    }
}

/// Health monitor configuration for search engines.
#[napi(object)]
#[derive(Clone)]
pub struct SearchHealthConfig {
    /// Consecutive failures before suspension. Omission keeps Core's default.
    pub max_failures: Option<u32>,
    /// Suspension duration. Omission keeps Core's default.
    pub suspend_seconds: Option<u32>,
}

impl From<SearchHealthConfig> for RustSearchHealthConfig {
    fn from(config: SearchHealthConfig) -> Self {
        Self {
            max_failures: config.max_failures.unwrap_or(3),
            suspend_seconds: u64::from(config.suspend_seconds.unwrap_or(60)),
        }
    }
}

/// Search engine configuration (a3s-search integration).
#[napi(object)]
#[derive(Clone)]
pub struct SearchConfig {
    /// Global timeout in seconds. Omission keeps Core's default (`20`).
    pub timeout: Option<u32>,
    pub health: Option<SearchHealthConfig>,
    /// Per-engine overrides. Omission keeps the Core engine defaults.
    pub engines: Option<HashMap<String, SearchEngineConfig>>,
    pub headless: Option<HeadlessConfig>,
}

impl From<SearchConfig> for RustSearchConfig {
    fn from(config: SearchConfig) -> Self {
        Self {
            timeout: u64::from(config.timeout.unwrap_or(20)),
            health: config.health.map(Into::into),
            engines: config
                .engines
                .unwrap_or_default()
                .into_iter()
                .map(|(name, engine)| (name, engine.into()))
                .collect(),
            headless: config.headless.map(Into::into),
        }
    }
}
