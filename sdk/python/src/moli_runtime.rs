//! Python projection of the Core-managed Moli runtime.

use super::*;

/// Return Moli resolution diagnostics without downloading anything.
#[pyfunction(name = "moli_runtime_info", signature = (config=None))]
pub(super) fn py_moli_runtime_info(
    py: Python<'_>,
    config: Option<PyHeadlessConfig>,
) -> PyResult<PyObject> {
    let config = config.map(Into::into);
    let info = a3s_code_core::moli_runtime_info(config.as_ref());
    let json = serde_json::to_string(&info)
        .map_err(|error| PyRuntimeError::new_err(format!("Failed to encode Moli info: {error}")))?;
    json_string_to_py(py, &json)
}

/// Ensure a verified Moli executable is available and return its path.
#[pyfunction(name = "ensure_moli", signature = (config=None))]
pub(super) fn py_ensure_moli(
    py: Python<'_>,
    config: Option<PyHeadlessConfig>,
) -> PyResult<String> {
    let config = config
        .map(Into::into)
        .unwrap_or_else(a3s_code_core::config::HeadlessConfig::default);
    let timeout = std::time::Duration::from_secs(config.moli_download_timeout_secs);
    let path = py
        .allow_threads(move || get_runtime().block_on(a3s_code_core::ensure_moli(&config, timeout)))
        .map_err(|error| PyRuntimeError::new_err(format!("Moli provisioning failed: {error}")))?;
    Ok(path.to_string_lossy().into_owned())
}

/// Return the pinned Moli release version used by this Code build.
#[pyfunction(name = "moli_default_version")]
pub(super) fn py_moli_default_version() -> String {
    a3s_code_core::default_moli_version().to_owned()
}

