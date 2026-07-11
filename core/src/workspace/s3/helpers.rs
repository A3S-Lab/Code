//! Pure S3 path and response-validation helpers.

use crate::workspace::WorkspacePath;
use anyhow::{anyhow, Result};

pub(super) fn join_workspace_path(base: &WorkspacePath, rel: &str) -> WorkspacePath {
    if base.is_root() {
        WorkspacePath::from_normalized(rel)
    } else {
        WorkspacePath::from_normalized(format!("{}/{}", base.as_str(), rel))
    }
}

/// Last segment of a slash-separated key, used to apply filename-only glob
/// filters in `grep` (matches `ignore::types` semantics from the local
/// backend: a pattern without `/` is treated as a basename match).
pub(super) fn basename(rel: &str) -> &str {
    rel.rsplit_once('/').map_or(rel, |(_, b)| b)
}

/// Decide whether a `GetObject` response is safe to buffer fully into memory.
///
/// Returns `Ok(())` when the advertised `Content-Length` is non-negative and
/// within `max_bytes`. Rejects in three cases:
/// * `Content-Length` was not advertised (we refuse to read without a size
///   guard rather than risking OOM);
/// * the advertised length is negative (protocol violation);
/// * the advertised length exceeds `max_bytes`.
///
/// Extracted as a free function so it can be unit-tested without standing up
/// an `aws_sdk_s3::Client`.
pub(super) fn validate_content_length(
    advertised: Option<i64>,
    max_bytes: u64,
    bucket: &str,
    key: &str,
) -> Result<()> {
    match advertised {
        Some(n) if n < 0 => Err(anyhow!(
            "S3 object s3://{}/{} reported invalid content-length {}",
            bucket,
            key,
            n
        )),
        Some(n) if (n as u64) > max_bytes => Err(anyhow!(
            "S3 object s3://{}/{} is {} bytes, exceeds workspace max_read_bytes ({}); \
             raise S3BackendConfig::max_read_bytes if the read is legitimate",
            bucket,
            key,
            n,
            max_bytes
        )),
        Some(_) => Ok(()),
        None => Err(anyhow!(
            "S3 object s3://{}/{} did not report Content-Length; refusing to read \
             without a size guard. Check that the endpoint is S3-compliant.",
            bucket,
            key
        )),
    }
}

pub(super) fn normalize_prefix(prefix: &str) -> String {
    prefix
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

pub(super) fn strip_dir_name(common_prefix: &str, listing_prefix: &str) -> Option<String> {
    let remainder = common_prefix.strip_prefix(listing_prefix)?;
    let trimmed = remainder.trim_end_matches('/');
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(super) fn strip_file_name(key: &str, listing_prefix: &str) -> Option<String> {
    let remainder = key.strip_prefix(listing_prefix)?;
    if remainder.is_empty() || remainder.contains('/') {
        None
    } else {
        Some(remainder.to_string())
    }
}
