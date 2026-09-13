//! S3 workspace backend bindings (requires the `s3` Cargo feature).

use super::*;

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
