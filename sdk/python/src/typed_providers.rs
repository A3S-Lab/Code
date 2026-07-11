//! Typed Python wrappers for session stores, security, and workspace providers.

use super::*;

// ============================================================================
// Typed store / provider helpers
// ============================================================================

/// File-backed long-term memory store.
///
/// Pass to ``SessionOptions.memory_store``:
///
/// .. code-block:: python
///
///     opts = SessionOptions()
///     opts.memory_store = FileMemoryStore('./memory')
///     session = agent.session('.', opts)
#[pyclass(name = "FileMemoryStore")]
#[derive(Clone)]
pub(super) struct PyFileMemoryStore {
    #[pyo3(get, set)]
    pub(super) dir: String,
}

#[pymethods]
impl PyFileMemoryStore {
    #[new]
    pub(super) fn new(dir: String) -> Self {
        Self { dir }
    }

    fn __repr__(&self) -> String {
        format!("FileMemoryStore(dir={:?})", self.dir)
    }
}

/// File-backed session store — persists sessions to disk for later resumption.
///
/// Pass to ``SessionOptions.session_store``:
///
/// .. code-block:: python
///
///     opts = SessionOptions()
///     opts.session_store = FileSessionStore('./sessions')
///     opts.session_id = 'my-session'
///     opts.auto_save = True
///     session = agent.session('.', opts)
#[pyclass(name = "FileSessionStore")]
#[derive(Clone)]
pub(super) struct PyFileSessionStore {
    #[pyo3(get, set)]
    pub(super) dir: String,
}

#[pymethods]
impl PyFileSessionStore {
    #[new]
    pub(super) fn new(dir: String) -> Self {
        Self { dir }
    }

    fn __repr__(&self) -> String {
        format!("FileSessionStore(dir={:?})", self.dir)
    }
}

/// In-memory (non-persistent) session store.
///
/// Useful for testing, ephemeral runs, and CI pipelines where no disk state is needed.
/// Reuse the same instance for save/resume; each constructed instance owns an
/// isolated store.
///
/// .. code-block:: python
///
///     opts = SessionOptions()
///     opts.session_store = MemorySessionStore()
#[pyclass(name = "MemorySessionStore")]
#[derive(Clone)]
pub(super) struct PyMemorySessionStore {
    pub(super) inner: Arc<a3s_code_core::store::MemorySessionStore>,
}

#[pymethods]
impl PyMemorySessionStore {
    #[new]
    pub(super) fn new() -> Self {
        Self {
            inner: Arc::new(a3s_code_core::store::MemorySessionStore::new()),
        }
    }

    fn __repr__(&self) -> String {
        "MemorySessionStore()".to_string()
    }
}

/// Default security provider: input taint tracking + output sanitisation.
///
/// Pass to ``SessionOptions.security_provider``:
///
/// .. code-block:: python
///
///     opts = SessionOptions()
///     opts.security_provider = DefaultSecurityProvider()
#[pyclass(name = "DefaultSecurityProvider")]
#[derive(Clone)]
pub(super) struct PyDefaultSecurityProvider {}

#[pymethods]
impl PyDefaultSecurityProvider {
    #[new]
    pub(super) fn new() -> Self {
        Self {}
    }

    fn __repr__(&self) -> String {
        "DefaultSecurityProvider()".to_string()
    }
}

/// Local filesystem workspace backend.
///
/// This is the explicit typed form of the default local workspace behavior.
/// It is useful when callers want to pass workspace backends through the same
/// option surface that remote/browser backends will use.
///
/// .. code-block:: python
///
///     opts = SessionOptions()
///     opts.workspace_backend = LocalWorkspaceBackend('/repo')
///     session = agent.session('/repo', opts)
#[pyclass(name = "LocalWorkspaceBackend")]
#[derive(Clone)]
pub(super) struct PyLocalWorkspaceBackend {
    #[pyo3(get, set)]
    pub(super) root: String,
}

#[pymethods]
impl PyLocalWorkspaceBackend {
    #[new]
    pub(super) fn new(root: String) -> Self {
        Self { root }
    }

    fn __repr__(&self) -> String {
        format!("LocalWorkspaceBackend(root={:?})", self.root)
    }
}

/// S3-compatible object-storage workspace backend.
///
/// Points the built-in file tools (``read``, ``write``, ``edit``, ``patch``,
/// ``ls``) at any S3-compatible bucket (AWS S3, MinIO, RustFS, Cloudflare R2,
/// Backblaze B2, ...). ``bash``, ``git``, ``grep`` and ``glob`` are
/// intentionally **not** registered when this backend is used because
/// object storage cannot service them.
///
/// .. code-block:: python
///
///     opts = SessionOptions()
///     opts.workspace_backend = S3WorkspaceBackend(
///         bucket="workspace",
///         prefix="users/u1/sessions/s1",
///         access_key_id="AKIA...",
///         secret_access_key="...",
///         endpoint="https://minio.local:9000",
///         region="us-east-1",
///         force_path_style=True,
///     )
///     session = agent.session("s3://workspace/users/u1/sessions/s1", opts)
#[pyclass(name = "S3WorkspaceBackend")]
#[derive(Clone)]
pub(super) struct PyS3WorkspaceBackend {
    #[pyo3(get, set)]
    pub(super) bucket: String,
    #[pyo3(get, set)]
    pub(super) prefix: String,
    #[pyo3(get, set)]
    pub(super) access_key_id: String,
    #[pyo3(get, set)]
    pub(super) secret_access_key: String,
    #[pyo3(get, set)]
    pub(super) endpoint: Option<String>,
    #[pyo3(get, set)]
    pub(super) region: Option<String>,
    #[pyo3(get, set)]
    pub(super) session_token: Option<String>,
    #[pyo3(get, set)]
    pub(super) force_path_style: bool,
    /// Per-read size ceiling (bytes). Defaults to 10 MiB when ``None``.
    #[pyo3(get, set)]
    pub(super) max_read_bytes: Option<u64>,
    /// Enable degraded ``grep`` / ``glob`` against this backend. Off by default
    /// because LIST + GET + regex can be slow and expensive.
    #[pyo3(get, set)]
    pub(super) search_enabled: bool,
    /// Upper bound on objects considered per ``grep`` / ``glob`` call.
    /// Defaults to 500 when ``None``. Ignored when ``search_enabled`` is False.
    #[pyo3(get, set)]
    pub(super) max_objects_scanned: Option<u64>,
    /// Per-object body-size ceiling for ``grep`` downloads. Defaults to 1 MiB
    /// when ``None``. Ignored when ``search_enabled`` is False.
    #[pyo3(get, set)]
    pub(super) max_grep_bytes_per_object: Option<u64>,
    /// Concurrent object downloads during ``grep``. Defaults to 8 when
    /// ``None``. Set lower when the gitserver / S3 endpoint rate-limits
    /// aggressively; set higher when latency dominates. Ignored when
    /// ``search_enabled`` is False.
    #[pyo3(get, set)]
    pub(super) search_concurrency: Option<u64>,
}

#[pymethods]
impl PyS3WorkspaceBackend {
    #[new]
    #[pyo3(signature = (
        bucket,
        prefix,
        access_key_id,
        secret_access_key,
        endpoint = None,
        region = None,
        session_token = None,
        force_path_style = false,
        max_read_bytes = None,
        search_enabled = false,
        max_objects_scanned = None,
        max_grep_bytes_per_object = None,
        search_concurrency = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        bucket: String,
        prefix: String,
        access_key_id: String,
        secret_access_key: String,
        endpoint: Option<String>,
        region: Option<String>,
        session_token: Option<String>,
        force_path_style: bool,
        max_read_bytes: Option<u64>,
        search_enabled: bool,
        max_objects_scanned: Option<u64>,
        max_grep_bytes_per_object: Option<u64>,
        search_concurrency: Option<u64>,
    ) -> Self {
        Self {
            bucket,
            prefix,
            access_key_id,
            secret_access_key,
            endpoint,
            region,
            session_token,
            force_path_style,
            max_read_bytes,
            search_enabled,
            max_objects_scanned,
            max_grep_bytes_per_object,
            search_concurrency,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "S3WorkspaceBackend(bucket={:?}, prefix={:?}, endpoint={:?}, region={:?}, force_path_style={}, search_enabled={})",
            self.bucket, self.prefix, self.endpoint, self.region, self.force_path_style, self.search_enabled,
        )
    }
}

impl PyS3WorkspaceBackend {
    pub(super) fn to_core(&self) -> a3s_code_core::S3BackendConfig {
        let mut cfg = a3s_code_core::S3BackendConfig::new(
            self.bucket.clone(),
            self.prefix.clone(),
            self.access_key_id.clone(),
            self.secret_access_key.clone(),
        )
        .force_path_style(self.force_path_style)
        .enable_search(self.search_enabled);
        if let Some(ref endpoint) = self.endpoint {
            cfg = cfg.endpoint(endpoint.clone());
        }
        if let Some(ref region) = self.region {
            cfg = cfg.region(region.clone());
        }
        if let Some(ref token) = self.session_token {
            cfg = cfg.session_token(token.clone());
        }
        if let Some(n) = self.max_read_bytes {
            cfg = cfg.max_read_bytes(n);
        }
        if let Some(n) = self.max_objects_scanned {
            cfg = cfg.max_objects_scanned(n as usize);
        }
        if let Some(n) = self.max_grep_bytes_per_object {
            cfg = cfg.max_grep_bytes_per_object(n);
        }
        if let Some(n) = self.search_concurrency {
            cfg = cfg.search_concurrency(n as usize);
        }
        cfg
    }
}

/// Configuration for a remote git backend that brings the ``git`` tool to
/// non-local workspaces (S3, future container / DFS) over HTTP/JSON.
///
/// Attach to a session alongside ``workspace_backend``:
///
/// .. code-block:: python
///
///     opts = SessionOptions()
///     opts.workspace_backend = S3WorkspaceBackend(...)
///     opts.remote_git = RemoteGitBackendConfig(
///         base_url="https://gitserver.internal",
///         repo_id="u1/s1",
///         bearer_token=token,
///     )
#[pyclass(name = "RemoteGitBackendConfig")]
#[derive(Clone)]
pub(super) struct PyRemoteGitBackendConfig {
    #[pyo3(get, set)]
    pub(super) base_url: String,
    #[pyo3(get, set)]
    pub(super) repo_id: String,
    #[pyo3(get, set)]
    pub(super) bearer_token: Option<String>,
    /// mTLS client certificate path (PEM). When set together with
    /// ``client_key_pem``, the backend reads both files at construction and
    /// configures mTLS on the HTTP client. Setting only one of the pair
    /// errors at construction.
    #[pyo3(get, set)]
    pub(super) client_cert_pem: Option<String>,
    /// mTLS client private key path (PEM). PKCS#8 format expected for the
    /// ``rustls-tls`` backend. See ``client_cert_pem``.
    #[pyo3(get, set)]
    pub(super) client_key_pem: Option<String>,
    /// Per-call HTTP timeout in milliseconds. Defaults to 30 000.
    #[pyo3(get, set)]
    pub(super) request_timeout_ms: Option<u64>,
    /// Client-side cap on ``diff`` response bytes. Defaults to 1 MiB.
    #[pyo3(get, set)]
    pub(super) max_diff_bytes: Option<u64>,
    /// Client-side cap on ``log`` ``max_count``. Defaults to 200.
    #[pyo3(get, set)]
    pub(super) max_log_entries: Option<u64>,
}

#[pymethods]
impl PyRemoteGitBackendConfig {
    #[new]
    #[pyo3(signature = (
        base_url,
        repo_id,
        bearer_token = None,
        client_cert_pem = None,
        client_key_pem = None,
        request_timeout_ms = None,
        max_diff_bytes = None,
        max_log_entries = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        base_url: String,
        repo_id: String,
        bearer_token: Option<String>,
        client_cert_pem: Option<String>,
        client_key_pem: Option<String>,
        request_timeout_ms: Option<u64>,
        max_diff_bytes: Option<u64>,
        max_log_entries: Option<u64>,
    ) -> Self {
        Self {
            base_url,
            repo_id,
            bearer_token,
            client_cert_pem,
            client_key_pem,
            request_timeout_ms,
            max_diff_bytes,
            max_log_entries,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "RemoteGitBackendConfig(base_url={:?}, repo_id={:?})",
            self.base_url, self.repo_id
        )
    }
}

impl PyRemoteGitBackendConfig {
    pub(super) fn to_core(&self) -> a3s_code_core::RemoteGitBackendConfig {
        let mut cfg =
            a3s_code_core::RemoteGitBackendConfig::new(self.base_url.clone(), self.repo_id.clone());
        if let Some(ref t) = self.bearer_token {
            cfg = cfg.bearer_token(t.clone());
        }
        if let Some(ref p) = self.client_cert_pem {
            cfg = cfg.client_cert_pem(std::path::PathBuf::from(p));
        }
        if let Some(ref p) = self.client_key_pem {
            cfg = cfg.client_key_pem(std::path::PathBuf::from(p));
        }
        if let Some(ms) = self.request_timeout_ms {
            cfg = cfg.request_timeout(std::time::Duration::from_millis(ms));
        }
        if let Some(n) = self.max_diff_bytes {
            cfg = cfg.max_diff_bytes(n);
        }
        if let Some(n) = self.max_log_entries {
            cfg = cfg.max_log_entries(n as usize);
        }
        cfg
    }
}
