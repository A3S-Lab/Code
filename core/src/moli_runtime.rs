//! Managed Moli runtime for the Code headless search tier.
//!
//! `a3s-search` owns the renderer adapter but intentionally leaves executable
//! provisioning to its host.  This module is that host boundary for A3S Code:
//! it prefers an explicitly configured or packaged sidecar, validates a
//! versioned cache, and only then downloads a pinned release asset with a
//! SHA-256 check.  Installation is atomic and protected by a cross-process
//! lock so concurrent SDK/CLI calls cannot publish a partial executable.

mod manifest;

use crate::config::HeadlessConfig;
use anyhow::{bail, Context, Result};
use fs2::FileExt;
use futures::StreamExt;
use manifest::{asset_for, current_target, default_version, ManifestAsset};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

pub use manifest::{DEFAULT_MOLI_VERSION, MOLI_REPOSITORY_URL};

const CACHE_ENV: &str = "A3S_CODE_MOLI_CACHE_DIR";
const EXECUTABLE_ENV: &str = "A3S_CODE_MOLI_EXECUTABLE";
const RELEASE_BASE_ENV: &str = "A3S_CODE_MOLI_RELEASE_BASE_URL";
const RECEIPT_SCHEMA: &str = "a3s-code/moli-runtime-receipt/v1";
const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_BINARY_BYTES: u64 = 256 * 1024 * 1024;
const LOCK_POLL: Duration = Duration::from_millis(50);

/// Schema identifier for [`MoliRuntimeInfo`].
pub const MOLI_RUNTIME_INFO_SCHEMA_V1: &str = "a3s-code/moli-runtime-info/v1";

/// Secret-free diagnostics for the Moli runtime resolution path.
///
/// The structure is intentionally value-only so SDKs can expose it without
/// sharing the runtime manager's filesystem handles or locks. `executable`
/// is the currently discoverable path (if any); it is not a promise that the
/// browser has been started. Call [`ensure_moli`] before using it for search.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct MoliRuntimeInfo {
    pub schema: String,
    pub version: String,
    pub target: Option<String>,
    pub executable: Option<String>,
    pub packaged: bool,
    pub cache_dir: Option<String>,
    pub auto_download: bool,
}

/// Return the current, secret-free Moli resolution diagnostics.
pub fn moli_runtime_info(config: Option<&HeadlessConfig>) -> MoliRuntimeInfo {
    let fallback = HeadlessConfig::default();
    let config = config.unwrap_or(&fallback);
    let version = config
        .moli_version
        .as_deref()
        .map(|value| value.trim().trim_start_matches('v').to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default_moli_version().to_owned());
    let target = current_target().map(str::to_owned);
    let packaged = packaged_moli();
    let executable = config
        .browser_path
        .as_deref()
        .map(PathBuf::from)
        .filter(|path| is_executable(path))
        .or_else(|| explicit_environment_executable().ok().flatten())
        .or_else(|| packaged.clone())
        .or_else(a3s_search::detect_moli)
        .or_else(|| {
            let target = target.as_deref()?;
            let root = cache_root(config).ok()?;
            let candidate = root
                .join(&version)
                .join(target)
                .join(manifest::executable_name());
            is_executable(&candidate).then_some(candidate)
        })
        .map(|path| path.to_string_lossy().into_owned());
    let cache_dir = cache_root(config)
        .ok()
        .map(|path| path.to_string_lossy().into_owned());
    MoliRuntimeInfo {
        schema: MOLI_RUNTIME_INFO_SCHEMA_V1.to_owned(),
        version,
        target,
        executable,
        packaged: packaged.is_some(),
        cache_dir,
        auto_download: config.auto_download_moli,
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct InstallReceipt {
    schema: String,
    version: String,
    target: String,
    archive_sha256: String,
    binary_sha256: String,
}

/// Return the pinned Moli release used when no version is specified.
pub fn default_moli_version() -> &'static str {
    default_version()
}

/// Ensure a usable Moli executable exists and return its absolute path.
pub async fn ensure_moli(config: &HeadlessConfig, timeout: Duration) -> Result<PathBuf> {
    ensure_moli_from(config, timeout, None, false).await
}

/// Locate a packaged Moli sidecar without consulting the cache or network.
/// Hosts that package a native SDK can use this for diagnostics and tests.
pub fn packaged_moli() -> Option<PathBuf> {
    packaged_candidates()
        .into_iter()
        .find(|path| is_executable(path))
}

async fn ensure_moli_from(
    config: &HeadlessConfig,
    timeout: Duration,
    test_base_url: Option<&str>,
    allow_insecure_test_url: bool,
) -> Result<PathBuf> {
    let deadline = Instant::now() + timeout.max(Duration::from_secs(1));
    // Explicit and packaged runtimes are valid even on targets for which the
    // upstream project does not publish a prebuilt archive (for example musl
    // Linux). Resolve these first so a host-provided executable is never
    // blocked by release-manifest lookup.
    if let Some(raw_path) = config.browser_path.as_deref() {
        let path = PathBuf::from(raw_path);
        return validate_explicit_executable(&path);
    }
    if let Some(path) = explicit_environment_executable()? {
        return Ok(path);
    }
    if let Some(path) = packaged_moli() {
        return Ok(path);
    }

    let target = current_target();
    let managed = if let Some(target) = target {
        // Some targets (currently Linux musl) have no upstream Moli asset.
        // Keep system/explicit discovery usable on those targets instead of
        // failing before the fallback check below. Configuration errors for a
        // target that does have an asset remain fatal and are not swallowed.
        match resolve_release(config, target) {
            Ok((version, expected_sha256, asset)) => {
                let cache_root = cache_root(config)?;
                prepare_cache_layout(&cache_root, &version, target).await?;
                let binary_path = cache_root
                    .join(&version)
                    .join(target)
                    .join(manifest::executable_name());
                let Some(parent) = binary_path.parent() else {
                    bail!("Moli cache binary path has no parent");
                };
                let receipt_path = parent.join("receipt.json");
                if validate_cached(
                    &binary_path,
                    &receipt_path,
                    &version,
                    target,
                    &expected_sha256,
                )
                .await
                {
                    return Ok(binary_path);
                }
                Some((
                    target,
                    version,
                    expected_sha256,
                    asset,
                    cache_root,
                    binary_path,
                    receipt_path,
                ))
            }
            Err(_) if asset_for(target).is_none() => None,
            Err(error) => return Err(error),
        }
    } else {
        None
    };

    // A system-installed Moli is a valid host-owned runtime even when the
    // upstream release does not publish an archive for the current target
    // (notably musl Linux).  Check it after the verified managed cache so a
    // pinned cache remains authoritative when both are available.
    if let Some(path) = a3s_search::detect_moli() {
        return Ok(path);
    }

    let Some((target, version, expected_sha256, asset, cache_root, binary_path, receipt_path)) =
        managed
    else {
        return Err(anyhow::anyhow!(
            "Moli has no prebuilt asset for this target; use an explicit Chrome/Lightpanda backend or provide A3S_CODE_MOLI_EXECUTABLE"
        ));
    };

    if !config.auto_download_moli {
        bail!(
            "Moli is unavailable and auto_download_moli is disabled; install Moli from {MOLI_REPOSITORY_URL} or set {EXECUTABLE_ENV}"
        );
    }

    let _lock = acquire_install_lock(&cache_root, deadline).await?;
    if validate_cached(
        &binary_path,
        &receipt_path,
        &version,
        target,
        &expected_sha256,
    )
    .await
    {
        return Ok(binary_path);
    }

    let base_url = test_base_url
        .map(str::to_string)
        .or_else(|| std::env::var(RELEASE_BASE_ENV).ok())
        .unwrap_or_else(|| format!("{MOLI_REPOSITORY_URL}/releases/download/v{version}"));
    validate_release_base_url(&base_url, allow_insecure_test_url)?;
    install_downloaded(InstallRequest {
        cache_root: &cache_root,
        binary_path: &binary_path,
        receipt_path: &receipt_path,
        version: &version,
        target,
        expected_archive_sha256: &expected_sha256,
        asset,
        base_url: &base_url,
        deadline,
        allow_insecure_test_url,
    })
    .await
}

/// Create and validate the shared cache hierarchy before reading or writing
/// any runtime files. Rejecting symlinked directories prevents a less
/// privileged process from redirecting an installation outside the selected
/// cache root. The root is private because it contains executable code and
/// integrity receipts shared by all local Code processes.
async fn prepare_cache_layout(root: &Path, version: &str, target: &str) -> Result<()> {
    tokio::fs::create_dir_all(root)
        .await
        .with_context(|| format!("create Moli cache {}", root.display()))?;
    validate_cache_directory(root).await?;

    let version_dir = root.join(version);
    tokio::fs::create_dir_all(&version_dir)
        .await
        .with_context(|| format!("create Moli version cache {}", version_dir.display()))?;
    validate_cache_directory(&version_dir).await?;

    let target_dir = version_dir.join(target);
    tokio::fs::create_dir_all(&target_dir)
        .await
        .with_context(|| format!("create Moli target cache {}", target_dir.display()))?;
    validate_cache_directory(&target_dir).await
}

async fn validate_cache_directory(path: &Path) -> Result<()> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .with_context(|| format!("inspect Moli cache directory {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "Moli cache path is not a real directory: {}",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = metadata.permissions();
        if permissions.mode() & 0o077 != 0 {
            permissions.set_mode(0o700);
            tokio::fs::set_permissions(path, permissions)
                .await
                .with_context(|| format!("restrict Moli cache directory {}", path.display()))?;
        }
    }
    Ok(())
}

fn resolve_release(
    config: &HeadlessConfig,
    target: &'static str,
) -> Result<(String, String, ManifestAsset)> {
    let version = config
        .moli_version
        .as_deref()
        .unwrap_or(default_moli_version())
        .trim()
        .trim_start_matches('v')
        .to_string();
    validate_version(&version)?;

    let expected_sha256 = match config.moli_sha256.as_deref() {
        Some(value) => normalize_digest(value)?,
        None if version == default_moli_version() => asset_for(target)
            .map(|asset| asset.sha256.to_string())
            .ok_or_else(|| anyhow::anyhow!("Moli asset metadata is missing for target {target}"))?,
        None => bail!(
            "moli_version={version} must be accompanied by moli_sha256 so the downloaded archive is pinned"
        ),
    };
    let asset = asset_for(target)
        .ok_or_else(|| anyhow::anyhow!("Moli asset metadata is missing for target {target}"))?;
    Ok((version, expected_sha256, asset))
}

fn cache_root(config: &HeadlessConfig) -> Result<PathBuf> {
    let configured = config
        .moli_cache_dir
        .clone()
        .or_else(|| std::env::var_os(CACHE_ENV).map(PathBuf::from))
        .unwrap_or_else(|| {
            dirs::cache_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("a3s-code")
                .join("moli")
        });
    if !configured.is_absolute() {
        bail!(
            "Moli cache directory must be absolute: {}",
            configured.display()
        );
    }
    Ok(configured)
}

fn explicit_environment_executable() -> Result<Option<PathBuf>> {
    let Some(raw) = std::env::var_os(EXECUTABLE_ENV) else {
        return Ok(None);
    };
    let path = resolve_named_path(&raw)
        .ok_or_else(|| anyhow::anyhow!("{EXECUTABLE_ENV} does not identify an executable"))?;
    Ok(Some(path))
}

fn packaged_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("A3S_CODE_MOLI_PATH") {
        candidates.push(PathBuf::from(path));
    }
    if let Some(directory) = std::env::var_os("A3S_CODE_MOLI_DIR") {
        let directory = PathBuf::from(directory);
        candidates.push(directory.join(manifest::executable_name()));
    }
    if let Ok(executable) = std::env::current_exe() {
        let mut roots = Vec::new();
        if let Some(parent) = executable.parent() {
            roots.push(parent.to_path_buf());
            if let Some(grandparent) = parent.parent() {
                roots.push(grandparent.to_path_buf());
                roots.push(grandparent.join("Resources"));
            }
        }
        for root in roots {
            candidates.extend([
                root.join(manifest::executable_name()),
                root.join("moli").join(manifest::executable_name()),
                root.join("resources").join(manifest::executable_name()),
                root.join("resources")
                    .join("moli")
                    .join(manifest::executable_name()),
            ]);
        }
    }
    if let Ok(directory) = std::env::current_dir() {
        candidates.extend([
            directory.join(manifest::executable_name()),
            directory.join("moli").join(manifest::executable_name()),
        ]);
    }
    let mut unique = Vec::new();
    for path in candidates {
        if !unique.iter().any(|current: &PathBuf| current == &path) {
            unique.push(path);
        }
    }
    unique
}

fn resolve_named_path(value: &std::ffi::OsStr) -> Option<PathBuf> {
    let path = Path::new(value);
    if path.is_absolute() || path.components().count() > 1 {
        return is_executable(path).then(|| path.to_path_buf());
    }
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|directory| directory.join(value))
        .find(|candidate| is_executable(candidate))
}

fn validate_explicit_executable(path: &Path) -> Result<PathBuf> {
    if is_executable(path) {
        Ok(path.to_path_buf())
    } else {
        bail!(
            "configured Moli executable is missing or not executable: {}",
            path.display()
        )
    }
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn validate_version(version: &str) -> Result<()> {
    if version.is_empty()
        || version.len() > 64
        || !version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        bail!("invalid Moli version `{version}`");
    }
    Ok(())
}

fn normalize_digest(value: &str) -> Result<String> {
    let value = value.trim();
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("moli_sha256 must contain exactly 64 hexadecimal characters");
    }
    Ok(value.to_ascii_lowercase())
}

fn validate_release_base_url(value: &str, allow_insecure_test_url: bool) -> Result<()> {
    let parsed = reqwest::Url::parse(value).with_context(|| "invalid Moli release base URL")?;
    if parsed.username() != "" || parsed.password().is_some() || parsed.host_str().is_none() {
        bail!("Moli release base URL must not contain credentials and must include a host");
    }
    if parsed.scheme() != "https" && !(allow_insecure_test_url && parsed.scheme() == "http") {
        bail!("Moli release base URL must use https");
    }
    Ok(())
}

async fn validate_cached(
    binary_path: &Path,
    receipt_path: &Path,
    version: &str,
    target: &str,
    expected_archive_sha256: &str,
) -> bool {
    if symlink_or_missing(binary_path).await || symlink_or_missing(receipt_path).await {
        return false;
    }
    if !is_executable(binary_path) {
        return false;
    }
    let Ok(metadata) = tokio::fs::metadata(binary_path).await else {
        return false;
    };
    if metadata.len() == 0 || metadata.len() > MAX_BINARY_BYTES {
        return false;
    }
    let Ok(bytes) = tokio::fs::read(receipt_path).await else {
        return false;
    };
    let Ok(receipt) = serde_json::from_slice::<InstallReceipt>(&bytes) else {
        return false;
    };
    if receipt.schema != RECEIPT_SCHEMA
        || receipt.version != version
        || receipt.target != target
        || receipt.archive_sha256 != expected_archive_sha256
    {
        return false;
    }
    hash_file(binary_path)
        .await
        .is_ok_and(|digest| digest == receipt.binary_sha256)
}

async fn symlink_or_missing(path: &Path) -> bool {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata.file_type().is_symlink(),
        Err(_) => true,
    }
}

async fn hash_file(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("open {} for SHA-256", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 128 * 1024];
    loop {
        let read = tokio::io::AsyncReadExt::read(&mut file, &mut buffer)
            .await
            .with_context(|| format!("hash {}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

async fn acquire_install_lock(root: &Path, deadline: Instant) -> Result<std::fs::File> {
    let path = root.join(".install.lock");
    let path_for_open = path.clone();
    let file = tokio::task::spawn_blocking(move || {
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path_for_open)
            .with_context(|| format!("open Moli install lock {}", path_for_open.display()))
    })
    .await
    .context("Moli install-lock worker failed")??;

    loop {
        if Instant::now() >= deadline {
            bail!("timed out waiting for the Moli install lock");
        }
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            // `fs2` exposes the platform's native contention error.  Unix
            // normally maps it to `WouldBlock`, while Windows reports
            // `ERROR_LOCK_VIOLATION` as `PermissionDenied`; compare the
            // canonical fs2 error as well so waiters never fail spuriously
            // on Windows when another Code process owns the lock.
            Err(error) if is_lock_contended(&error) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                tokio::time::sleep(LOCK_POLL.min(remaining)).await;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("acquire Moli install lock {}", path.display()))
            }
        }
    }
}

fn is_lock_contended(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    fs2::lock_contended_error()
        .raw_os_error()
        .is_some_and(|code| error.raw_os_error() == Some(code))
}

struct InstallRequest<'a> {
    cache_root: &'a Path,
    binary_path: &'a Path,
    receipt_path: &'a Path,
    version: &'a str,
    target: &'a str,
    expected_archive_sha256: &'a str,
    asset: ManifestAsset,
    base_url: &'a str,
    deadline: Instant,
    allow_insecure_test_url: bool,
}

async fn install_downloaded(request: InstallRequest<'_>) -> Result<PathBuf> {
    let InstallRequest {
        cache_root,
        binary_path,
        receipt_path,
        version,
        target,
        expected_archive_sha256,
        asset,
        base_url,
        deadline,
        allow_insecure_test_url,
    } = request;
    let Some(parent) = binary_path.parent() else {
        bail!("Moli cache binary path has no parent");
    };
    let target_dir = parent.to_path_buf();
    tokio::fs::create_dir_all(&target_dir)
        .await
        .with_context(|| format!("create Moli target directory {}", target_dir.display()))?;

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let archive_path = cache_root.join(format!(".moli-download-{suffix}.part"));
    let stage_path = target_dir.join(format!(".moli-stage-{suffix}"));
    let result = async {
        let url = format!("{}/{}", base_url.trim_end_matches('/'), asset.archive);
        download_archive(
            &url,
            expected_archive_sha256,
            &archive_path,
            deadline,
            allow_insecure_test_url,
        )
        .await?;
        let stage_for_extract = stage_path.clone();
        let archive_for_extract = archive_path.clone();
        let format = asset.format;
        tokio::task::spawn_blocking(move || {
            extract_binary(&archive_for_extract, &stage_for_extract, format)
        })
        .await
        .context("Moli archive extraction worker failed")??;
        if !is_executable(&stage_path) {
            bail!("extracted Moli binary is not executable");
        }
        let binary_sha256 = hash_file(&stage_path).await?;
        let receipt = InstallReceipt {
            schema: RECEIPT_SCHEMA.to_string(),
            version: version.to_string(),
            target: target.to_string(),
            archive_sha256: expected_archive_sha256.to_string(),
            binary_sha256,
        };

        let binary_for_publish = binary_path.to_path_buf();
        let stage_for_publish = stage_path.clone();
        tokio::task::spawn_blocking(move || {
            if binary_for_publish.exists() {
                std::fs::remove_file(&binary_for_publish).with_context(|| {
                    format!(
                        "replace stale Moli executable {}",
                        binary_for_publish.display()
                    )
                })?;
            }
            std::fs::rename(&stage_for_publish, &binary_for_publish).with_context(|| {
                format!(
                    "atomically publish Moli executable {}",
                    binary_for_publish.display()
                )
            })?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("Moli executable publication worker failed")??;

        let receipt_bytes = serde_json::to_vec_pretty(&receipt).context("encode Moli receipt")?;
        let receipt_tmp = receipt_path.with_extension(format!("json-{suffix}.tmp"));
        tokio::fs::write(&receipt_tmp, receipt_bytes)
            .await
            .with_context(|| format!("write Moli receipt {}", receipt_tmp.display()))?;
        let receipt_for_rename = receipt_path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            if receipt_for_rename.exists() {
                std::fs::remove_file(&receipt_for_rename).with_context(|| {
                    format!(
                        "replace stale Moli receipt {}",
                        receipt_for_rename.display()
                    )
                })?;
            }
            std::fs::rename(&receipt_tmp, &receipt_for_rename).with_context(|| {
                format!(
                    "atomically publish Moli receipt {}",
                    receipt_for_rename.display()
                )
            })?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("Moli receipt publication worker failed")??;

        #[cfg(unix)]
        {
            let directory = target_dir.clone();
            tokio::task::spawn_blocking(move || std::fs::File::open(directory)?.sync_all())
                .await
                .context("Moli directory sync worker failed")??;
        }
        Ok::<_, anyhow::Error>(binary_path.to_path_buf())
    }
    .await;

    let _ = tokio::fs::remove_file(&archive_path).await;
    let _ = tokio::fs::remove_file(&stage_path).await;
    result
}

async fn download_archive(
    url: &str,
    expected_sha256: &str,
    destination: &Path,
    deadline: Instant,
    allow_insecure_test_url: bool,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(3))
        // HTTP is enabled only for in-process wiremock tests. Production
        // callers always pass false and therefore get an HTTPS-only client.
        .https_only(!allow_insecure_test_url)
        .build()
        .context("build secure Moli download client")?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        bail!("Moli download deadline expired before starting");
    }
    let response = tokio::time::timeout(remaining, client.get(url).send())
        .await
        .context("Moli download request timed out")?
        .context("Moli download request failed")?;
    if !response.status().is_success() {
        bail!("Moli download returned HTTP {}", response.status());
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ARCHIVE_BYTES)
    {
        bail!(
            "Moli archive exceeds the {} MiB limit",
            MAX_ARCHIVE_BYTES / 1024 / 1024
        );
    }
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(destination)
        .await
        .with_context(|| format!("create Moli archive {}", destination.display()))?;
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    while let Some(chunk) = tokio::time::timeout(
        deadline.saturating_duration_since(Instant::now()),
        stream.next(),
    )
    .await
    .context("Moli archive response timed out")?
    {
        let chunk = chunk.context("read Moli archive response")?;
        total = total.saturating_add(chunk.len() as u64);
        if total > MAX_ARCHIVE_BYTES {
            bail!(
                "Moli archive exceeds the {} MiB limit",
                MAX_ARCHIVE_BYTES / 1024 / 1024
            );
        }
        tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            file.write_all(&chunk),
        )
        .await
        .context("writing Moli archive timed out")?
        .context("write Moli archive")?;
        digest.update(&chunk);
        if Instant::now() >= deadline {
            bail!("Moli download timed out");
        }
    }
    file.sync_all().await.context("sync Moli archive")?;
    let actual = format!("{:x}", digest.finalize());
    if actual != expected_sha256 {
        bail!("Moli archive SHA-256 mismatch: expected {expected_sha256}, got {actual}");
    }
    Ok(())
}

fn extract_binary(archive_path: &Path, destination: &Path, format: &str) -> Result<()> {
    if destination.exists() {
        std::fs::remove_file(destination)
            .with_context(|| format!("remove stale Moli staging file {}", destination.display()))?;
    }
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Moli staging path has no parent"))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create Moli staging directory {}", parent.display()))?;
    match format {
        "tar.gz" => extract_tar_gz(archive_path, destination)?,
        "zip" => extract_zip(archive_path, destination)?,
        other => bail!("unsupported Moli archive format `{other}`"),
    }
    set_executable(destination)?;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(destination)
        .with_context(|| format!("open extracted Moli {}", destination.display()))?
        .sync_all()
        .context("sync extracted Moli binary")?;
    Ok(())
}

fn extract_tar_gz(archive_path: &Path, destination: &Path) -> Result<()> {
    let file = std::fs::File::open(archive_path)
        .with_context(|| format!("open Moli archive {}", archive_path.display()))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let mut found = false;
    for entry in archive.entries().context("read Moli tar entries")? {
        let mut entry = entry.context("read Moli tar entry")?;
        let path = entry
            .path()
            .context("read Moli tar member path")?
            .into_owned();
        let Some(name) = validate_member_name(&path)? else {
            continue;
        };
        if name != manifest::executable_name() {
            continue;
        }
        if !entry.header().entry_type().is_file() || found {
            bail!("Moli archive contains an invalid or duplicate executable member");
        }
        copy_bounded(&mut entry, destination)?;
        found = true;
    }
    if !found {
        bail!(
            "Moli archive does not contain {}",
            manifest::executable_name()
        );
    }
    Ok(())
}

fn extract_zip(archive_path: &Path, destination: &Path) -> Result<()> {
    let file = std::fs::File::open(archive_path)
        .with_context(|| format!("open Moli archive {}", archive_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("read Moli zip archive")?;
    let mut found = false;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).context("read Moli zip entry")?;
        let path = entry
            .enclosed_name()
            .ok_or_else(|| anyhow::anyhow!("Moli zip contains a traversal path"))?
            .to_path_buf();
        let Some(name) = validate_member_name(&path)? else {
            continue;
        };
        if name != manifest::executable_name() {
            continue;
        }
        if !entry.is_file() || found {
            bail!("Moli zip contains an invalid or duplicate executable member");
        }
        copy_bounded(&mut entry, destination)?;
        found = true;
    }
    if !found {
        bail!(
            "Moli archive does not contain {}",
            manifest::executable_name()
        );
    }
    Ok(())
}

fn validate_member_name(path: &Path) -> Result<Option<&str>> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        bail!("Moli archive contains a traversal path: {}", path.display());
    }
    Ok(path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| *name == manifest::executable_name()))
}

fn copy_bounded(reader: &mut impl Read, destination: &Path) -> Result<()> {
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .with_context(|| format!("create Moli staging binary {}", destination.display()))?;
    let mut limited = reader.take(MAX_BINARY_BYTES.saturating_add(1));
    let copied = std::io::copy(&mut limited, &mut output).context("extract Moli executable")?;
    if copied > MAX_BINARY_BYTES {
        bail!(
            "Moli executable exceeds the {} MiB limit",
            MAX_BINARY_BYTES / 1024 / 1024
        );
    }
    output.flush().context("flush extracted Moli executable")?;
    Ok(())
}

fn set_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_config(cache: &TempDir, version: &str, sha256: &str) -> HeadlessConfig {
        HeadlessConfig {
            browser_path: None,
            auto_download_moli: true,
            moli_version: Some(version.to_string()),
            moli_sha256: Some(sha256.to_string()),
            moli_cache_dir: Some(cache.path().join("moli")),
            ..HeadlessConfig::default()
        }
    }

    fn fixture_archive() -> (Vec<u8>, &'static str) {
        let mut bytes: Vec<u8> = Vec::new();
        #[cfg(not(windows))]
        {
            let encoder = flate2::write::GzEncoder::new(&mut bytes, flate2::Compression::fast());
            let mut builder = tar::Builder::new(encoder);
            let content = b"#!/bin/sh\nprintf '<html></html>\\n'\n";
            let mut header = tar::Header::new_gnu();
            header.set_path("moli-v-test-target/moli").unwrap();
            header.set_size(content.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append(&header, &content[..]).unwrap();
            let encoder = builder.into_inner().unwrap();
            encoder.finish().unwrap();
            (bytes, "tar.gz")
        }
        #[cfg(windows)]
        {
            let cursor = std::io::Cursor::new(Vec::new());
            let mut archive = zip::ZipWriter::new(cursor);
            let options = zip::write::FileOptions::default();
            archive
                .start_file("moli-v-test-target/moli.exe", options)
                .unwrap();
            archive.write_all(b"fixture moli").unwrap();
            (archive.finish().unwrap().into_inner(), "zip")
        }
    }

    fn fixture_digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    #[test]
    fn embedded_manifest_has_supported_default_asset() {
        manifest::_manifest_resource_is_parseable().expect("embedded Moli manifest");
        let target = current_target().expect("tests run on a supported release target");
        let asset = asset_for(target).expect("manifest asset");
        assert_eq!(default_moli_version(), DEFAULT_MOLI_VERSION);
        assert_eq!(asset.sha256.len(), 64);
        assert!(asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn custom_version_requires_a_digest() {
        let config = HeadlessConfig {
            moli_version: Some("9.9.9".to_string()),
            moli_sha256: None,
            ..HeadlessConfig::default()
        };
        let error = resolve_release(&config, current_target().unwrap()).unwrap_err();
        assert!(error.to_string().contains("moli_sha256"));
    }

    #[test]
    fn relative_cache_directory_is_rejected() {
        let config = HeadlessConfig {
            moli_cache_dir: Some(PathBuf::from("relative/moli")),
            ..HeadlessConfig::default()
        };
        let error = cache_root(&config).unwrap_err();
        assert!(error.to_string().contains("absolute"));
    }

    #[test]
    fn default_configs_share_one_cache_root() {
        let first = cache_root(&HeadlessConfig::default()).expect("default cache root");
        let second = cache_root(&HeadlessConfig::default()).expect("default cache root");
        assert_eq!(
            first, second,
            "all Code processes must converge on one cache"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_cache_root_is_rejected() {
        let parent = tempfile::tempdir().unwrap();
        let real = parent.path().join("real");
        let link = parent.path().join("moli");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let error = prepare_cache_layout(&link, "1.1.1", "test-target")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("real directory"));
    }

    #[test]
    fn traversal_member_is_not_accepted() {
        assert!(validate_member_name(Path::new("../../moli")).is_err());
        let valid_member = if cfg!(windows) {
            Path::new("moli-v1/moli.exe")
        } else {
            Path::new("moli-v1/moli")
        };
        assert_eq!(
            validate_member_name(valid_member).unwrap(),
            Some(manifest::executable_name())
        );
    }

    #[test]
    fn lock_contention_recognizes_platform_error() {
        assert!(is_lock_contended(&fs2::lock_contended_error()));
        assert!(is_lock_contended(&std::io::Error::from(
            std::io::ErrorKind::WouldBlock
        )));
        assert!(!is_lock_contended(&std::io::Error::other(
            "unrelated installation error"
        )));
    }

    #[tokio::test]
    async fn downloads_verifies_and_reuses_the_atomic_cache() {
        let server = MockServer::start().await;
        let (archive, format) = fixture_archive();
        let digest = fixture_digest(&archive);
        let target = current_target().unwrap();
        let asset_name = format!(
            "moli-{target}.{}",
            if format == "zip" { "zip" } else { "tar.gz" }
        );
        Mock::given(method("GET"))
            .and(wiremock::matchers::path(format!("/{asset_name}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(archive.clone()))
            .expect(1)
            .mount(&server)
            .await;
        let cache = tempfile::tempdir().unwrap();
        let config = test_config(&cache, "9.9.9", &digest);
        let first = ensure_moli_from(&config, Duration::from_secs(10), Some(&server.uri()), true)
            .await
            .unwrap();
        assert!(is_executable(&first));
        assert_eq!(
            tokio::fs::read(&first).await.unwrap(),
            if cfg!(windows) {
                b"fixture moli".to_vec()
            } else {
                b"#!/bin/sh\nprintf '<html></html>\\n'\n".to_vec()
            }
        );
        let second = ensure_moli_from(&config, Duration::from_secs(10), Some(&server.uri()), true)
            .await
            .unwrap();
        assert_eq!(first, second);
        server.verify().await;
    }

    #[tokio::test]
    async fn concurrent_first_use_downloads_once() {
        let server = MockServer::start().await;
        let (archive, format) = fixture_archive();
        let digest = fixture_digest(&archive);
        let target = current_target().unwrap();
        let asset_name = format!(
            "moli-{target}.{}",
            if format == "zip" { "zip" } else { "tar.gz" }
        );
        Mock::given(method("GET"))
            .and(wiremock::matchers::path(format!("/{asset_name}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(archive))
            .expect(1)
            .mount(&server)
            .await;
        let cache = tempfile::tempdir().unwrap();
        let config = Arc::new(test_config(&cache, "9.9.8", &digest));
        let mut tasks = Vec::new();
        for _ in 0..4 {
            let config = Arc::clone(&config);
            let base = server.uri();
            tasks.push(tokio::spawn(async move {
                ensure_moli_from(&config, Duration::from_secs(10), Some(&base), true).await
            }));
        }
        for task in tasks {
            assert!(task.await.unwrap().is_ok());
        }
        server.verify().await;
    }
}
