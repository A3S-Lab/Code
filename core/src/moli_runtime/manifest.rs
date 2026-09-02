pub const MOLI_REPOSITORY_URL: &str = "https://github.com/lexmount/moli";
pub const DEFAULT_MOLI_VERSION: &str = "1.1.1";

#[cfg(test)]
#[derive(Debug, Clone, serde::Deserialize)]
struct RuntimeManifest {
    schema: String,
    repository: String,
    version: String,
    assets: std::collections::HashMap<String, ManifestAssetOwned>,
}

#[cfg(test)]
#[derive(Debug, Clone, serde::Deserialize)]
struct ManifestAssetOwned {
    archive: String,
    format: String,
    sha256: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ManifestAsset {
    pub(crate) archive: String,
    pub(crate) format: &'static str,
    pub(crate) sha256: &'static str,
}

pub(crate) fn default_version() -> &'static str {
    DEFAULT_MOLI_VERSION
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn current_target() -> Option<&'static str> {
    Some("aarch64-apple-darwin")
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub(crate) fn current_target() -> Option<&'static str> {
    Some("x86_64-apple-darwin")
}

#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
pub(crate) fn current_target() -> Option<&'static str> {
    Some("aarch64-unknown-linux-gnu")
}

#[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
pub(crate) fn current_target() -> Option<&'static str> {
    Some("x86_64-unknown-linux-gnu")
}

#[cfg(all(target_os = "windows", target_arch = "aarch64"))]
pub(crate) fn current_target() -> Option<&'static str> {
    Some("aarch64-pc-windows-msvc")
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub(crate) fn current_target() -> Option<&'static str> {
    Some("x86_64-pc-windows-msvc")
}

#[cfg(not(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"),
    all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"),
    all(target_os = "windows", target_arch = "aarch64"),
    all(target_os = "windows", target_arch = "x86_64")
)))]
pub(crate) fn current_target() -> Option<&'static str> {
    None
}

pub(crate) fn executable_name() -> &'static str {
    #[cfg(windows)]
    {
        "moli.exe"
    }
    #[cfg(not(windows))]
    {
        "moli"
    }
}

pub(crate) fn asset_for(target: &str) -> Option<ManifestAsset> {
    // Keep this table in lockstep with resources/moli-runtime-manifest.json.
    // Static values avoid parsing user-controlled files during runtime.
    let (archive, format, sha256) = match target {
        "aarch64-apple-darwin" => (
            "moli-aarch64-apple-darwin.tar.gz",
            "tar.gz",
            "56deed4634b9c77641ce31f3802b9bb3f32c6d7f28073f73901540429a29864b",
        ),
        "x86_64-apple-darwin" => (
            "moli-x86_64-apple-darwin.tar.gz",
            "tar.gz",
            "bb4f80d6a2786909457a66675ec5cd2118038afaaedeac0d90f9911427d38f56",
        ),
        "aarch64-unknown-linux-gnu" => (
            "moli-aarch64-unknown-linux-gnu.tar.gz",
            "tar.gz",
            "549484765476b8dd3fd93ebf59a089e4424425a961c14874974a88bba6d8b5b4",
        ),
        "x86_64-unknown-linux-gnu" => (
            "moli-x86_64-unknown-linux-gnu.tar.gz",
            "tar.gz",
            "7b3eb9cbbf2cc8bd5ea9ef4a5bdb24cee2df35d26da621216a8b69c2aff3ebaa",
        ),
        "aarch64-pc-windows-msvc" => (
            "moli-aarch64-pc-windows-msvc.zip",
            "zip",
            "1f3031a2ad668bc0b235f543b0559be70dd7649ff6ec36f45a44eedd9a43ece7",
        ),
        "x86_64-pc-windows-msvc" => (
            "moli-x86_64-pc-windows-msvc.zip",
            "zip",
            "ab87af41cca72531bfc08cff068a8bcf3c9f0f53f15dcdd153e1da547191c87c",
        ),
        _ => return None,
    };
    Some(ManifestAsset {
        archive: archive.to_string(),
        format,
        sha256,
    })
}

#[cfg(test)]
pub(crate) fn _manifest_resource_is_parseable() -> Result<(), serde_json::Error> {
    let manifest: RuntimeManifest =
        serde_json::from_str(include_str!("../../resources/moli-runtime-manifest.json"))?;
    debug_assert_eq!(manifest.schema, "a3s-code/moli-runtime-manifest/v1");
    debug_assert_eq!(manifest.repository, MOLI_REPOSITORY_URL);
    debug_assert_eq!(manifest.version, DEFAULT_MOLI_VERSION);
    debug_assert!(!manifest.assets.is_empty());
    for asset in manifest.assets.values() {
        debug_assert!(!asset.archive.is_empty());
        debug_assert!(matches!(asset.format.as_str(), "tar.gz" | "zip"));
        debug_assert_eq!(asset.sha256.len(), 64);
    }
    Ok(())
}
