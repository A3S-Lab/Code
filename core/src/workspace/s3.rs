//! S3-compatible object-storage workspace backend.
//!
//! [`S3WorkspaceBackend`] implements [`WorkspaceFileSystem`] against any
//! S3-compatible endpoint (AWS S3, MinIO, Cloudflare R2, Backblaze B2, ...)
//! using the AWS Rust SDK. The backend deliberately does **not** implement
//! [`WorkspaceCommandRunner`], [`WorkspaceSearch`], or any of the git
//! provider traits — object storage cannot natively service those operations,
//! and capability gating prevents the corresponding tools (`bash`, `grep`,
//! `glob`, `git`) from being registered when the backend is in use.
//!
//! Path semantics are lexical (no host filesystem involved), inherited from
//! [`super::VirtualPathResolver`]: paths are relative, parent-directory
//! traversal is rejected, and absolute or Windows-style paths are refused.
//!
//! # Concurrency caveats
//!
//! S3 does not provide atomic rename or read-modify-write. Tools like `edit`
//! and `patch` perform a `read_text` then `write_text` — concurrent writers
//! to the same key will overwrite each other (last-writer-wins). Callers
//! that need stronger guarantees should partition workspaces per session.
//!
//! # Memory bounds
//!
//! [`S3WorkspaceBackend::read_text`] enforces a `max_read_bytes` ceiling
//! (default [`DEFAULT_MAX_READ_BYTES`]) by inspecting `Content-Length` on the
//! `GetObject` response before consuming the body. Oversized objects are
//! rejected with a clear error and never buffered into memory. Override the
//! limit via [`S3BackendConfig::max_read_bytes`] when reading larger text
//! artifacts is legitimate.
//!
//! Available only when the `s3` feature is enabled.

use super::{
    escape_control_chars_for_display, validate_relative_pattern, WorkspaceDirEntry, WorkspaceError,
    WorkspaceFileSystem, WorkspaceFileSystemExt, WorkspaceFileType, WorkspaceGlobRequest,
    WorkspaceGlobResult, WorkspaceGrepOutcome, WorkspaceGrepRequest, WorkspaceGrepResult,
    WorkspacePath, WorkspaceResult, WorkspaceSearch, WorkspaceVersionConflict,
    WorkspaceWriteOutcome,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use aws_credential_types::Credentials;
use aws_sdk_s3::config::{BehaviorVersion, Region};
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::list_objects_v2::ListObjectsV2Error;
use aws_sdk_s3::operation::put_object::PutObjectError;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use std::sync::Arc;
use std::time::Duration;

mod config;
mod helpers;

use config::DEFAULT_REGION;
pub use config::{
    S3BackendConfig, DEFAULT_MAX_GREP_BYTES_PER_OBJECT, DEFAULT_MAX_OBJECTS_SCANNED,
    DEFAULT_MAX_READ_BYTES, DEFAULT_SEARCH_CONCURRENCY,
};
use helpers::*;

/// S3-compatible workspace backend.
///
/// Construct with [`Self::new`] for production, or [`Self::with_client`] for
/// tests that need to inject a pre-built [`aws_sdk_s3::Client`] (e.g. with a
/// mock HTTP layer).
#[derive(Debug, Clone)]
pub struct S3WorkspaceBackend {
    client: Client,
    bucket: String,
    /// Normalised prefix without trailing slash. Empty string means
    /// "bucket root is the workspace".
    prefix: String,
    /// Per-read size ceiling (bytes). Enforced via `Content-Length`
    /// inspection before the body is consumed.
    max_read_bytes: u64,
    /// When `true` the backend implements [`WorkspaceSearch`]; otherwise the
    /// `grep` / `glob` tools are gated off by capability registration.
    search_enabled: bool,
    /// Upper bound on objects considered per search call.
    max_objects_scanned: usize,
    /// Per-object body-size ceiling for `grep` downloads.
    max_grep_bytes_per_object: u64,
    /// Concurrent object downloads during `grep`.
    search_concurrency: usize,
}

impl S3WorkspaceBackend {
    /// Build a backend from declarative configuration.
    pub fn new(config: S3BackendConfig) -> Self {
        let credentials = Credentials::new(
            config.access_key_id,
            config.secret_access_key,
            config.session_token,
            None,
            "a3s-code-static",
        );

        let mut builder = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(
                config.region.unwrap_or_else(|| DEFAULT_REGION.to_string()),
            ))
            .credentials_provider(credentials)
            .force_path_style(config.force_path_style);

        if let Some(endpoint) = config.endpoint {
            builder = builder.endpoint_url(endpoint);
        }

        let client = Client::from_conf(builder.build());
        Self::with_client(client, config.bucket, config.prefix)
            .with_max_read_bytes(config.max_read_bytes.unwrap_or(DEFAULT_MAX_READ_BYTES))
            .with_search_enabled(config.search_enabled)
            .with_max_objects_scanned(
                config
                    .max_objects_scanned
                    .unwrap_or(DEFAULT_MAX_OBJECTS_SCANNED),
            )
            .with_max_grep_bytes_per_object(
                config
                    .max_grep_bytes_per_object
                    .unwrap_or(DEFAULT_MAX_GREP_BYTES_PER_OBJECT),
            )
            .with_search_concurrency(
                config
                    .search_concurrency
                    .unwrap_or(DEFAULT_SEARCH_CONCURRENCY),
            )
    }

    /// Build a backend from a pre-configured S3 client. Intended for tests
    /// and advanced use cases (custom retries, signer overrides, http_client
    /// injection, etc.).
    pub fn with_client(
        client: Client,
        bucket: impl Into<String>,
        prefix: impl Into<String>,
    ) -> Self {
        Self {
            client,
            bucket: bucket.into(),
            prefix: normalize_prefix(&prefix.into()),
            max_read_bytes: DEFAULT_MAX_READ_BYTES,
            search_enabled: false,
            max_objects_scanned: DEFAULT_MAX_OBJECTS_SCANNED,
            max_grep_bytes_per_object: DEFAULT_MAX_GREP_BYTES_PER_OBJECT,
            search_concurrency: DEFAULT_SEARCH_CONCURRENCY,
        }
    }

    /// Override the per-read size ceiling. Passing `0` falls back to
    /// [`DEFAULT_MAX_READ_BYTES`] — a zero ceiling would make every read
    /// fail and is treated as a configuration mistake.
    pub fn with_max_read_bytes(mut self, bytes: u64) -> Self {
        self.max_read_bytes = if bytes == 0 {
            DEFAULT_MAX_READ_BYTES
        } else {
            bytes
        };
        self
    }

    /// Active per-read size ceiling in bytes.
    pub fn max_read_bytes(&self) -> u64 {
        self.max_read_bytes
    }

    /// Enable or disable degraded `grep` / `glob` against this backend.
    /// See [`S3BackendConfig::search_enabled`] for cost trade-offs.
    pub fn with_search_enabled(mut self, enabled: bool) -> Self {
        self.search_enabled = enabled;
        self
    }

    /// Whether this backend exposes [`WorkspaceSearch`].
    pub fn search_enabled(&self) -> bool {
        self.search_enabled
    }

    /// Override the per-search object-scan ceiling. `0` resets to default.
    pub fn with_max_objects_scanned(mut self, n: usize) -> Self {
        self.max_objects_scanned = if n == 0 {
            DEFAULT_MAX_OBJECTS_SCANNED
        } else {
            n
        };
        self
    }

    /// Active per-search object-scan ceiling.
    pub fn max_objects_scanned(&self) -> usize {
        self.max_objects_scanned
    }

    /// Override the per-object body-size ceiling for `grep`. `0` resets to default.
    pub fn with_max_grep_bytes_per_object(mut self, bytes: u64) -> Self {
        self.max_grep_bytes_per_object = if bytes == 0 {
            DEFAULT_MAX_GREP_BYTES_PER_OBJECT
        } else {
            bytes
        };
        self
    }

    /// Active per-object body-size ceiling for `grep` downloads.
    pub fn max_grep_bytes_per_object(&self) -> u64 {
        self.max_grep_bytes_per_object
    }

    /// Override the per-search download concurrency. `0` resets to default.
    pub fn with_search_concurrency(mut self, n: usize) -> Self {
        self.search_concurrency = if n == 0 {
            DEFAULT_SEARCH_CONCURRENCY
        } else {
            n
        };
        self
    }

    /// Active per-search download concurrency.
    pub fn search_concurrency(&self) -> usize {
        self.search_concurrency
    }

    /// The bucket this backend is bound to.
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// The workspace prefix inside the bucket (no leading or trailing slash).
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Underlying AWS SDK client — exposed for advanced workflows that need
    /// to perform out-of-band operations (e.g. presigned URLs, ACL changes).
    pub fn client(&self) -> &Client {
        &self.client
    }

    fn key_for(&self, path: &WorkspacePath) -> String {
        if path.is_root() {
            self.prefix.clone()
        } else if self.prefix.is_empty() {
            path.as_str().to_string()
        } else {
            format!("{}/{}", self.prefix, path.as_str())
        }
    }

    fn list_prefix_for(&self, path: &WorkspacePath) -> String {
        if path.is_root() {
            if self.prefix.is_empty() {
                String::new()
            } else {
                format!("{}/", self.prefix)
            }
        } else if self.prefix.is_empty() {
            format!("{}/", path.as_str())
        } else {
            format!("{}/{}/", self.prefix, path.as_str())
        }
    }

    /// Shared GET path used by both [`WorkspaceFileSystem::read_text`] and
    /// [`WorkspaceFileSystemExt::read_text_with_version`].
    ///
    /// Returns `(content, etag)`. The ETag is the opaque version token used
    /// by compare-and-swap writes. Refuses responses without an ETag — every
    /// S3-compatible service must return one for a successful GET; absence
    /// indicates a misconfigured endpoint.
    async fn get_object_text(&self, path: &WorkspacePath) -> WorkspaceResult<(String, String)> {
        let key = self.key_for(path);
        let start = std::time::Instant::now();
        let send_result = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await;
        emit_s3_call_event(
            "s3.get_object",
            &self.bucket,
            &key,
            send_result
                .as_ref()
                .ok()
                .and_then(|r| r.content_length())
                .unwrap_or(0)
                .max(0) as u64,
            send_result.is_ok(),
            start.elapsed(),
        );
        let resp = send_result.map_err(|e| classify_get_error(&self.bucket, &key, e))?;

        validate_content_length(
            resp.content_length(),
            self.max_read_bytes,
            &self.bucket,
            &key,
        )?;

        let etag = resp
            .e_tag()
            .map(|s| s.to_string())
            .ok_or_else(|| {
                anyhow!(
                    "S3 object s3://{}/{} returned no ETag; cannot use compare-and-swap writes against this endpoint",
                    self.bucket,
                    key
                )
            })?;

        let bytes = resp
            .body
            .collect()
            .await
            .map_err(|e| {
                anyhow!(
                    "Failed to read S3 object body s3://{}/{}: {}",
                    self.bucket,
                    key,
                    e
                )
            })?
            .into_bytes();

        let content = String::from_utf8(bytes.to_vec()).map_err(|e| {
            anyhow!(
                "S3 object s3://{}/{} is not valid UTF-8: {}",
                self.bucket,
                key,
                e
            )
        })?;

        Ok((content, etag))
    }
}

#[async_trait]
impl WorkspaceFileSystem for S3WorkspaceBackend {
    async fn read_text(&self, path: &WorkspacePath) -> WorkspaceResult<String> {
        let (content, _etag) = self.get_object_text(path).await?;
        Ok(content)
    }

    async fn write_text(
        &self,
        path: &WorkspacePath,
        content: &str,
    ) -> WorkspaceResult<WorkspaceWriteOutcome> {
        let key = self.key_for(path);
        let body = ByteStream::from(content.as_bytes().to_vec());
        let bytes = content.len() as u64;

        let start = std::time::Instant::now();
        let send_result = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(body)
            .content_type("text/plain; charset=utf-8")
            .send()
            .await;
        emit_s3_call_event(
            "s3.put_object",
            &self.bucket,
            &key,
            bytes,
            send_result.is_ok(),
            start.elapsed(),
        );
        send_result.map_err(|e| {
            anyhow!(
                "Failed to write S3 object s3://{}/{}: {}",
                self.bucket,
                key,
                e
            )
        })?;

        Ok(WorkspaceWriteOutcome {
            bytes: content.len(),
            lines: content.lines().count(),
        })
    }

    async fn list_dir(&self, path: &WorkspacePath) -> WorkspaceResult<Vec<WorkspaceDirEntry>> {
        let prefix = self.list_prefix_for(path);
        let mut entries: Vec<WorkspaceDirEntry> = Vec::new();
        // `total_listed` counts every Content/CommonPrefix the server returned
        // including the prefix marker (the zero-byte "<prefix>/" object some
        // tools create to denote an empty directory). We use it to distinguish
        // "prefix exists but has no children" from "prefix never existed" so
        // `ls` on a missing path on S3 errors like it does on local FS.
        let mut total_listed: usize = 0;
        let mut continuation: Option<String> = None;

        loop {
            let mut req = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(&prefix)
                .delimiter("/");
            if let Some(token) = continuation.as_ref() {
                req = req.continuation_token(token);
            }

            let start = std::time::Instant::now();
            let send_result = req.send().await;
            emit_s3_call_event(
                "s3.list_objects_v2",
                &self.bucket,
                &prefix,
                send_result.as_ref().ok().map_or(0, |r| {
                    r.contents().len() as u64 + r.common_prefixes().len() as u64
                }),
                send_result.is_ok(),
                start.elapsed(),
            );
            let resp = send_result.map_err(|e| classify_list_error(&self.bucket, &prefix, e))?;

            // CommonPrefixes → directories
            for cp in resp.common_prefixes() {
                total_listed += 1;
                if let Some(p) = cp.prefix() {
                    // p looks like "<prefix><name>/"; extract <name>
                    if let Some(name) = strip_dir_name(p, &prefix) {
                        entries.push(WorkspaceDirEntry {
                            name,
                            kind: WorkspaceFileType::Directory,
                            size: 0,
                        });
                    }
                }
            }

            // Contents → files
            for obj in resp.contents() {
                total_listed += 1;
                let Some(key) = obj.key() else { continue };
                // Skip the prefix marker itself (key == prefix exactly).
                if key == prefix {
                    continue;
                }
                if let Some(name) = strip_file_name(key, &prefix) {
                    entries.push(WorkspaceDirEntry {
                        name,
                        kind: WorkspaceFileType::File,
                        size: obj.size().unwrap_or(0).max(0) as u64,
                    });
                }
            }

            if resp.is_truncated().unwrap_or(false) {
                continuation = resp.next_continuation_token().map(|s| s.to_string());
                if continuation.is_none() {
                    break;
                }
            } else {
                break;
            }
        }

        if !path.is_root() && total_listed == 0 {
            return Err(WorkspaceError::NotFound {
                path: format!("s3://{}/{}", self.bucket, prefix.trim_end_matches('/')),
            });
        }

        Ok(entries)
    }
}

#[async_trait]
impl WorkspaceFileSystemExt for S3WorkspaceBackend {
    async fn read_text_with_version(
        &self,
        path: &WorkspacePath,
    ) -> WorkspaceResult<(String, String)> {
        self.get_object_text(path).await
    }

    async fn write_text_if_version(
        &self,
        path: &WorkspacePath,
        content: &str,
        expected_version: &str,
    ) -> WorkspaceResult<WorkspaceWriteOutcome> {
        if expected_version.is_empty() {
            return Err(WorkspaceError::InvalidArgument {
                message:
                    "write_text_if_version requires a non-empty expected version (got empty); \
                 use write_text for unconditional writes"
                        .to_string(),
            });
        }

        let key = self.key_for(path);
        let body = ByteStream::from(content.as_bytes().to_vec());
        let bytes = content.len() as u64;

        let start = std::time::Instant::now();
        let send_result = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .if_match(expected_version)
            .body(body)
            .content_type("text/plain; charset=utf-8")
            .send()
            .await;
        emit_s3_call_event(
            "s3.put_object_if_match",
            &self.bucket,
            &key,
            bytes,
            send_result.is_ok(),
            start.elapsed(),
        );

        match send_result {
            Ok(_) => Ok(WorkspaceWriteOutcome {
                bytes: content.len(),
                lines: content.lines().count(),
            }),
            Err(e) => Err(map_put_error(&self.bucket, &key, expected_version, e)),
        }
    }
}

impl S3WorkspaceBackend {
    /// Recursive (no-delimiter) listing of objects under `base`, with a hard
    /// cap on the number of objects considered.
    ///
    /// Returns `(entries, truncated)` where `entries` holds `(relative_key,
    /// size_bytes)` tuples relative to `base`'s S3 prefix, and `truncated` is
    /// `true` when the cap was reached before the listing completed. The
    /// listing-prefix marker itself is filtered out.
    ///
    /// Used as the foundation for both [`WorkspaceSearch::glob`] and
    /// [`WorkspaceSearch::grep`]. Always paginates through continuation
    /// tokens to avoid silently dropping objects past the first page.
    async fn list_recursive_under(
        &self,
        base: &WorkspacePath,
        max_objects: usize,
    ) -> Result<(Vec<(String, u64)>, bool)> {
        let prefix = self.list_prefix_for(base);
        let mut entries: Vec<(String, u64)> = Vec::new();
        let mut continuation: Option<String> = None;
        let mut truncated = false;

        loop {
            let mut req = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(&prefix);
            if let Some(t) = continuation.as_ref() {
                req = req.continuation_token(t);
            }
            let start = std::time::Instant::now();
            let send_result = req.send().await;
            emit_s3_call_event(
                "s3.list_objects_v2_recursive",
                &self.bucket,
                &prefix,
                send_result
                    .as_ref()
                    .ok()
                    .map_or(0, |r| r.contents().len() as u64),
                send_result.is_ok(),
                start.elapsed(),
            );
            let resp = send_result.map_err(|e| classify_list_error(&self.bucket, &prefix, e))?;

            for obj in resp.contents() {
                if entries.len() >= max_objects {
                    truncated = true;
                    return Ok((entries, truncated));
                }
                let Some(key) = obj.key() else { continue };
                if key == prefix {
                    continue;
                }
                let Some(rel) = key.strip_prefix(&prefix) else {
                    continue;
                };
                if rel.is_empty() {
                    continue;
                }
                let size = obj.size().unwrap_or(0).max(0) as u64;
                entries.push((rel.to_string(), size));
            }

            if resp.is_truncated().unwrap_or(false) {
                continuation = resp.next_continuation_token().map(|s| s.to_string());
                if continuation.is_none() {
                    break;
                }
            } else {
                break;
            }
        }

        Ok((entries, truncated))
    }
}

#[async_trait]
impl WorkspaceSearch for S3WorkspaceBackend {
    async fn glob(&self, request: WorkspaceGlobRequest) -> Result<WorkspaceGlobResult> {
        validate_relative_pattern(&request.pattern, "glob pattern")?;
        let pattern = glob::Pattern::new(&request.pattern)
            .map_err(|e| anyhow!("Invalid glob pattern '{}': {}", request.pattern, e))?;
        // The `glob` crate's `Pattern::matches` is more permissive than the
        // filesystem walker behind `glob::glob` — `*` happily matches across
        // `/`. To stay consistent with the local backend (where `*.rs` does
        // NOT recurse into subdirectories), require an explicit `**` for
        // tree-wide matches; otherwise skip any key containing `/`.
        let recursive = request.pattern.contains("**");

        let (entries, scan_truncated) = self
            .list_recursive_under(&request.base, self.max_objects_scanned)
            .await?;
        if scan_truncated {
            tracing::debug!(
                "S3 glob scan truncated at {} objects under s3://{}/{}",
                self.max_objects_scanned,
                self.bucket,
                self.list_prefix_for(&request.base)
            );
        }

        let mut matches = Vec::new();
        for (rel, _size) in entries {
            if !recursive && rel.contains('/') {
                continue;
            }
            if pattern.matches(&rel) {
                matches.push(join_workspace_path(&request.base, &rel));
            }
        }
        matches.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        Ok(WorkspaceGlobResult { matches })
    }

    async fn grep(&self, request: WorkspaceGrepRequest) -> Result<WorkspaceGrepResult> {
        Ok(self.grep_with_sources(request).await?.result)
    }

    async fn grep_with_sources(
        &self,
        request: WorkspaceGrepRequest,
    ) -> Result<WorkspaceGrepOutcome> {
        use futures::stream::StreamExt;

        if let Some(ref g) = request.glob {
            validate_relative_pattern(g, "grep glob filter")?;
        }

        let regex_pattern = if request.case_insensitive {
            format!("(?i){}", request.pattern)
        } else {
            request.pattern.clone()
        };
        let regex = std::sync::Arc::new(
            regex::Regex::new(&regex_pattern)
                .map_err(|e| anyhow!("Invalid regex pattern '{}': {}", request.pattern, e))?,
        );

        let glob_filter = match request.glob.as_deref() {
            Some(g) => Some((
                glob::Pattern::new(g)
                    .map_err(|e| anyhow!("Invalid grep glob filter '{}': {}", g, e))?,
                g.contains('/'),
            )),
            None => None,
        };

        let (entries, scan_truncated) = self
            .list_recursive_under(&request.base, self.max_objects_scanned)
            .await?;

        // Phase 1 — sequentially filter the listing (cheap; no I/O). We
        // produce a list of objects that pass the glob filter and the
        // per-object size cap. Oversized objects are skipped here, not
        // downloaded.
        let listing_prefix = self.list_prefix_for(&request.base);
        let candidates: Vec<(WorkspacePath, String)> = entries
            .into_iter()
            .filter_map(|(rel, size)| {
                if let Some((ref pat, has_sep)) = glob_filter {
                    let target = if has_sep {
                        rel.as_str()
                    } else {
                        basename(&rel)
                    };
                    if !pat.matches(target) {
                        return None;
                    }
                }
                if size > self.max_grep_bytes_per_object {
                    tracing::debug!(
                        "Skipping S3 object {}{} ({} bytes > grep cap {})",
                        listing_prefix,
                        rel,
                        size,
                        self.max_grep_bytes_per_object
                    );
                    return None;
                }
                let ws_path = join_workspace_path(&request.base, &rel);
                let display_str = escape_control_chars_for_display(ws_path.as_str());
                Some((ws_path, display_str))
            })
            .collect();

        // Phase 2 — fetch objects concurrently and run the regex per file.
        // Output is *not* assembled here; that needs deterministic ordering
        // (Phase 3) and global truncation accounting, so we just collect
        // per-file matches.
        type FileMatch = (WorkspacePath, String, Vec<String>, Vec<usize>);
        let regex_for_stream = std::sync::Arc::clone(&regex);
        let listing_prefix_for_stream = listing_prefix.clone();
        let per_file: Vec<Option<FileMatch>> = futures::stream::iter(candidates)
            .map(|(ws_path, display_str)| {
                let regex = std::sync::Arc::clone(&regex_for_stream);
                let listing_prefix = listing_prefix_for_stream.clone();
                async move {
                    let content = match self.read_text(&ws_path).await {
                        Ok(c) => c,
                        Err(e) => {
                            tracing::debug!(
                                "Skipping S3 object {}{}: {}",
                                listing_prefix,
                                ws_path.as_str(),
                                e
                            );
                            return None;
                        }
                    };
                    let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
                    let mut file_matches: Vec<usize> = Vec::new();
                    for (idx, line) in lines.iter().enumerate() {
                        if regex.is_match(line) {
                            file_matches.push(idx);
                        }
                    }
                    if file_matches.is_empty() {
                        None
                    } else {
                        Some((ws_path, display_str, lines, file_matches))
                    }
                }
            })
            .buffer_unordered(self.search_concurrency.max(1))
            .collect()
            .await;

        // Phase 3 — sort by display path for deterministic output across
        // runs (concurrent completion order is otherwise nondeterministic),
        // then walk the collected matches and accumulate output until
        // `max_output_size` is hit.
        let mut hits: Vec<FileMatch> = per_file.into_iter().flatten().collect();
        hits.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));

        let mut output = String::new();
        let mut match_count = 0usize;
        let mut file_count = 0usize;
        let mut total_size = 0usize;
        let mut output_truncated = false;
        let mut matched_paths = Vec::new();

        'outer: for (workspace_path, display_str, lines, file_matches) in hits {
            file_count += 1;
            let mut path_recorded = false;
            for &match_idx in &file_matches {
                if total_size > request.max_output_size {
                    output_truncated = true;
                    break 'outer;
                }
                if !path_recorded {
                    matched_paths.push(workspace_path.clone());
                    path_recorded = true;
                }
                match_count += 1;

                let start = match_idx.saturating_sub(request.context_lines);
                let end = (match_idx + request.context_lines + 1).min(lines.len());
                for (i, line) in lines[start..end].iter().enumerate() {
                    let abs_i = start + i;
                    let prefix = if abs_i == match_idx { ">" } else { " " };
                    let line = format!("{}{}:{}: {}\n", prefix, display_str, abs_i + 1, line);
                    total_size += line.len();
                    output.push_str(&line);
                }
                if request.context_lines > 0 {
                    output.push_str("--\n");
                    total_size += 3;
                }
            }
        }

        Ok(WorkspaceGrepOutcome {
            result: WorkspaceGrepResult {
                output,
                match_count,
                file_count,
                truncated: output_truncated || scan_truncated,
            },
            matched_paths: Some(matched_paths),
        })
    }
}

/// Join `base` and a key relative to its S3 prefix into a workspace-relative
/// [`WorkspacePath`]. Handles the "base is root" case so the result does not
/// start with `./`.
fn classify_get_error<E>(bucket: &str, key: &str, error: SdkError<E>) -> WorkspaceError
where
    E: std::error::Error + Send + Sync + 'static,
{
    let raw = error
        .raw_response()
        .map(|r| r.status().as_u16())
        .unwrap_or_default();
    if raw == 404 {
        WorkspaceError::NotFound {
            path: format!("s3://{}/{}", bucket, key),
        }
    } else {
        WorkspaceError::Backend(anyhow!(
            "Failed to read S3 object s3://{}/{}: {}",
            bucket,
            key,
            error
        ))
    }
}

fn classify_list_error(
    bucket: &str,
    prefix: &str,
    error: SdkError<ListObjectsV2Error>,
) -> WorkspaceError {
    WorkspaceError::Backend(anyhow!(
        "Failed to list S3 prefix s3://{}/{}: {}",
        bucket,
        prefix,
        error
    ))
}

/// Emit a structured `tracing` event for a single S3 API call.
///
/// Hosts that want to meter S3 cost (call count, bytes transferred, latency)
/// can subscribe to events from this module at `DEBUG` level and route on
/// the `op` field. Fields emitted:
///
/// | Field          | Type    | Meaning                                                   |
/// |----------------|---------|-----------------------------------------------------------|
/// | `op`           | string  | S3 operation (e.g. `s3.get_object`, `s3.list_objects_v2`) |
/// | `bucket`       | string  | Bucket name                                               |
/// | `target`       | string  | Key (GET/PUT) or listing prefix (LIST)                    |
/// | `bytes`        | u64     | Body bytes for GET/PUT; entries returned for LIST         |
/// | `outcome`      | string  | `ok` or `error`                                           |
/// | `duration_ms`  | u64     | Wall-clock duration                                       |
///
/// Emitted at `DEBUG`; zero-cost when the level is disabled.
fn emit_s3_call_event(
    op: &'static str,
    bucket: &str,
    target: &str,
    bytes: u64,
    ok: bool,
    elapsed: std::time::Duration,
) {
    tracing::debug!(
        op = op,
        bucket = %bucket,
        target = %target,
        bytes = bytes,
        outcome = if ok { "ok" } else { "error" },
        duration_ms = elapsed.as_millis() as u64,
    );
}

/// Map a `PutObject` failure to either a [`WorkspaceVersionConflict`]
/// (HTTP 412 Precondition Failed from `If-Match`) or a generic write error.
///
/// AWS S3 does not return the current ETag on 412 so [`WorkspaceVersionConflict::actual`]
/// is left `None`; callers that need the current version must re-read.
fn map_put_error(
    bucket: &str,
    key: &str,
    expected_version: &str,
    error: SdkError<PutObjectError>,
) -> WorkspaceError {
    let status = error
        .raw_response()
        .map(|r| r.status().as_u16())
        .unwrap_or_default();
    if status == 412 {
        WorkspaceError::VersionConflict(WorkspaceVersionConflict {
            path: format!("s3://{}/{}", bucket, key),
            expected: expected_version.to_string(),
            actual: None,
        })
    } else {
        WorkspaceError::Backend(anyhow!(
            "Failed to write S3 object s3://{}/{}: {}",
            bucket,
            key,
            error
        ))
    }
}

impl super::WorkspaceServices {
    /// Build a workspace whose files live in an S3-compatible bucket.
    ///
    /// By default the resulting [`WorkspaceServices`](super::WorkspaceServices)
    /// exposes only read / write / list capabilities (`read`, `write`,
    /// `edit`, `patch`, `ls`); `bash` and `git` are never registered (object
    /// storage cannot service them), and `grep` / `glob` are registered only
    /// when [`S3BackendConfig::search_enabled`] is set — see that field for
    /// cost trade-offs. A 60s per-operation timeout is applied by default;
    /// override via [`super::WorkspaceServicesBuilder::operation_timeout`]
    /// when building manually.
    pub fn s3(config: S3BackendConfig) -> Arc<Self> {
        let backend = Arc::new(S3WorkspaceBackend::new(config));
        Self::from_s3_backend(backend)
    }

    /// Build a workspace from a pre-constructed [`S3WorkspaceBackend`].
    ///
    /// Useful when the caller has injected a custom AWS client (e.g. a mocked
    /// HTTP layer, alternative credential provider, or a wrapper that adds
    /// metrics / tracing).
    ///
    /// The backend is wired both as the `WorkspaceFileSystem` and the
    /// optional `WorkspaceFileSystemExt`, so tools that perform
    /// read-modify-write cycles (`edit`, `patch`) get compare-and-swap
    /// semantics via ETag automatically. When `search_enabled` is set on the
    /// backend, the `grep` / `glob` tools are also registered and constrained
    /// by `max_objects_scanned` / `max_grep_bytes_per_object`; otherwise
    /// capability gating keeps them hidden from the model.
    pub fn from_s3_backend(backend: Arc<S3WorkspaceBackend>) -> Arc<Self> {
        let workspace_ref = super::WorkspaceRef::new(
            format!("s3://{}/{}", backend.bucket(), backend.prefix()),
            format!("s3://{}/{}", backend.bucket(), backend.prefix()),
        );
        let search_capable = backend.search_enabled();
        let fs: Arc<dyn WorkspaceFileSystem> = backend.clone();
        let fs_ext: Arc<dyn WorkspaceFileSystemExt> = backend.clone();
        let mut builder = Self::builder(workspace_ref, fs)
            .file_system_ext(fs_ext)
            .operation_timeout(Duration::from_secs(60));
        if search_capable {
            let search: Arc<dyn WorkspaceSearch> = backend;
            builder = builder.search(search);
        }
        builder.build()
    }
}

#[cfg(test)]
#[path = "s3/tests.rs"]
mod tests;
