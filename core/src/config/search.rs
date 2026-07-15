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

    /// Headless browser configuration for JS-rendered engines (google and baidu).
    /// When enabled, an existing system or explicitly managed runtime is discovered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headless: Option<HeadlessConfig>,
}

/// Browser backend for JS-rendered search engines.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserBackend {
    /// Chrome/Chromium headless browser.
    #[default]
    Chrome,
    /// Lightpanda headless browser.
    Lightpanda,
}

/// Headless browser configuration for JS-rendered engines.
/// Uses a3s-search's browser pool, backed by Chrome/Chromium or Lightpanda.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadlessConfig {
    /// Browser backend to use.
    #[serde(default)]
    pub backend: BrowserBackend,

    /// Maximum number of concurrent browser tabs.
    #[serde(default = "default_headless_max_tabs")]
    pub max_tabs: usize,

    /// Path to the browser executable. If None, an existing runtime is discovered.
    #[serde(
        default,
        alias = "chromePath",
        alias = "lightpandaPath",
        alias = "obscuraPath",
        alias = "playwrightPath",
        skip_serializing_if = "Option::is_none"
    )]
    pub browser_path: Option<String>,

    /// Additional browser launch arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub launch_args: Vec<String>,

    /// Proxy URL for the browser to use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
}

impl BrowserBackend {
    pub fn is_lightpanda(self) -> bool {
        matches!(self, Self::Lightpanda)
    }
}

impl Default for HeadlessConfig {
    fn default() -> Self {
        Self {
            backend: BrowserBackend::Chrome,
            max_tabs: 4,
            browser_path: None,
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

    /// Optional OCR / vision-model settings for image-heavy documents.
    ///
    /// These settings control OCR fallback when context extraction reaches
    /// scanned or image-heavy inputs. Current parsers may not execute OCR for
    /// every format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr: Option<DocumentOcrConfig>,

    /// Optional cache settings for parsed / normalized document context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<DocumentCacheConfig>,
}

impl Default for DocumentParserConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_file_size_mb: default_document_parser_max_file_size_mb(),
            ocr: None,
            cache: Some(DocumentCacheConfig::default()),
        }
    }
}

impl DocumentParserConfig {
    pub fn normalized(&self) -> Self {
        Self {
            enabled: self.enabled,
            max_file_size_mb: self.max_file_size_mb.clamp(1, 1024),
            ocr: self.ocr.as_ref().map(DocumentOcrConfig::normalized),
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

/// OCR / vision-model configuration for built-in document context extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentOcrConfig {
    /// Whether OCR fallback is enabled for image-heavy documents.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Vision-capable model identifier, for example `openai/gpt-4.1-mini`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Optional custom OCR prompt / extraction instruction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,

    /// Maximum number of rendered images/pages to send for OCR fallback.
    #[serde(default = "default_document_ocr_max_images")]
    pub max_images: usize,

    /// Render DPI when rasterizing pages for OCR fallback.
    #[serde(default = "default_document_ocr_dpi")]
    pub dpi: u32,

    /// OCR provider backend. Defaults to "vision" when model is set.
    /// "vision" - Vision API (OpenAI-compatible)
    /// "builtin" - Local tesseract (requires tesseract + pdftoppm binaries)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,

    /// Base URL for vision API. Defaults to OpenAI API if not set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    /// API key for vision API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

impl Default for DocumentOcrConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: None,
            prompt: None,
            max_images: default_document_ocr_max_images(),
            dpi: default_document_ocr_dpi(),
            provider: None,
            base_url: None,
            api_key: None,
        }
    }
}

impl DocumentOcrConfig {
    pub fn normalized(&self) -> Self {
        Self {
            enabled: self.enabled,
            model: self.model.clone(),
            prompt: self.prompt.clone(),
            max_images: self.max_images.clamp(1, 64),
            dpi: self.dpi.clamp(72, 600),
            provider: self.provider.clone(),
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
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
    10
}

pub(crate) fn default_headless_max_tabs() -> usize {
    4
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

pub(crate) fn default_document_ocr_max_images() -> usize {
    8
}

pub(crate) fn default_document_ocr_dpi() -> u32 {
    144
}
