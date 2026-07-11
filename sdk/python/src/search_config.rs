use a3s_code_core::config::{
    BrowserBackend as RustBrowserBackend, HeadlessConfig as RustHeadlessConfig,
    SearchConfig as RustSearchConfig, SearchEngineConfig as RustSearchEngineConfig,
    SearchHealthConfig as RustSearchHealthConfig,
};
use pyo3::prelude::*;
use std::collections::HashMap;

/// Configuration for a search engine.
#[pyclass(name = "SearchEngineConfig")]
#[derive(Clone)]
pub(super) struct PySearchEngineConfig {
    #[pyo3(get, set)]
    enabled: bool,
    #[pyo3(get, set)]
    weight: f64,
    #[pyo3(get, set)]
    timeout: Option<u64>,
}

#[pymethods]
impl PySearchEngineConfig {
    #[new]
    #[pyo3(signature = (enabled=true, weight=1.0, timeout=None))]
    fn new(enabled: bool, weight: f64, timeout: Option<u64>) -> Self {
        Self {
            enabled,
            weight,
            timeout,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "SearchEngineConfig(enabled={}, weight={}, timeout={:?})",
            self.enabled, self.weight, self.timeout
        )
    }
}

impl From<PySearchEngineConfig> for RustSearchEngineConfig {
    fn from(config: PySearchEngineConfig) -> Self {
        Self {
            enabled: config.enabled,
            weight: config.weight,
            timeout: config.timeout,
        }
    }
}

/// Health monitor configuration for search engines.
#[pyclass(name = "SearchHealthConfig")]
#[derive(Clone)]
pub(super) struct PySearchHealthConfig {
    #[pyo3(get, set)]
    max_failures: u32,
    #[pyo3(get, set)]
    suspend_seconds: u64,
}

#[pymethods]
impl PySearchHealthConfig {
    #[new]
    #[pyo3(signature = (max_failures=3, suspend_seconds=60))]
    fn new(max_failures: u32, suspend_seconds: u64) -> Self {
        Self {
            max_failures,
            suspend_seconds,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "SearchHealthConfig(max_failures={}, suspend_seconds={})",
            self.max_failures, self.suspend_seconds
        )
    }
}

impl From<PySearchHealthConfig> for RustSearchHealthConfig {
    fn from(config: PySearchHealthConfig) -> Self {
        Self {
            max_failures: config.max_failures,
            suspend_seconds: config.suspend_seconds,
        }
    }
}

/// Headless browser backend selection.
#[pyclass(name = "BrowserBackend", eq, eq_int)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum PyBrowserBackend {
    /// Chrome/Chromium headless.
    Chrome,
    /// Lightpanda headless browser (Linux/macOS only).
    Lightpanda,
}

impl From<PyBrowserBackend> for RustBrowserBackend {
    fn from(backend: PyBrowserBackend) -> Self {
        match backend {
            PyBrowserBackend::Chrome => RustBrowserBackend::Chrome,
            PyBrowserBackend::Lightpanda => RustBrowserBackend::Lightpanda,
        }
    }
}

/// Headless browser configuration for JS-rendered search engines.
#[pyclass(name = "HeadlessConfig")]
#[derive(Clone)]
pub(super) struct PyHeadlessConfig {
    #[pyo3(get, set)]
    backend: PyBrowserBackend,
    #[pyo3(get, set)]
    browser_path: Option<String>,
    #[pyo3(get, set)]
    max_tabs: Option<usize>,
    #[pyo3(get, set)]
    launch_args: Option<Vec<String>>,
    #[pyo3(get, set)]
    proxy_url: Option<String>,
}

#[pymethods]
impl PyHeadlessConfig {
    #[new]
    #[pyo3(signature = (backend, browser_path=None, max_tabs=None, launch_args=None, proxy_url=None))]
    fn new(
        backend: PyBrowserBackend,
        browser_path: Option<String>,
        max_tabs: Option<usize>,
        launch_args: Option<Vec<String>>,
        proxy_url: Option<String>,
    ) -> Self {
        Self {
            backend,
            browser_path,
            max_tabs,
            launch_args,
            proxy_url,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "HeadlessConfig(backend={:?}, browser_path={:?}, max_tabs={:?}, launch_args={:?}, proxy_url={:?})",
            self.backend, self.browser_path, self.max_tabs, self.launch_args, self.proxy_url
        )
    }
}

impl From<PyHeadlessConfig> for RustHeadlessConfig {
    fn from(config: PyHeadlessConfig) -> Self {
        Self {
            backend: config.backend.into(),
            browser_path: config.browser_path,
            max_tabs: config.max_tabs.unwrap_or(4),
            launch_args: config.launch_args.unwrap_or_default(),
            proxy_url: config.proxy_url,
        }
    }
}

/// Search engine configuration (a3s-search integration).
#[pyclass(name = "SearchConfig")]
#[derive(Clone)]
pub(super) struct PySearchConfig {
    #[pyo3(get, set)]
    timeout: u64,
    #[pyo3(get, set)]
    health: Option<PySearchHealthConfig>,
    engines: HashMap<String, PySearchEngineConfig>,
    #[pyo3(get, set)]
    headless: Option<PyHeadlessConfig>,
}

#[pymethods]
impl PySearchConfig {
    #[new]
    #[pyo3(signature = (timeout=10, health=None, headless=None))]
    fn new(
        timeout: u64,
        health: Option<PySearchHealthConfig>,
        headless: Option<PyHeadlessConfig>,
    ) -> Self {
        Self {
            timeout,
            health,
            engines: HashMap::new(),
            headless,
        }
    }

    /// Set engine configuration.
    fn set_engine(&mut self, name: String, config: PySearchEngineConfig) {
        self.engines.insert(name, config);
    }

    /// Get engine configuration.
    fn get_engine(&self, name: String) -> Option<PySearchEngineConfig> {
        self.engines.get(&name).cloned()
    }

    /// Get all engine names.
    fn engine_names(&self) -> Vec<String> {
        self.engines.keys().cloned().collect()
    }

    fn __repr__(&self) -> String {
        format!(
            "SearchConfig(timeout={}, engines={}, health={:?})",
            self.timeout,
            self.engines.len(),
            self.health.is_some()
        )
    }
}

impl From<PySearchConfig> for RustSearchConfig {
    fn from(config: PySearchConfig) -> Self {
        Self {
            timeout: config.timeout,
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
