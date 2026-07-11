use super::*;

// ============================================================================
// Typed store / provider helpers
// ============================================================================

// Internal napi-rs compatibility shims.
//
// napi-rs `#[napi(object)]` structs cannot hold `#[napi]` class instances directly,
// so SessionOptions fields that accept store/provider objects are typed as these plain
// structs. Users work exclusively with the public classes (FileMemoryStore,
// FileSessionStore, MemorySessionStore, DefaultSecurityProvider); TypeScript structural
// compatibility ensures those instances satisfy these struct shapes automatically.
//
// These are NOT exported in the public TypeScript API surface (index.d.ts).

#[napi(object)]
#[derive(Clone, Default)]
pub struct JsMemoryStore {
    pub backend: String,
    pub dir: Option<String>,
}

#[napi(object)]
#[derive(Clone, Default)]
pub struct JsSessionStore {
    pub backend: String,
    pub dir: Option<String>,
    /// Opaque identity for a live in-process memory store.
    ///
    /// This is populated by `MemorySessionStore`; callers must not synthesize it.
    pub instance_id: Option<String>,
}

#[napi(object)]
#[derive(Clone, Default)]
pub struct JsSecurityProvider {
    pub kind: String,
}

#[napi(object)]
#[derive(Clone, Default)]
pub struct JsWorkspaceBackend {
    pub kind: String,
    pub root: Option<String>,
    pub s3: Option<JsS3BackendConfig>,
}

/// Configuration for an S3-compatible workspace backend.
///
/// Use this with [`S3WorkspaceBackend`] to point a session's built-in file
/// tools at any S3-compatible endpoint (AWS S3, MinIO, RustFS, R2, etc.).
/// `endpoint` is optional — omit it to use the AWS default. `prefix` is
/// the logical workspace root inside the bucket; every workspace path
/// becomes `<prefix>/<path>` when sent to S3.
#[napi(object)]
#[derive(Clone, Default)]
pub struct JsS3BackendConfig {
    /// Optional S3 endpoint URL. Omit for AWS S3 (the SDK will compute it
    /// from `region`). Set to `https://...` for MinIO / RustFS / R2 / etc.
    pub endpoint: Option<String>,
    /// AWS region. Defaults to `us-east-1` when omitted.
    pub region: Option<String>,
    /// Static access key. Use `sessionToken` together when STS-issued.
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    /// Bucket name.
    pub bucket: String,
    /// Logical workspace prefix inside the bucket (without leading/trailing
    /// slashes). Use `""` to make the bucket root the workspace.
    pub prefix: String,
    /// `true` for MinIO / RustFS / most non-AWS endpoints; `false` for AWS S3.
    pub force_path_style: Option<bool>,
    /// Maximum bytes a single `read` may return. The backend rejects any
    /// response with `Content-Length` greater than this without buffering
    /// the body. Defaults to 10 MiB on the Rust side when omitted.
    pub max_read_bytes: Option<i64>,
    /// Enable degraded `grep` / `glob` against this S3 backend. Off by
    /// default — object storage has no native search, so the only viable
    /// strategy is `LIST` + `GET` + regex, which can be slow and expensive.
    pub search_enabled: Option<bool>,
    /// Upper bound on objects considered per `grep` / `glob` call. Defaults
    /// to 500 on the Rust side. Ignored when `searchEnabled` is `false`.
    pub max_objects_scanned: Option<i64>,
    /// Per-object body-size ceiling for `grep` downloads. Larger objects are
    /// skipped (debug-traced). Defaults to 1 MiB on the Rust side. Ignored
    /// when `searchEnabled` is `false`.
    pub max_grep_bytes_per_object: Option<i64>,
    /// Concurrent object downloads during `grep`. Defaults to 8 on the
    /// Rust side. Set lower when the gitserver / S3 endpoint rate-limits
    /// aggressively; set higher when latency dominates. Ignored when
    /// `searchEnabled` is `false`.
    pub search_concurrency: Option<i64>,
}

/// Configuration for a [`RemoteGitBackend`] — an HTTP/JSON client that
/// brings the `git` tool to non-local workspaces (S3, future container /
/// DFS).
///
/// Pass alongside `workspaceBackend` on a session to attach remote git
/// on top of any filesystem backend. The protocol is specified in the
/// repository RFC `apps/docs/content/docs/en/code/rfcs/workspace-remote-git.mdx`.
#[napi(object)]
#[derive(Clone, Default)]
pub struct JsRemoteGitBackendConfig {
    /// Base URL of the gitserver, no trailing slash. The client builds
    /// `{baseUrl}/v1/repos/{repoId}/git/{op}` per the RFC.
    pub base_url: String,
    /// Opaque repository identifier, URL-safe. Negotiated out of band
    /// with the gitserver operator.
    pub repo_id: String,
    /// Bearer token sent as `Authorization: Bearer <token>`. Required in
    /// production; omitting it emits a `tracing::warn!` and is only safe
    /// on a trusted localhost gitserver.
    pub bearer_token: Option<String>,
    /// mTLS client certificate path (PEM). When set together with
    /// `clientKeyPem`, the backend reads both files at construction and
    /// configures mTLS on the HTTP client. Setting only one of the pair
    /// errors at construction.
    pub client_cert_pem: Option<String>,
    /// mTLS client private key path (PEM). PKCS#8 format expected for the
    /// `rustls-tls` backend. See `clientCertPem`.
    pub client_key_pem: Option<String>,
    /// Per-call HTTP timeout in milliseconds. Defaults to 30 000.
    pub request_timeout_ms: Option<i64>,
    /// Client-side cap on `diff` response bytes. Defaults to 1 MiB.
    pub max_diff_bytes: Option<i64>,
    /// Client-side cap on `log` `max_count`. Defaults to 200.
    pub max_log_entries: Option<i64>,
}

/// File-backed long-term memory store.
///
/// ```js
/// agent.session('.', { memoryStore: new FileMemoryStore('./memory') });
/// ```
#[napi]
pub struct FileMemoryStore {
    pub backend: String,
    pub dir: String,
}

#[napi]
impl FileMemoryStore {
    /// Create a file-backed memory store at `dir`.
    #[napi(constructor)]
    pub fn new(dir: String) -> Self {
        Self {
            backend: "file".to_string(),
            dir,
        }
    }
}

/// File-backed session store (persists sessions to disk for later resumption).
///
/// ```js
/// agent.session('.', {
///   sessionStore: new FileSessionStore('./sessions'),
///   sessionId: 'my-session',
///   autoSave: true,
/// });
/// ```
#[napi]
pub struct FileSessionStore {
    pub backend: String,
    pub dir: String,
}

#[napi]
impl FileSessionStore {
    /// Create a file-backed session store at `dir`.
    #[napi(constructor)]
    pub fn new(dir: String) -> Self {
        Self {
            backend: "file".to_string(),
            dir,
        }
    }
}

/// In-memory (non-persistent) session store.
///
/// Useful for testing, ephemeral runs, and CI pipelines where no disk state is needed.
/// Reuse the same instance for save/resume; each constructed instance owns an
/// isolated store.
///
/// ```js
/// agent.session('.', { sessionStore: new MemorySessionStore() });
/// ```
#[napi]
pub struct MemorySessionStore {
    pub backend: String,
    /// Opaque handle used to preserve this store's identity through napi-rs'
    /// structural `SessionOptions` conversion.
    pub instance_id: String,
    inner: Arc<a3s_code_core::store::MemorySessionStore>,
}

pub(super) type NodeMemorySessionStoreRegistry =
    std::collections::HashMap<String, Weak<a3s_code_core::store::MemorySessionStore>>;

pub(super) fn node_memory_session_store_registry() -> &'static Mutex<NodeMemorySessionStoreRegistry>
{
    static REGISTRY: OnceLock<Mutex<NodeMemorySessionStoreRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

pub(super) fn resolve_node_memory_session_store(
    instance_id: Option<&str>,
) -> napi::Result<Arc<a3s_code_core::store::MemorySessionStore>> {
    let instance_id = instance_id.ok_or_else(|| {
        napi::Error::from_reason(
            "MemorySessionStore identity is missing; pass the original MemorySessionStore instance",
        )
    })?;
    let weak = {
        let mut registry = node_memory_session_store_registry()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        registry.retain(|_, store| store.strong_count() > 0);
        registry.get(instance_id).cloned()
    };
    weak.and_then(|store| store.upgrade()).ok_or_else(|| {
        napi::Error::from_reason(
            "MemorySessionStore identity is invalid or expired; pass the original MemorySessionStore instance",
        )
    })
}

#[napi]
impl MemorySessionStore {
    #[napi(constructor)]
    pub fn new() -> Self {
        let store = Self {
            backend: "memory".to_string(),
            instance_id: a3s_code_core::host_env::HostEnv::system().next_id(),
            inner: Arc::new(a3s_code_core::store::MemorySessionStore::new()),
        };
        let mut registry = node_memory_session_store_registry()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        registry.retain(|_, registered| registered.strong_count() > 0);
        registry.insert(store.instance_id.clone(), Arc::downgrade(&store.inner));
        drop(registry);
        store
    }
}

impl Drop for MemorySessionStore {
    fn drop(&mut self) {
        let mut registry = node_memory_session_store_registry()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let own_store = Arc::downgrade(&self.inner);
        if registry
            .get(&self.instance_id)
            .is_some_and(|registered| registered.ptr_eq(&own_store))
        {
            registry.remove(&self.instance_id);
        }
    }
}

impl Default for MemorySessionStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Default security provider: input taint tracking + output sanitisation.
///
/// ```js
/// agent.session('.', { securityProvider: new DefaultSecurityProvider() });
/// ```
#[napi]
pub struct DefaultSecurityProvider {
    pub kind: String,
}

#[napi]
impl DefaultSecurityProvider {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            kind: "default".to_string(),
        }
    }
}

impl Default for DefaultSecurityProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Local filesystem workspace backend.
///
/// This is the explicit typed form of the default local workspace behavior.
/// It is useful when callers want to pass workspace backends through the same
/// option surface that remote/browser backends will use.
///
/// ```js
/// agent.session('/repo', { workspaceBackend: new LocalWorkspaceBackend('/repo') });
/// ```
#[napi]
pub struct LocalWorkspaceBackend {
    pub kind: String,
    pub root: String,
}

#[napi]
impl LocalWorkspaceBackend {
    /// Create a local filesystem workspace backend rooted at `root`.
    #[napi(constructor)]
    pub fn new(root: String) -> Self {
        Self {
            kind: "local".to_string(),
            root,
        }
    }
}

/// S3-compatible object-storage workspace backend.
///
/// Points built-in file tools (`read`, `write`, `edit`, `patch`, `ls`) at an
/// S3-compatible bucket. Works with AWS S3, MinIO, RustFS, Cloudflare R2,
/// Backblaze B2, and other S3-API-compatible services.
///
/// `bash`, `git`, `grep`, and `glob` are intentionally **not** registered
/// when this backend is in use — object storage cannot service them.
///
/// ```js
/// const backend = new S3WorkspaceBackend({
///   endpoint: 'https://minio.local:9000',
///   region: 'us-east-1',
///   accessKeyId: 'AKIA...',
///   secretAccessKey: '...',
///   bucket: 'workspace',
///   prefix: 'users/u1/sessions/s1',
///   forcePathStyle: true,
/// });
/// agent.session('s3://workspace/users/u1/sessions/s1', { workspaceBackend: backend });
/// ```
#[napi]
pub struct S3WorkspaceBackend {
    pub kind: String,
    pub s3: JsS3BackendConfig,
}

#[napi]
impl S3WorkspaceBackend {
    /// Create an S3-compatible workspace backend.
    #[napi(constructor)]
    pub fn new(config: JsS3BackendConfig) -> Self {
        Self {
            kind: "s3".to_string(),
            s3: config,
        }
    }
}

pub(super) fn s3_config_to_core(js: &JsS3BackendConfig) -> a3s_code_core::S3BackendConfig {
    let mut cfg = a3s_code_core::S3BackendConfig::new(
        js.bucket.clone(),
        js.prefix.clone(),
        js.access_key_id.clone(),
        js.secret_access_key.clone(),
    );
    if let Some(ref endpoint) = js.endpoint {
        cfg = cfg.endpoint(endpoint.clone());
    }
    if let Some(ref region) = js.region {
        cfg = cfg.region(region.clone());
    }
    if let Some(ref token) = js.session_token {
        cfg = cfg.session_token(token.clone());
    }
    if let Some(force) = js.force_path_style {
        cfg = cfg.force_path_style(force);
    }
    if let Some(n) = js.max_read_bytes {
        cfg = cfg.max_read_bytes(n.max(0) as u64);
    }
    if let Some(on) = js.search_enabled {
        cfg = cfg.enable_search(on);
    }
    if let Some(n) = js.max_objects_scanned {
        cfg = cfg.max_objects_scanned(n.max(0) as usize);
    }
    if let Some(n) = js.max_grep_bytes_per_object {
        cfg = cfg.max_grep_bytes_per_object(n.max(0) as u64);
    }
    if let Some(n) = js.search_concurrency {
        cfg = cfg.search_concurrency(n.max(0) as usize);
    }
    cfg
}

pub(super) fn remote_git_config_to_core(
    js: &JsRemoteGitBackendConfig,
) -> a3s_code_core::RemoteGitBackendConfig {
    let mut cfg =
        a3s_code_core::RemoteGitBackendConfig::new(js.base_url.clone(), js.repo_id.clone());
    if let Some(ref t) = js.bearer_token {
        cfg = cfg.bearer_token(t.clone());
    }
    if let Some(ref p) = js.client_cert_pem {
        cfg = cfg.client_cert_pem(std::path::PathBuf::from(p));
    }
    if let Some(ref p) = js.client_key_pem {
        cfg = cfg.client_key_pem(std::path::PathBuf::from(p));
    }
    if let Some(ms) = js.request_timeout_ms {
        cfg = cfg.request_timeout(std::time::Duration::from_millis(ms.max(0) as u64));
    }
    if let Some(n) = js.max_diff_bytes {
        cfg = cfg.max_diff_bytes(n.max(0) as u64);
    }
    if let Some(n) = js.max_log_entries {
        cfg = cfg.max_log_entries(n.max(0) as usize);
    }
    cfg
}
