//! Declarative S3 workspace configuration and search limits.

use std::time::Duration;

pub(super) const DEFAULT_REGION: &str = "us-east-1";

/// Default cap on the size of a single object readable via
/// [`super::S3WorkspaceBackend::read_text`].
///
/// 10 MiB. Generous for typical source / config files, far below the AWS
/// per-object limit. Override per workspace with [`S3BackendConfig::max_read_bytes`].
pub const DEFAULT_MAX_READ_BYTES: u64 = 10 * 1024 * 1024;

/// Default cap on the number of objects scanned by a single [`WorkspaceSearch`]
/// call (`grep` or `glob`) when the S3 search capability is enabled.
///
/// Override per workspace with [`S3BackendConfig::max_objects_scanned`]. The
/// scan stops once this many keys have been considered and the result is
/// marked truncated.
pub const DEFAULT_MAX_OBJECTS_SCANNED: usize = 500;

/// Default per-object size ceiling for `grep` when the S3 search capability is
/// enabled. Objects larger than this are skipped (tracing::debug) rather than
/// fully downloaded. Override with [`S3BackendConfig::max_grep_bytes_per_object`].
pub const DEFAULT_MAX_GREP_BYTES_PER_OBJECT: u64 = 1024 * 1024;

/// Default concurrency for `grep` object downloads. The backend fetches up to
/// this many objects in parallel; the remaining work serializes after each
/// completes. Override with [`S3BackendConfig::search_concurrency`]. Set lower
/// when the gitserver or S3 endpoint rate-limits aggressively; set higher
/// when latency dominates.
pub const DEFAULT_SEARCH_CONCURRENCY: usize = 8;

/// Configuration for an [`super::S3WorkspaceBackend`].
///
/// `endpoint` is optional: omit it to use the AWS default. Set it to point at
/// MinIO, RustFS, R2, or any other S3-compatible service.
///
/// `prefix` is the logical workspace root inside the bucket — every workspace
/// path becomes `<prefix>/<path>` when sent to S3. An empty prefix means the
/// bucket root itself acts as the workspace.
#[derive(Debug, Clone)]
pub struct S3BackendConfig {
    pub endpoint: Option<String>,
    pub region: Option<String>,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub bucket: String,
    pub prefix: String,
    /// `true` for MinIO / RustFS / most non-AWS endpoints, `false` for AWS S3.
    pub force_path_style: bool,
    /// Per-operation request timeout. Defaults to 30 seconds. Independent of
    /// the workspace-level `operation_timeout` set on [`super::WorkspaceServices`];
    /// whichever fires first wins.
    pub request_timeout: Option<Duration>,
    /// Maximum bytes that may be returned by a single
    /// [`crate::workspace::WorkspaceFileSystem::read_text`]
    /// call. Enforced before the response body is consumed by inspecting the
    /// `Content-Length` reported by S3. Defaults to [`DEFAULT_MAX_READ_BYTES`]
    /// when `None`.
    pub max_read_bytes: Option<u64>,
    /// Enables the `grep` / `glob` built-in tools against this S3 backend.
    ///
    /// Defaults to `false` — object storage cannot natively service search,
    /// and the only available implementation strategy (List + GET + regex)
    /// can produce non-trivial S3 API costs. Hosts must opt in explicitly.
    /// When `false`, capability gating hides `grep` and `glob` from the
    /// model entirely; when `true`, they are registered and constrained by
    /// [`Self::max_objects_scanned`] and [`Self::max_grep_bytes_per_object`].
    pub search_enabled: bool,
    /// Upper bound on objects considered during a single search. `None`
    /// applies [`DEFAULT_MAX_OBJECTS_SCANNED`]. Ignored when
    /// `search_enabled` is `false`.
    pub max_objects_scanned: Option<usize>,
    /// Per-object size ceiling for `grep` body downloads. Objects larger than
    /// this are skipped (debug-traced) rather than fetched. `None` applies
    /// [`DEFAULT_MAX_GREP_BYTES_PER_OBJECT`]. Ignored when `search_enabled` is
    /// `false`.
    pub max_grep_bytes_per_object: Option<u64>,
    /// Number of concurrent object downloads during `grep`. `None` applies
    /// [`DEFAULT_SEARCH_CONCURRENCY`]. Ignored when `search_enabled` is
    /// `false`.
    pub search_concurrency: Option<usize>,
}

impl S3BackendConfig {
    pub fn new(
        bucket: impl Into<String>,
        prefix: impl Into<String>,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: None,
            region: None,
            access_key_id: access_key_id.into(),
            secret_access_key: secret_access_key.into(),
            session_token: None,
            bucket: bucket.into(),
            prefix: prefix.into(),
            force_path_style: false,
            request_timeout: None,
            max_read_bytes: None,
            search_enabled: false,
            max_objects_scanned: None,
            max_grep_bytes_per_object: None,
            search_concurrency: None,
        }
    }

    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    pub fn region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    pub fn session_token(mut self, token: impl Into<String>) -> Self {
        self.session_token = Some(token.into());
        self
    }

    pub fn force_path_style(mut self, enabled: bool) -> Self {
        self.force_path_style = enabled;
        self
    }

    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = Some(timeout);
        self
    }

    /// Override the per-read size ceiling. `0` is rejected at backend
    /// construction time as a configuration mistake. See [`DEFAULT_MAX_READ_BYTES`].
    pub fn max_read_bytes(mut self, bytes: u64) -> Self {
        self.max_read_bytes = Some(bytes);
        self
    }

    /// Enable degraded `grep` / `glob` against this S3 backend. Off by default.
    /// See the documentation on [`Self::search_enabled`] for cost caveats.
    pub fn enable_search(mut self, enabled: bool) -> Self {
        self.search_enabled = enabled;
        self
    }

    /// Override the upper bound on objects considered per search. See
    /// [`DEFAULT_MAX_OBJECTS_SCANNED`]. `0` is treated as the default at
    /// backend construction.
    pub fn max_objects_scanned(mut self, n: usize) -> Self {
        self.max_objects_scanned = Some(n);
        self
    }

    /// Override the per-object body-size ceiling for `grep`. See
    /// [`DEFAULT_MAX_GREP_BYTES_PER_OBJECT`]. `0` is treated as the default.
    pub fn max_grep_bytes_per_object(mut self, bytes: u64) -> Self {
        self.max_grep_bytes_per_object = Some(bytes);
        self
    }

    /// Override the per-search download concurrency. See
    /// [`DEFAULT_SEARCH_CONCURRENCY`]. `0` resets to the default at backend
    /// construction.
    pub fn search_concurrency(mut self, n: usize) -> Self {
        self.search_concurrency = Some(n);
        self
    }
}
