//! Node.js projection of the Core-managed Moli runtime.

use crate::search_config::HeadlessConfig;

/// Secret-free Moli runtime diagnostics.
#[napi(object)]
pub struct MoliRuntimeInfo {
    pub schema: String,
    pub version: String,
    pub target: Option<String>,
    pub executable: Option<String>,
    pub packaged: bool,
    pub cache_dir: Option<String>,
    pub auto_download: bool,
}

impl From<a3s_code_core::MoliRuntimeInfo> for MoliRuntimeInfo {
    fn from(value: a3s_code_core::MoliRuntimeInfo) -> Self {
        Self {
            schema: value.schema,
            version: value.version,
            target: value.target,
            executable: value.executable,
            packaged: value.packaged,
            cache_dir: value.cache_dir,
            auto_download: value.auto_download,
        }
    }
}

/// Return the current Moli resolution path without downloading anything.
#[napi(js_name = "moliRuntimeInfo")]
pub fn moli_runtime_info(config: Option<HeadlessConfig>) -> MoliRuntimeInfo {
    let config = config.map(Into::into);
    a3s_code_core::moli_runtime_info(config.as_ref()).into()
}

/// Ensure a verified Moli executable is available and return its path.
#[napi(js_name = "ensureMoli")]
pub async fn ensure_moli(config: Option<HeadlessConfig>) -> napi::Result<String> {
    let config = config
        .map(Into::into)
        .unwrap_or_else(a3s_code_core::config::HeadlessConfig::default);
    let timeout = std::time::Duration::from_secs(config.moli_download_timeout_secs);
    let path = a3s_code_core::ensure_moli(&config, timeout)
        .await
        .map_err(|error| napi::Error::from_reason(format!("Moli provisioning failed: {error}")))?;
    Ok(path.to_string_lossy().into_owned())
}

/// Return the pinned Moli release version used by this Code build.
#[napi(js_name = "moliDefaultVersion")]
pub fn moli_default_version() -> String {
    a3s_code_core::default_moli_version().to_owned()
}
