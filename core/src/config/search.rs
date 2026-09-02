use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ============================================================================
// Search / Browser / Document Configuration
// ============================================================================

/// Search engine configuration (a3s-search integration)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchConfig {
    /// Default timeout in seconds for all engines
    #[serde(default = "default_search_timeout")]
    pub timeout: u64,

    /// Health monitor configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<SearchHealthConfig>,

    /// Engine configurations
    #[serde(default, rename = "engine")]
    pub engines: std::collections::HashMap<String, SearchEngineConfig>,

    /// Headless browser configuration for JS-rendered engines (Google, Baidu,
    /// Bing, and Brave). When omitted, Moli is provisioned lazily from the
    /// bundled sidecar or the shared per-user cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headless: Option<HeadlessConfig>,
}

/// Browser backend for JS-rendered search engines.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserBackend {
    /// Moli's standalone, JavaScript-capable headless browser and the default.
    #[default]
    Moli,
    /// Chrome/Chromium headless browser for explicit compatibility use.
    #[serde(alias = "chromium")]
    Chrome,
    /// Explicit Lightpanda backend (native Linux/macOS; WSL2 on Windows).
    Lightpanda,
}

/// Headless browser configuration for JS-rendered search engines.
///
/// Moli is the default backend. When it is selected and no executable is
/// configured or packaged, A3S Code downloads the pinned, digest-verified
/// runtime into the user cache on first use.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadlessConfig {
    /// Browser backend to use.
    #[serde(default)]
    pub backend: BrowserBackend,

    /// Maximum number of concurrent browser tabs.
    #[serde(default = "default_headless_max_tabs")]
    pub max_tabs: usize,

    /// Path to the browser executable. If None, Moli is discovered/downloaded
    /// or an explicit Chrome/Lightpanda runtime is discovered.
    #[serde(
        default,
        alias = "chromePath",
        alias = "lightpandaPath",
        alias = "obscuraPath",
        alias = "playwrightPath",
        skip_serializing_if = "Option::is_none"
    )]
    pub browser_path: Option<String>,

    /// Download Moli automatically when the selected backend is Moli and no
    /// usable executable is already available.
    #[serde(default = "default_auto_download_moli")]
    pub auto_download_moli: bool,

    /// Optional pinned Moli release version. Defaults to the version bundled
    /// by this A3S Code release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moli_version: Option<String>,

    /// Optional SHA-256 digest for the pinned Moli archive. Supplying a version
    /// without its digest is rejected by the runtime manager.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moli_sha256: Option<String>,

    /// Optional cache directory for the managed Moli runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moli_cache_dir: Option<PathBuf>,

    /// Maximum time in seconds spent provisioning a missing Moli runtime.
    /// This budget is separate from the per-request web-search timeout so a
    /// first-use download does not make the default search path unusable.
    #[serde(default = "default_moli_download_timeout_secs")]
    pub moli_download_timeout_secs: u64,

    /// Additional browser launch arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub launch_args: Vec<String>,

    /// Proxy URL for the browser to use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
}

impl BrowserBackend {
    pub fn is_moli(self) -> bool {
        matches!(self, Self::Moli)
    }

    pub fn is_lightpanda(self) -> bool {
        matches!(self, Self::Lightpanda)
    }
}

impl Default for HeadlessConfig {
    fn default() -> Self {
        Self {
            backend: BrowserBackend::Moli,
            max_tabs: 4,
            browser_path: None,
            auto_download_moli: true,
            moli_version: None,
            moli_sha256: None,
            moli_cache_dir: None,
            moli_download_timeout_secs: default_moli_download_timeout_secs(),
            launch_args: Vec::new(),
            proxy_url: None,
        }
    }
}

/// Default configuration for built-in document context extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentParserConfig {
    /// Whether the default document extraction stack is registered in the parser registry.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Maximum file size accepted by the parser, in MiB.
    #[serde(default = "default_document_parser_max_file_size_mb")]
    pub max_file_size_mb: u64,

    /// Optional cache settings for parsed / normalized document context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<DocumentCacheConfig>,
}

impl Default for DocumentParserConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_file_size_mb: default_document_parser_max_file_size_mb(),
            cache: Some(DocumentCacheConfig::default()),
        }
    }
}

impl DocumentParserConfig {
    pub fn normalized(&self) -> Self {
        Self {
            enabled: self.enabled,
            max_file_size_mb: self.max_file_size_mb.clamp(1, 1024),
            cache: self.cache.as_ref().map(DocumentCacheConfig::normalized),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentCacheConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<PathBuf>,
}

impl Default for DocumentCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            directory: None,
        }
    }
}

impl DocumentCacheConfig {
    pub fn normalized(&self) -> Self {
        Self {
            enabled: self.enabled,
            directory: self.directory.clone(),
        }
    }
}

/// Search health monitor configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHealthConfig {
    /// Number of consecutive failures before suspending
    #[serde(default = "default_max_failures")]
    pub max_failures: u32,

    /// Suspension duration in seconds
    #[serde(default = "default_suspend_seconds")]
    pub suspend_seconds: u64,
}

/// Per-engine search configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchEngineConfig {
    /// Whether the engine is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Weight for ranking (higher = more influence)
    #[serde(default = "default_weight")]
    pub weight: f64,

    /// Per-engine timeout override in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

pub(crate) fn default_search_timeout() -> u64 {
    20
}

pub(crate) fn default_headless_max_tabs() -> usize {
    4
}

fn default_auto_download_moli() -> bool {
    true
}

pub(crate) fn default_moli_download_timeout_secs() -> u64 {
    120
}

fn default_max_failures() -> u32 {
    3
}

fn default_suspend_seconds() -> u64 {
    60
}

pub(crate) fn default_enabled() -> bool {
    true
}

fn default_weight() -> f64 {
    1.0
}

pub(crate) fn default_document_parser_max_file_size_mb() -> u64 {
    50
}
