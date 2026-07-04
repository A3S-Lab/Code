//! Remote `WorkspaceGit` backend.
//!
//! Talks an HTTP/JSON protocol to a host-operated `gitserver` so non-local
//! workspaces (S3, future container / DFS) can offer the `git` tool to
//! the model. The full protocol specification lives in the RFC at
//! `apps/docs/content/docs/en/code/rfcs/workspace-remote-git.mdx`. This
//! module is the Rust client side of that protocol.
//!
//! # Capabilities
//!
//! Implements [`WorkspaceGit`] in full and [`WorkspaceGitStashProvider`].
//! Deliberately does **not** implement [`WorkspaceGitWorktreeProvider`]:
//! worktrees are a local-filesystem concept that does not map cleanly onto
//! a remote service. Tools that need per-branch isolation on remote
//! workspaces should use separate sessions with separate `repo_id`s.
//!
//! # Observability
//!
//! Every HTTP call emits a `tracing::debug!` event with the same field
//! shape used by `S3WorkspaceBackend` (op / target / outcome / bytes /
//! duration_ms / status). Hosts that already meter S3 cost via that
//! channel pick up gitserver cost for free.
//!
//! # Authentication
//!
//! Bearer token (default). Empty token mode is permitted for localhost
//! development and emits a warn on construction. mTLS is supported by
//! setting both `client_cert_pem` and `client_key_pem` on the config —
//! the files are read at backend construction, concatenated (cert + key)
//! and handed to `reqwest::Identity::from_pem`. Setting only one of the
//! pair fails at construction with a clear error.

use super::{
    WorkspaceGit, WorkspaceGitBranch, WorkspaceGitCheckoutOutput, WorkspaceGitCheckoutRequest,
    WorkspaceGitCommit, WorkspaceGitCreateBranchRequest, WorkspaceGitDiffRequest,
    WorkspaceGitRemote, WorkspaceGitStash, WorkspaceGitStashProvider, WorkspaceGitStashRequest,
    WorkspaceGitStatus,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Default per-call HTTP timeout, applied to every request the client makes.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Default body-size cap for `diff` responses. The server should honour the
/// same ceiling and set `truncated: true` if it had to clip; this is the
/// client-side defence.
pub const DEFAULT_MAX_DIFF_BYTES: u64 = 1024 * 1024;

/// Default ceiling on `log` `max_count` — caps the per-call response size
/// even when the model requests more.
pub const DEFAULT_MAX_LOG_ENTRIES: usize = 200;

/// Configuration for a [`RemoteGitBackend`].
///
/// `base_url` should not have a trailing slash; the client constructs
/// `{base_url}/v1/repos/{repo_id}/git/{op}` per the RFC.
#[derive(Debug, Clone)]
pub struct RemoteGitBackendConfig {
    pub base_url: String,
    pub repo_id: String,
    pub bearer_token: Option<String>,
    /// mTLS client certificate path (PEM). When set together with
    /// `client_key_pem`, the backend reads both files at construction,
    /// concatenates them, and configures `reqwest::Identity::from_pem`
    /// on the HTTP client. Setting only one of the pair errors at
    /// construction.
    pub client_cert_pem: Option<PathBuf>,
    /// mTLS client private key path (PEM). See `client_cert_pem`. The key
    /// must be in PKCS#8 PEM format for the `rustls-tls` backend.
    pub client_key_pem: Option<PathBuf>,
    pub request_timeout: Option<Duration>,
    pub max_diff_bytes: Option<u64>,
    pub max_log_entries: Option<usize>,
}

impl RemoteGitBackendConfig {
    pub fn new(base_url: impl Into<String>, repo_id: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            repo_id: repo_id.into(),
            bearer_token: None,
            client_cert_pem: None,
            client_key_pem: None,
            request_timeout: None,
            max_diff_bytes: None,
            max_log_entries: None,
        }
    }

    pub fn bearer_token(mut self, token: impl Into<String>) -> Self {
        self.bearer_token = Some(token.into());
        self
    }

    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = Some(timeout);
        self
    }

    pub fn max_diff_bytes(mut self, bytes: u64) -> Self {
        self.max_diff_bytes = Some(bytes);
        self
    }

    pub fn max_log_entries(mut self, n: usize) -> Self {
        self.max_log_entries = Some(n);
        self
    }

    pub fn client_cert_pem(mut self, path: impl Into<PathBuf>) -> Self {
        self.client_cert_pem = Some(path.into());
        self
    }

    pub fn client_key_pem(mut self, path: impl Into<PathBuf>) -> Self {
        self.client_key_pem = Some(path.into());
        self
    }
}

/// Error returned for HTTP 409 / 422 responses that carry a recoverable
/// failure code. Tools downcast with `anyhow::Error::downcast_ref` to react
/// — for example, retrying after a `WORKING_TREE_DIRTY` by stashing first.
#[derive(Debug, Clone, thiserror::Error)]
#[error("remote git conflict: {code}: {message}")]
pub struct RemoteGitConflict {
    pub code: String,
    pub message: String,
}

/// Client for a remote `gitserver`. See module docs / RFC for the protocol.
#[derive(Debug, Clone)]
pub struct RemoteGitBackend {
    http: Client,
    base_url: String,
    repo_id: String,
    bearer_token: Option<String>,
    max_diff_bytes: u64,
    max_log_entries: usize,
}

impl RemoteGitBackend {
    /// Build a backend from declarative configuration.
    pub fn new(config: RemoteGitBackendConfig) -> Result<Arc<Self>> {
        if config
            .bearer_token
            .as_deref()
            .map(str::is_empty)
            .unwrap_or(true)
            && config.client_cert_pem.is_none()
        {
            tracing::warn!(
                "RemoteGitBackend constructed without bearer token or mTLS; \
                 this is only safe on a trusted localhost gitserver"
            );
        }

        let mut builder = Client::builder()
            .no_proxy()
            .timeout(config.request_timeout.unwrap_or(DEFAULT_REQUEST_TIMEOUT));

        // mTLS: both files must be present, otherwise fail closed.
        match (
            config.client_cert_pem.as_deref(),
            config.client_key_pem.as_deref(),
        ) {
            (Some(cert_path), Some(key_path)) => {
                let identity = load_mtls_identity(cert_path, key_path)?;
                builder = builder.identity(identity);
            }
            (Some(_), None) => {
                return Err(anyhow!(
                    "client_cert_pem was set without client_key_pem; both must be provided for mTLS"
                ));
            }
            (None, Some(_)) => {
                return Err(anyhow!(
                    "client_key_pem was set without client_cert_pem; both must be provided for mTLS"
                ));
            }
            (None, None) => {}
        }

        let http = builder
            .build()
            .map_err(|e| anyhow!("failed to build reqwest client: {}", e))?;

        let base_url = config.base_url.trim_end_matches('/').to_string();
        Ok(Arc::new(Self {
            http,
            base_url,
            repo_id: config.repo_id,
            bearer_token: config.bearer_token,
            max_diff_bytes: config.max_diff_bytes.unwrap_or(DEFAULT_MAX_DIFF_BYTES),
            max_log_entries: config.max_log_entries.unwrap_or(DEFAULT_MAX_LOG_ENTRIES),
        }))
    }

    /// Base URL the client is configured to use (no trailing slash).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Opaque repository identifier passed in every request URL.
    pub fn repo_id(&self) -> &str {
        &self.repo_id
    }

    pub fn max_diff_bytes(&self) -> u64 {
        self.max_diff_bytes
    }

    pub fn max_log_entries(&self) -> usize {
        self.max_log_entries
    }

    fn endpoint(&self, op: &str) -> String {
        format!("{}/v1/repos/{}/git/{}", self.base_url, self.repo_id, op)
    }

    async fn post_json<Req, Resp>(&self, op: &'static str, body: &Req) -> Result<Resp>
    where
        Req: Serialize + ?Sized,
        Resp: for<'de> Deserialize<'de>,
    {
        let url = self.endpoint(op);
        let mut req = self.http.post(&url).json(body);
        if let Some(token) = self.bearer_token.as_deref() {
            if !token.is_empty() {
                req = req.bearer_auth(token);
            }
        }

        let start = std::time::Instant::now();
        let send_result = req.send().await;
        let status_code = send_result.as_ref().ok().map(|r| r.status().as_u16());
        let ok = matches!(send_result.as_ref(), Ok(r) if r.status().is_success());
        emit_remote_git_event(op, &self.repo_id, status_code, ok, start.elapsed(), None);

        let resp =
            send_result.map_err(|e| anyhow!("remote git call '{}' transport error: {}", op, e))?;

        let status = resp.status();
        if status.is_success() {
            let parsed = resp
                .json::<Resp>()
                .await
                .map_err(|e| anyhow!("remote git '{}' response body decode error: {}", op, e))?;
            return Ok(parsed);
        }

        let body_text = resp.text().await.unwrap_or_default();
        Err(map_error_response(op, status, &body_text))
    }

    async fn post_unit<Req>(&self, op: &'static str, body: &Req) -> Result<()>
    where
        Req: Serialize + ?Sized,
    {
        let url = self.endpoint(op);
        let mut req = self.http.post(&url).json(body);
        if let Some(token) = self.bearer_token.as_deref() {
            if !token.is_empty() {
                req = req.bearer_auth(token);
            }
        }

        let start = std::time::Instant::now();
        let send_result = req.send().await;
        let status_code = send_result.as_ref().ok().map(|r| r.status().as_u16());
        let ok = matches!(send_result.as_ref(), Ok(r) if r.status().is_success());
        emit_remote_git_event(op, &self.repo_id, status_code, ok, start.elapsed(), None);

        let resp =
            send_result.map_err(|e| anyhow!("remote git call '{}' transport error: {}", op, e))?;

        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let body_text = resp.text().await.unwrap_or_default();
        Err(map_error_response(op, status, &body_text))
    }

    /// Like [`Self::post_json`] but with a hard cap on the streamed response
    /// body in bytes, intended for endpoints that can legitimately return
    /// large payloads (`diff`).
    ///
    /// Two layers of defence:
    /// 1. If the server sends a `Content-Length` greater than `max_bytes`,
    ///    the request is rejected before any body is consumed.
    /// 2. Otherwise the body is streamed; once the accumulated buffer
    ///    exceeds `max_bytes`, the stream is dropped and the call returns
    ///    an error. Memory is bounded at `max_bytes + one chunk`.
    ///
    /// Used by [`WorkspaceGit::diff`]; protects against a misbehaving
    /// gitserver that ignores the client's soft `max_diff_bytes`.
    async fn post_streamed<Req>(
        &self,
        op: &'static str,
        body: &Req,
        max_bytes: u64,
    ) -> Result<Vec<u8>>
    where
        Req: Serialize + ?Sized,
    {
        use futures::StreamExt;

        let url = self.endpoint(op);
        let mut req = self.http.post(&url).json(body);
        if let Some(token) = self.bearer_token.as_deref() {
            if !token.is_empty() {
                req = req.bearer_auth(token);
            }
        }

        let start = std::time::Instant::now();
        let send_result = req.send().await;
        let status_code = send_result.as_ref().ok().map(|r| r.status().as_u16());
        let resp = match send_result {
            Ok(r) => r,
            Err(e) => {
                emit_remote_git_event(op, &self.repo_id, status_code, false, start.elapsed(), None);
                return Err(anyhow!("remote git call '{}' transport error: {}", op, e));
            }
        };

        // Layer 1: eager rejection on advertised oversized body.
        if let Some(len) = resp.content_length() {
            if len > max_bytes {
                emit_remote_git_event(
                    op,
                    &self.repo_id,
                    status_code,
                    false,
                    start.elapsed(),
                    Some(len),
                );
                return Err(anyhow!(
                    "remote git '{}' Content-Length {} exceeds client cap {} bytes; \
                     refusing to download. Raise max_diff_bytes if the body is legitimate.",
                    op,
                    len,
                    max_bytes
                ));
            }
        }

        // Layer 2: stream-bound accumulation.
        let status = resp.status();
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| anyhow!("remote git '{}' stream error: {}", op, e))?;
            if (buf.len() as u64).saturating_add(chunk.len() as u64) > max_bytes {
                emit_remote_git_event(
                    op,
                    &self.repo_id,
                    status_code,
                    false,
                    start.elapsed(),
                    Some(buf.len() as u64),
                );
                return Err(anyhow!(
                    "remote git '{}' response body exceeded client cap {} bytes mid-stream; \
                     aborting",
                    op,
                    max_bytes
                ));
            }
            buf.extend_from_slice(&chunk);
        }

        emit_remote_git_event(
            op,
            &self.repo_id,
            status_code,
            status.is_success(),
            start.elapsed(),
            Some(buf.len() as u64),
        );

        if !status.is_success() {
            let body_text = String::from_utf8_lossy(&buf).into_owned();
            return Err(map_error_response(op, status, &body_text));
        }
        Ok(buf)
    }
}

#[derive(Serialize)]
struct EmptyReq;

#[derive(Deserialize)]
struct StatusResp {
    branch: String,
    commit: String,
    #[serde(default)]
    is_worktree: bool,
    #[serde(default)]
    is_dirty: bool,
    #[serde(default)]
    dirty_count: usize,
}

#[derive(Serialize)]
struct LogReq {
    max_count: usize,
}

#[derive(Deserialize)]
struct LogResp {
    commits: Vec<CommitDto>,
}

#[derive(Deserialize)]
struct CommitDto {
    id: String,
    message: String,
    author: String,
    date: String,
}

#[derive(Deserialize)]
struct BranchesResp {
    branches: Vec<BranchDto>,
}

#[derive(Deserialize)]
struct BranchDto {
    name: String,
    #[serde(default)]
    is_current: bool,
}

#[derive(Serialize)]
struct CreateBranchReq<'a> {
    name: &'a str,
    base: &'a str,
}

#[derive(Serialize)]
struct CheckoutReq<'a> {
    refspec: &'a str,
    force: bool,
}

#[derive(Deserialize)]
struct CheckoutResp {
    #[serde(default)]
    stdout: String,
}

#[derive(Serialize)]
struct DiffReq<'a> {
    target: Option<&'a str>,
}

#[derive(Deserialize)]
struct DiffResp {
    diff: String,
    #[serde(default)]
    truncated: bool,
}

#[derive(Deserialize)]
struct RemotesResp {
    remotes: Vec<RemoteDto>,
}

#[derive(Deserialize)]
struct RemoteDto {
    name: String,
    url: String,
    #[serde(default = "default_direction")]
    direction: String,
}

fn default_direction() -> String {
    "fetch".to_string()
}

#[derive(Deserialize)]
struct ExistsResp {
    #[serde(default)]
    is_repository: bool,
}

#[derive(Deserialize)]
struct StashesResp {
    stashes: Vec<StashDto>,
}

#[derive(Deserialize)]
struct StashDto {
    index: usize,
    #[serde(default)]
    message: String,
}

#[derive(Serialize)]
struct StashCreateReq {
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    include_untracked: bool,
}

#[async_trait]
impl WorkspaceGit for RemoteGitBackend {
    async fn is_repository(&self) -> Result<bool> {
        let resp: ExistsResp = self.post_json("exists", &EmptyReq).await?;
        Ok(resp.is_repository)
    }

    async fn status(&self) -> Result<WorkspaceGitStatus> {
        let resp: StatusResp = self.post_json("status", &EmptyReq).await?;
        Ok(WorkspaceGitStatus {
            branch: resp.branch,
            commit: resp.commit,
            is_worktree: resp.is_worktree,
            is_dirty: resp.is_dirty,
            dirty_count: resp.dirty_count,
        })
    }

    async fn log(&self, max_count: usize) -> Result<Vec<WorkspaceGitCommit>> {
        let capped = max_count.min(self.max_log_entries);
        let resp: LogResp = self.post_json("log", &LogReq { max_count: capped }).await?;
        Ok(resp
            .commits
            .into_iter()
            .map(|c| WorkspaceGitCommit {
                id: c.id,
                message: c.message,
                author: c.author,
                date: c.date,
            })
            .collect())
    }

    async fn list_branches(&self) -> Result<Vec<WorkspaceGitBranch>> {
        let resp: BranchesResp = self.post_json("branches", &EmptyReq).await?;
        Ok(resp
            .branches
            .into_iter()
            .map(|b| WorkspaceGitBranch {
                name: b.name,
                is_current: b.is_current,
            })
            .collect())
    }

    async fn create_branch(&self, request: WorkspaceGitCreateBranchRequest) -> Result<()> {
        self.post_unit(
            "branches/create",
            &CreateBranchReq {
                name: &request.name,
                base: &request.base,
            },
        )
        .await
    }

    async fn checkout(
        &self,
        request: WorkspaceGitCheckoutRequest,
    ) -> Result<WorkspaceGitCheckoutOutput> {
        let resp: CheckoutResp = self
            .post_json(
                "checkout",
                &CheckoutReq {
                    refspec: &request.refspec,
                    force: request.force,
                },
            )
            .await?;
        Ok(WorkspaceGitCheckoutOutput {
            stdout: resp.stdout,
        })
    }

    async fn diff(&self, request: WorkspaceGitDiffRequest) -> Result<String> {
        // Two-layered defence against a misbehaving gitserver:
        //
        // * **Hard memory cap** = `max_diff_bytes * 4` (floor 64 KiB). The
        //   request streams the body and aborts once this is exceeded, so a
        //   server returning a 1 GiB diff never gets fully buffered. We
        //   allow 4× slack over the soft cap so legitimate-but-large diffs
        //   reach the parser and can be display-truncated below.
        // * **Soft display cap** = `max_diff_bytes`. Applied after JSON
        //   decode: the diff text we hand back to the tool is shortened to
        //   this many bytes (UTF-8-safe) so callers see a useful preview
        //   without the model context bloating.
        const DIFF_HARD_CAP_FLOOR: u64 = 64 * 1024;
        let hard_cap = self
            .max_diff_bytes
            .saturating_mul(4)
            .max(DIFF_HARD_CAP_FLOOR);

        let bytes = self
            .post_streamed(
                "diff",
                &DiffReq {
                    target: request.target.as_deref(),
                },
                hard_cap,
            )
            .await?;
        let resp: DiffResp = serde_json::from_slice(&bytes)
            .map_err(|e| anyhow!("remote git 'diff' response body decode error: {}", e))?;

        if (resp.diff.len() as u64) > self.max_diff_bytes {
            tracing::debug!(
                "remote git diff body {} bytes exceeds max_diff_bytes {} — \
                 client-side display truncation",
                resp.diff.len(),
                self.max_diff_bytes
            );
            let cap = self.max_diff_bytes as usize;
            let mut trimmed = resp.diff;
            trimmed.truncate(safe_utf8_truncate(&trimmed, cap));
            trimmed.push_str("\n... [truncated by client max_diff_bytes]\n");
            return Ok(trimmed);
        }
        if resp.truncated {
            return Ok(format!("{}\n... [truncated by gitserver]\n", resp.diff));
        }
        Ok(resp.diff)
    }

    async fn list_remotes(&self) -> Result<Vec<WorkspaceGitRemote>> {
        let resp: RemotesResp = self.post_json("remotes", &EmptyReq).await?;
        Ok(resp
            .remotes
            .into_iter()
            .map(|r| WorkspaceGitRemote {
                name: r.name,
                url: r.url,
                direction: r.direction,
            })
            .collect())
    }
}

#[async_trait]
impl WorkspaceGitStashProvider for RemoteGitBackend {
    async fn list_stashes(&self) -> Result<Vec<WorkspaceGitStash>> {
        let resp: StashesResp = self.post_json("stashes", &EmptyReq).await?;
        Ok(resp
            .stashes
            .into_iter()
            .map(|s| WorkspaceGitStash {
                index: s.index,
                message: s.message,
            })
            .collect())
    }

    async fn stash(&self, request: WorkspaceGitStashRequest) -> Result<()> {
        self.post_unit(
            "stashes/create",
            &StashCreateReq {
                message: request.message,
                include_untracked: request.include_untracked,
            },
        )
        .await
    }
}

/// Read the mTLS cert + key PEM files and assemble a `reqwest::Identity`.
///
/// `reqwest::Identity::from_pem` (with the `rustls-tls` backend) wants a
/// single PEM blob containing the certificate chain followed by the
/// private key. We concatenate the two files with a newline separator —
/// stray trailing newlines in either file are tolerated by the PEM
/// parser. Errors at every step (file I/O, PEM parsing) are mapped to
/// `anyhow` with the source path included so misconfigurations surface
/// clearly.
fn load_mtls_identity(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> Result<reqwest::Identity> {
    let cert = std::fs::read(cert_path).map_err(|e| {
        anyhow!(
            "failed to read mTLS client_cert_pem at {}: {}",
            cert_path.display(),
            e
        )
    })?;
    let key = std::fs::read(key_path).map_err(|e| {
        anyhow!(
            "failed to read mTLS client_key_pem at {}: {}",
            key_path.display(),
            e
        )
    })?;

    let mut pem = Vec::with_capacity(cert.len() + key.len() + 1);
    pem.extend_from_slice(&cert);
    if !cert.ends_with(b"\n") {
        pem.push(b'\n');
    }
    pem.extend_from_slice(&key);

    reqwest::Identity::from_pem(&pem).map_err(|e| {
        anyhow!(
            "failed to parse mTLS PEM material (cert={}, key={}): {}",
            cert_path.display(),
            key_path.display(),
            e
        )
    })
}

/// Truncate `s` to at most `max_bytes`, rounding down to the nearest UTF-8
/// character boundary to keep the result a valid `&str`.
fn safe_utf8_truncate(s: &str, max_bytes: usize) -> usize {
    if s.len() <= max_bytes {
        return s.len();
    }
    let mut idx = max_bytes;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Map a non-2xx response to an `anyhow::Error`, attaching a typed
/// [`RemoteGitConflict`] when the server returned a recoverable code under
/// 409 or 422.
///
/// Synchronous and takes the pre-fetched response body so it can be shared
/// between callers that hold a `reqwest::Response` and callers that have
/// already streamed the body into a `Vec<u8>` (for size-capped paths).
fn map_error_response(op: &'static str, status: StatusCode, body: &str) -> anyhow::Error {
    let parsed: Option<RemoteErrorBody> = serde_json::from_str(body).ok();

    let (code, message) = match parsed {
        Some(b) => (b.error.code, b.error.message),
        None => (format!("HTTP_{}", status.as_u16()), body.to_string()),
    };

    let status_u16 = status.as_u16();
    if status_u16 == 409 || status_u16 == 422 {
        return anyhow::Error::new(RemoteGitConflict { code, message });
    }

    match status_u16 {
        400 => anyhow!("remote git '{}' bad request: {}: {}", op, code, message),
        401 | 403 => anyhow!("remote git '{}' auth failed: {}: {}", op, code, message),
        404 => anyhow!("remote git '{}' not found: {}: {}", op, code, message),
        500..=599 => anyhow!(
            "remote git '{}' server error ({}): {}: {}",
            op,
            status_u16,
            code,
            message
        ),
        _ => anyhow!(
            "remote git '{}' unexpected status {}: {}: {}",
            op,
            status_u16,
            code,
            message
        ),
    }
}

#[derive(Deserialize)]
struct RemoteErrorBody {
    error: RemoteErrorDetail,
}

#[derive(Deserialize)]
struct RemoteErrorDetail {
    code: String,
    #[serde(default)]
    message: String,
}

/// Emit a structured `tracing::debug!` event for a single gitserver call.
///
/// Mirrors the metering shape used by `S3WorkspaceBackend::emit_s3_call_event`
/// so a single subscriber can meter both backends. Fields:
///
/// | Field         | Meaning                                          |
/// |---------------|--------------------------------------------------|
/// | `op`          | gitserver op (`status`, `log`, `diff`, ...)      |
/// | `repo_id`     | opaque repo identifier                           |
/// | `status`      | HTTP status code (when the request reached server) |
/// | `outcome`     | `ok` \| `error`                                   |
/// | `bytes`       | response body length, when known                  |
/// | `duration_ms` | wall-clock                                       |
fn emit_remote_git_event(
    op: &'static str,
    repo_id: &str,
    status: Option<u16>,
    ok: bool,
    elapsed: Duration,
    bytes: Option<u64>,
) {
    tracing::debug!(
        op = format!("git.{}", op),
        repo_id = %repo_id,
        status = status.unwrap_or(0),
        outcome = if ok { "ok" } else { "error" },
        bytes = bytes.unwrap_or(0),
        duration_ms = elapsed.as_millis() as u64,
    );
}

impl super::WorkspaceServices {
    /// Attach a remote git provider to an existing [`WorkspaceServices`].
    ///
    /// Returns a new `Arc<WorkspaceServices>` with `git` and `git_stash`
    /// wired to the remote backend. The original `WorkspaceServices` is
    /// not mutated. `git_worktree` is intentionally reset to `None` —
    /// worktrees are a local-filesystem concept that does not map cleanly
    /// onto a remote service (see RFC §8). All other fields — including
    /// `local_root`, the command runner, the search provider, the
    /// optional `file_system_ext` (S3 CAS), and `operation_timeout` — are
    /// preserved verbatim via
    /// [`super::WorkspaceServices::with_git_provider`].
    pub fn with_remote_git(self: Arc<Self>, config: RemoteGitBackendConfig) -> Result<Arc<Self>> {
        let backend = RemoteGitBackend::new(config)?;
        let git: Arc<dyn WorkspaceGit> = backend.clone();
        let stash: Arc<dyn WorkspaceGitStashProvider> = backend;
        Ok(self.with_git_provider(git, Some(stash)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn server_and_backend() -> (MockServer, Arc<RemoteGitBackend>) {
        let server = MockServer::start().await;
        let cfg = RemoteGitBackendConfig::new(server.uri(), "test")
            .bearer_token("test-token")
            .request_timeout(Duration::from_secs(5));
        let backend = RemoteGitBackend::new(cfg).unwrap();
        (server, backend)
    }

    #[test]
    fn config_defaults_are_documented() {
        let cfg = RemoteGitBackendConfig::new("http://localhost", "r");
        assert!(cfg.bearer_token.is_none());
        assert!(cfg.client_cert_pem.is_none());
        assert!(cfg.request_timeout.is_none());
        assert!(cfg.max_diff_bytes.is_none());
        assert!(cfg.max_log_entries.is_none());
    }

    #[test]
    fn endpoint_url_format_matches_rfc() {
        let cfg = RemoteGitBackendConfig::new("http://localhost:8080/", "u1/s1");
        let backend = RemoteGitBackend::new(cfg).unwrap();
        // Trailing slash on base_url is stripped.
        assert_eq!(backend.base_url(), "http://localhost:8080");
        assert_eq!(
            backend.endpoint("status"),
            "http://localhost:8080/v1/repos/u1/s1/git/status"
        );
        assert_eq!(
            backend.endpoint("branches/create"),
            "http://localhost:8080/v1/repos/u1/s1/git/branches/create"
        );
    }

    #[test]
    fn mtls_requires_both_cert_and_key() {
        let cfg = RemoteGitBackendConfig::new("http://localhost", "r").client_cert_pem("/dev/null");
        let err = RemoteGitBackend::new(cfg).unwrap_err();
        assert!(
            err.to_string().contains("client_key_pem"),
            "missing-key error must name the missing field, got: {}",
            err
        );

        let cfg = RemoteGitBackendConfig::new("http://localhost", "r").client_key_pem("/dev/null");
        let err = RemoteGitBackend::new(cfg).unwrap_err();
        assert!(
            err.to_string().contains("client_cert_pem"),
            "missing-cert error must name the missing field, got: {}",
            err
        );
    }

    #[test]
    fn mtls_rejects_invalid_pem_blob() {
        let tmp = tempfile::tempdir().unwrap();
        let cert = tmp.path().join("cert.pem");
        let key = tmp.path().join("key.pem");
        std::fs::write(&cert, b"not a pem").unwrap();
        std::fs::write(&key, b"also not a pem").unwrap();

        let cfg = RemoteGitBackendConfig::new("http://localhost", "r")
            .client_cert_pem(&cert)
            .client_key_pem(&key);
        let err = RemoteGitBackend::new(cfg).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("PEM"),
            "PEM-parse failure must surface clearly, got: {}",
            msg
        );
        assert!(
            msg.contains(cert.to_str().unwrap()),
            "error must include the cert path for debugging, got: {}",
            msg
        );
    }

    #[test]
    fn mtls_accepts_self_signed_pair_from_rcgen() {
        // rcgen produces a valid cert + PKCS#8 key pair; `reqwest::Identity`
        // (rustls-tls backend) should accept the concatenated PEM blob.
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("rcgen self-signed cert");
        let tmp = tempfile::tempdir().unwrap();
        let cert_path = tmp.path().join("client.cert.pem");
        let key_path = tmp.path().join("client.key.pem");
        std::fs::write(&cert_path, cert.cert.pem()).unwrap();
        std::fs::write(&key_path, cert.key_pair.serialize_pem()).unwrap();

        let cfg = RemoteGitBackendConfig::new("http://localhost", "r")
            .bearer_token("t")
            .client_cert_pem(&cert_path)
            .client_key_pem(&key_path);
        let backend = RemoteGitBackend::new(cfg)
            .expect("valid rcgen-generated PEM pair must produce a backend");
        // We cannot easily verify the identity is wired into the client without
        // a live mTLS server; the assertion above (construction succeeds) is the
        // contract — invalid material would have errored at `from_pem`.
        assert_eq!(backend.base_url(), "http://localhost");
    }

    #[test]
    fn safe_utf8_truncate_respects_boundaries() {
        // ASCII path
        assert_eq!(safe_utf8_truncate("hello", 3), 3);
        assert_eq!(safe_utf8_truncate("hello", 100), 5);
        // Multi-byte path: "héllo" — 'é' is 2 bytes (0xC3 0xA9)
        let s = "héllo";
        // Truncating at byte 2 lands inside 'é'; rounds down to 1 (after 'h')
        assert_eq!(safe_utf8_truncate(s, 2), 1);
        assert_eq!(safe_utf8_truncate(s, 3), 3);
    }

    #[tokio::test]
    async fn status_happy_path() {
        let (server, backend) = server_and_backend().await;
        Mock::given(method("POST"))
            .and(path("/v1/repos/test/git/status"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "branch": "main",
                "commit": "abc123",
                "is_worktree": false,
                "is_dirty": true,
                "dirty_count": 3,
            })))
            .mount(&server)
            .await;

        let status = backend.status().await.unwrap();
        assert_eq!(status.branch, "main");
        assert_eq!(status.commit, "abc123");
        assert!(status.is_dirty);
        assert_eq!(status.dirty_count, 3);
    }

    #[tokio::test]
    async fn log_respects_client_max_log_entries() {
        let server = MockServer::start().await;
        let cfg = RemoteGitBackendConfig::new(server.uri(), "test")
            .bearer_token("t")
            .max_log_entries(5);
        let backend = RemoteGitBackend::new(cfg).unwrap();

        Mock::given(method("POST"))
            .and(path("/v1/repos/test/git/log"))
            .and(wiremock::matchers::body_json(json!({"max_count": 5})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "commits": [
                    {"id":"a","message":"m","author":"x","date":"d"}
                ]
            })))
            .mount(&server)
            .await;

        // Client asks for 100, but the server should see the capped value.
        let commits = backend.log(100).await.unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].id, "a");
    }

    #[tokio::test]
    async fn list_branches_maps_response() {
        let (server, backend) = server_and_backend().await;
        Mock::given(method("POST"))
            .and(path("/v1/repos/test/git/branches"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "branches": [
                    {"name":"main", "is_current":true},
                    {"name":"feat/x"}
                ]
            })))
            .mount(&server)
            .await;

        let branches = backend.list_branches().await.unwrap();
        assert_eq!(branches.len(), 2);
        assert!(branches[0].is_current);
        assert!(!branches[1].is_current);
    }

    #[tokio::test]
    async fn create_branch_succeeds_on_201() {
        let (server, backend) = server_and_backend().await;
        Mock::given(method("POST"))
            .and(path("/v1/repos/test/git/branches/create"))
            .and(wiremock::matchers::body_json(json!({
                "name":"feat/x","base":"main"
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
            .mount(&server)
            .await;

        backend
            .create_branch(WorkspaceGitCreateBranchRequest {
                name: "feat/x".into(),
                base: "main".into(),
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn create_branch_409_yields_remote_git_conflict() {
        let (server, backend) = server_and_backend().await;
        Mock::given(method("POST"))
            .and(path("/v1/repos/test/git/branches/create"))
            .respond_with(ResponseTemplate::new(409).set_body_json(json!({
                "error":{"code":"BRANCH_EXISTS","message":"branch 'feat/x' already exists"}
            })))
            .mount(&server)
            .await;

        let err = backend
            .create_branch(WorkspaceGitCreateBranchRequest {
                name: "feat/x".into(),
                base: "main".into(),
            })
            .await
            .unwrap_err();
        let conflict = err
            .downcast_ref::<RemoteGitConflict>()
            .expect("409 must downcast to RemoteGitConflict");
        assert_eq!(conflict.code, "BRANCH_EXISTS");
        assert!(conflict.message.contains("feat/x"));
    }

    #[tokio::test]
    async fn checkout_returns_stdout() {
        let (server, backend) = server_and_backend().await;
        Mock::given(method("POST"))
            .and(path("/v1/repos/test/git/checkout"))
            .and(wiremock::matchers::body_json(json!({
                "refspec":"feat/x","force":false
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "stdout":"Switched to branch 'feat/x'"
            })))
            .mount(&server)
            .await;

        let out = backend
            .checkout(WorkspaceGitCheckoutRequest {
                refspec: "feat/x".into(),
                force: false,
            })
            .await
            .unwrap();
        assert!(out.stdout.contains("feat/x"));
    }

    #[tokio::test]
    async fn checkout_409_dirty_yields_conflict() {
        let (server, backend) = server_and_backend().await;
        Mock::given(method("POST"))
            .and(path("/v1/repos/test/git/checkout"))
            .respond_with(ResponseTemplate::new(409).set_body_json(json!({
                "error":{"code":"WORKING_TREE_DIRTY","message":"please stash first"}
            })))
            .mount(&server)
            .await;

        let err = backend
            .checkout(WorkspaceGitCheckoutRequest {
                refspec: "main".into(),
                force: false,
            })
            .await
            .unwrap_err();
        let c = err.downcast_ref::<RemoteGitConflict>().unwrap();
        assert_eq!(c.code, "WORKING_TREE_DIRTY");
    }

    #[tokio::test]
    async fn diff_passes_target_through_and_surfaces_server_truncation() {
        let (server, backend) = server_and_backend().await;
        Mock::given(method("POST"))
            .and(path("/v1/repos/test/git/diff"))
            .and(wiremock::matchers::body_json(json!({"target":"main"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "diff":"<huge diff>",
                "truncated": true
            })))
            .mount(&server)
            .await;

        let diff = backend
            .diff(WorkspaceGitDiffRequest {
                target: Some("main".to_string()),
            })
            .await
            .unwrap();
        assert!(diff.contains("truncated by gitserver"));
    }

    #[tokio::test]
    async fn diff_enforces_client_max_diff_bytes() {
        let server = MockServer::start().await;
        let cfg = RemoteGitBackendConfig::new(server.uri(), "test")
            .bearer_token("t")
            .max_diff_bytes(8);
        let backend = RemoteGitBackend::new(cfg).unwrap();

        Mock::given(method("POST"))
            .and(path("/v1/repos/test/git/diff"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "diff":"AAAAAAAAAAAAAAAAAAAAAA",   // 22 bytes
                "truncated": false
            })))
            .mount(&server)
            .await;

        let diff = backend
            .diff(WorkspaceGitDiffRequest { target: None })
            .await
            .unwrap();
        assert!(diff.contains("truncated by client max_diff_bytes"));
        // First 8 bytes preserved.
        assert!(diff.starts_with("AAAAAAAA"));
    }

    /// Phase 6.2 OOM defence: the gitserver advertises a Content-Length far
    /// beyond what the client tolerates. The request must fail without
    /// consuming the body.
    ///
    /// `max_diff_bytes = 8` ⇒ `hard_cap = max(8 * 4, 64 KiB) = 64 KiB`.
    /// We respond with `Content-Length: 1 048 576` so the eager rejection
    /// path fires.
    #[tokio::test]
    async fn diff_rejects_oversized_content_length_upfront() {
        let server = MockServer::start().await;
        let cfg = RemoteGitBackendConfig::new(server.uri(), "test")
            .bearer_token("t")
            .max_diff_bytes(8);
        let backend = RemoteGitBackend::new(cfg).unwrap();

        // 1 MiB body — far past the 64 KiB hard cap floor.
        let huge_body = vec![b'A'; 1024 * 1024];
        Mock::given(method("POST"))
            .and(path("/v1/repos/test/git/diff"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_bytes(huge_body),
            )
            .mount(&server)
            .await;

        let err = backend
            .diff(WorkspaceGitDiffRequest { target: None })
            .await
            .expect_err("oversized body must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("Content-Length") && msg.contains("exceeds client cap"),
            "expected eager Content-Length rejection, got: {}",
            msg
        );
    }

    /// Phase 6.2 OOM defence layer 2: when Content-Length is absent or the
    /// server lies about it, the stream-bound accumulator must abort once
    /// the cap is exceeded. We use chunked transfer (no Content-Length) so
    /// the eager path doesn't fire.
    #[tokio::test]
    async fn diff_aborts_mid_stream_on_cap_exceeded() {
        let server = MockServer::start().await;
        let cfg = RemoteGitBackendConfig::new(server.uri(), "test")
            .bearer_token("t")
            .max_diff_bytes(8);
        let backend = RemoteGitBackend::new(cfg).unwrap();

        // Body large enough to exceed the 64 KiB hard cap floor; chunked
        // transfer encoded so no Content-Length header is set.
        let big_body = vec![b'A'; 256 * 1024];
        Mock::given(method("POST"))
            .and(path("/v1/repos/test/git/diff"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("transfer-encoding", "chunked")
                    .set_body_bytes(big_body),
            )
            .mount(&server)
            .await;

        let err = backend
            .diff(WorkspaceGitDiffRequest { target: None })
            .await
            .expect_err("oversized streamed body must be rejected");
        let msg = err.to_string();
        // Either the eager path (if wiremock surfaces a Content-Length) or
        // the stream-abort path fires; both are valid defences.
        assert!(
            msg.contains("exceeds client cap")
                || msg.contains("exceeded client cap")
                || msg.contains("Content-Length"),
            "expected oversize rejection, got: {}",
            msg
        );
    }

    #[tokio::test]
    async fn list_remotes_defaults_direction() {
        let (server, backend) = server_and_backend().await;
        Mock::given(method("POST"))
            .and(path("/v1/repos/test/git/remotes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "remotes":[{"name":"origin","url":"git@x:y.git"}]
            })))
            .mount(&server)
            .await;

        let rs = backend.list_remotes().await.unwrap();
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].direction, "fetch");
    }

    #[tokio::test]
    async fn is_repository_returns_bool() {
        let (server, backend) = server_and_backend().await;
        Mock::given(method("POST"))
            .and(path("/v1/repos/test/git/exists"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "is_repository": true
            })))
            .mount(&server)
            .await;

        assert!(backend.is_repository().await.unwrap());
    }

    #[tokio::test]
    async fn list_stashes_maps_response() {
        let (server, backend) = server_and_backend().await;
        Mock::given(method("POST"))
            .and(path("/v1/repos/test/git/stashes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "stashes":[{"index":0,"message":"WIP"}]
            })))
            .mount(&server)
            .await;

        let s = backend.list_stashes().await.unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].message, "WIP");
    }

    #[tokio::test]
    async fn stash_create_409_nothing_to_stash() {
        let (server, backend) = server_and_backend().await;
        Mock::given(method("POST"))
            .and(path("/v1/repos/test/git/stashes/create"))
            .respond_with(ResponseTemplate::new(409).set_body_json(json!({
                "error":{"code":"NOTHING_TO_STASH","message":"clean tree"}
            })))
            .mount(&server)
            .await;

        let err = backend
            .stash(WorkspaceGitStashRequest {
                message: None,
                include_untracked: false,
            })
            .await
            .unwrap_err();
        let c = err.downcast_ref::<RemoteGitConflict>().unwrap();
        assert_eq!(c.code, "NOTHING_TO_STASH");
    }

    #[tokio::test]
    async fn not_found_404_is_generic_anyhow() {
        let (server, backend) = server_and_backend().await;
        Mock::given(method("POST"))
            .and(path("/v1/repos/test/git/status"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "error":{"code":"REPO_NOT_FOUND","message":"unknown repo"}
            })))
            .mount(&server)
            .await;

        let err = backend.status().await.unwrap_err();
        assert!(err.to_string().contains("not found"), "msg: {}", err);
        assert!(err.downcast_ref::<RemoteGitConflict>().is_none());
    }

    #[tokio::test]
    async fn auth_failure_401_is_generic_anyhow() {
        let (server, backend) = server_and_backend().await;
        Mock::given(method("POST"))
            .and(path("/v1/repos/test/git/status"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "error":{"code":"INVALID_TOKEN","message":"bad bearer"}
            })))
            .mount(&server)
            .await;

        let err = backend.status().await.unwrap_err();
        assert!(err.to_string().contains("auth failed"), "msg: {}", err);
        assert!(err.downcast_ref::<RemoteGitConflict>().is_none());
    }

    #[tokio::test]
    async fn server_500_is_generic_anyhow() {
        let (server, backend) = server_and_backend().await;
        Mock::given(method("POST"))
            .and(path("/v1/repos/test/git/status"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;

        let err = backend.status().await.unwrap_err();
        assert!(err.to_string().contains("server error"), "msg: {}", err);
    }

    #[tokio::test]
    async fn non_json_error_body_falls_back_to_http_code() {
        let (server, backend) = server_and_backend().await;
        Mock::given(method("POST"))
            .and(path("/v1/repos/test/git/status"))
            .respond_with(ResponseTemplate::new(409).set_body_string("not json"))
            .mount(&server)
            .await;

        let err = backend.status().await.unwrap_err();
        // 409 always yields a conflict — even when the body is opaque, we
        // surface it so callers can detect it; the code falls back to
        // HTTP_409.
        let c = err
            .downcast_ref::<RemoteGitConflict>()
            .expect("409 must yield conflict regardless of body shape");
        assert_eq!(c.code, "HTTP_409");
        assert_eq!(c.message, "not json");
    }

    #[tokio::test]
    async fn with_remote_git_wires_git_and_stash() {
        let services = super::super::WorkspaceServices::local(std::env::temp_dir());
        let upgraded = services
            .with_remote_git(RemoteGitBackendConfig::new("http://localhost", "r").bearer_token("t"))
            .unwrap();
        assert!(upgraded.git().is_some());
        assert!(upgraded.git_stash().is_some());
        // Worktree provider intentionally dropped on remote-git workspaces —
        // worktrees do not have a remote analogue (see RFC §8).
        assert!(upgraded.git_worktree().is_none());
        assert!(upgraded.capabilities().git);
    }

    /// Regression test for Phase 6.1 field-loss bug.
    ///
    /// `with_remote_git` previously rebuilt `WorkspaceServices` via the
    /// builder, which silently dropped `local_root` (and would silently
    /// drop any future field). After the fix it goes through
    /// `with_git_provider`, which uses an explicit struct literal — the
    /// compiler now forces every field to be addressed.
    #[tokio::test]
    async fn with_remote_git_preserves_local_root_and_unrelated_capabilities() {
        let temp = tempfile::tempdir().unwrap();
        let base = super::super::WorkspaceServices::local(temp.path());
        assert!(
            base.local_root().is_some(),
            "precondition: local() must set local_root"
        );
        assert!(
            base.command_runner().is_some(),
            "precondition: local() must wire bash runner"
        );
        let base_root = base.local_root().map(|p| p.to_path_buf());

        let upgraded = base
            .with_remote_git(RemoteGitBackendConfig::new("http://localhost", "r").bearer_token("t"))
            .unwrap();

        // The git provider IS replaced.
        assert!(upgraded.git().is_some());
        assert!(upgraded.capabilities().git);
        // Unrelated capabilities survive.
        assert_eq!(
            upgraded.local_root().map(|p| p.to_path_buf()),
            base_root,
            "local_root must survive with_remote_git"
        );
        assert!(
            upgraded.command_runner().is_some(),
            "command_runner must survive with_remote_git"
        );
        assert!(
            upgraded.search().is_some(),
            "search provider must survive with_remote_git"
        );
        // But worktree is intentionally severed alongside the git swap.
        assert!(upgraded.git_worktree().is_none());
    }

    /// End-to-end test: drive the built-in `git` tool against a wiremock-backed
    /// gitserver. Exercises the full path `git tool → WorkspaceGit (remote) →
    /// HTTP → wiremock → JSON → DTO → WorkspaceGitStatus → tool output`.
    ///
    /// This is the contract test for Phase 4.2: if any layer breaks, this
    /// test fails. Per-method unit tests above isolate the HTTP layer; this
    /// one proves the tool wiring actually works through a real ToolContext.
    #[tokio::test]
    async fn git_tool_status_works_through_remote_backend() {
        use crate::tools::{Tool, ToolContext};

        let server = MockServer::start().await;
        // `git` tool probes `is_repository` before dispatching.
        Mock::given(method("POST"))
            .and(path("/v1/repos/u1/s1/git/exists"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"is_repository": true})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/repos/u1/s1/git/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "branch":"main",
                "commit":"deadbeef",
                "is_worktree": false,
                "is_dirty": false,
                "dirty_count": 0,
            })))
            .mount(&server)
            .await;

        let base = super::super::WorkspaceServices::local(std::env::temp_dir());
        let services = base
            .with_remote_git(RemoteGitBackendConfig::new(server.uri(), "u1/s1").bearer_token("tok"))
            .unwrap();

        let tool = crate::tools::builtin::git::GitTool;
        let ctx = ToolContext::new(std::env::temp_dir()).with_workspace_services(services);

        let result = tool
            .execute(&json!({"command": "status"}), &ctx)
            .await
            .unwrap();
        assert!(result.success, "tool output: {}", result.content);
        assert!(
            result.content.contains("main"),
            "expected branch name in output: {}",
            result.content
        );
        assert!(
            result.content.contains("deadbeef"),
            "expected commit hash in output: {}",
            result.content
        );
        assert!(
            result.content.contains("clean"),
            "expected clean status in output: {}",
            result.content
        );
    }
}
