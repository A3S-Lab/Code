//! Filesystem helpers backing the client-facing `workspace.client_fs_*` RPCs.
//! The a3s conversation-files UI and the chat backend call them, tunneled through the server.
//!
//! Deliberately separate from the shell-facing ext ops in [`ext_fs`](super::ext_fs).
//! Every path is relative to the client-fs base (`WorkspaceHandle::client_fs_base`) and resolves through the root-confinement helper.
//! The list walk excludes symlinks that resolve outside the base and never descends into them.
//! Listings paginate with stable post-sort slices, and reads are binary-safe (base64 chunks).
//! Writes stage base64 chunks in a temp file beside the target and rename it into place on finalize; the target and `overwrite` are pinned by the first chunk.
//!
//! Wire types live in `a3s_code_workspace_types::rpc::fs` (the `ClientFs*` types), shared with the backend caller.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use a3s_code_workspace_types::rpc::fs::{
    ClientFsListNode as FsListNode, ClientFsListReq as FsListReq, ClientFsListRes as FsListRes,
    ClientFsReadFileReq as FsReadFileReq, ClientFsReadFileRes as FsReadFileRes,
    ClientFsStatReq as FsStatReq, ClientFsStatRes as FsStatRes,
    ClientFsWriteFileReq as FsWriteFileReq, ClientFsWriteFileRes as FsWriteFileRes, FsContentType,
    FsNodeType, MAX_CLIENT_FS_WRITE_CHUNK_BYTES, MAX_CLIENT_FS_WRITE_FILE_BYTES,
};
use base64::Engine;
use sha2::{Digest, Sha256};

use crate::error::{WorkspaceError, WorkspaceResult};
use crate::handle::{ClientFsBase, WorkspaceHandle};
use crate::session::WorkspaceSession;

/// Hard cap on entries collected per list call before sorting (shared across all fs surfaces; see [`super::walk::MAX_LIST_COLLECT`]).
const MAX_LIST_COLLECT: usize = super::walk::MAX_LIST_COLLECT;

/// Server-side cap on `FsListReq::limit`.
const MAX_LIST_LIMIT: u32 = 1000;

/// Server-side cap on a single read's effective byte budget (shared across all fs surfaces; see [`super::walk::MAX_READ_BYTES`]).
/// Only referenced by tests now that the clamp lives in `walk::clamp_read_length`.
#[cfg(test)]
const MAX_READ_BYTES: u64 = super::walk::MAX_READ_BYTES;

/// Bound on memoized hashes; the memo is cleared (not LRU-evicted) when full, and entries re-hash on next use.
const HASH_MEMO_CAPACITY: usize = 4096;

#[derive(Debug, Clone)]
struct MemoEntry {
    size: u64,
    mtime_ms: i64,
    hash: String,
}

/// Memo of full-content SHA-256 digests keyed by absolute path and validated against `(size, mtime_ms)`.
/// Unchanged files hash once instead of on every `client_fs_stat`; on a mismatch the caller re-hashes, so mtime never stands in for the content hash.
#[derive(Debug, Default)]
pub(crate) struct FileHashMemo {
    entries: parking_lot::Mutex<HashMap<PathBuf, MemoEntry>>,
}

impl FileHashMemo {
    /// Return the memoized hash when `(size, mtime_ms)` still match.
    pub(crate) fn lookup(&self, path: &Path, size: u64, mtime_ms: i64) -> Option<String> {
        let entries = self.entries.lock();
        let entry = entries.get(path)?;
        (entry.size == size && entry.mtime_ms == mtime_ms).then(|| entry.hash.clone())
    }

    /// Record a freshly computed hash, replacing any stale entry for the same path.
    /// Clears the whole memo when inserting a new path would exceed [`HASH_MEMO_CAPACITY`].
    pub(crate) fn store(&self, path: &Path, size: u64, mtime_ms: i64, hash: String) {
        let mut entries = self.entries.lock();
        if !entries.contains_key(path) && entries.len() >= HASH_MEMO_CAPACITY {
            entries.clear();
        }
        entries.insert(
            path.to_path_buf(),
            MemoEntry {
                size,
                mtime_ms,
                hash,
            },
        );
    }
}

/// Resolve a base-relative request path (`""` and `"."` mean the base), rejecting `..` and symlink escapes above `base`.
async fn resolve_in_base(base: &ClientFsBase, path: &str) -> WorkspaceResult<PathBuf> {
    let rel = if path.is_empty() { "." } else { path };
    WorkspaceHandle::resolve_path_within_root(rel, &base.base, &base.canonical).await
}

/// [`resolve_in_base`] against the session's client-fs base.
async fn resolve(
    ws: &WorkspaceHandle,
    session_id: Option<&str>,
    path: &str,
) -> WorkspaceResult<PathBuf> {
    let base = ws.client_fs_base(session_id).await?;
    resolve_in_base(&base, path).await
}

fn system_time_ms(st: std::time::SystemTime) -> i64 {
    match st.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

/// List `req.path` with stable pagination: collect the full walk (bounded by [`MAX_LIST_COLLECT`]), sort, then slice `[offset, offset + limit)`.
/// The sort is directories first, then case-insensitive by name.
/// Symlinks resolving outside the base are excluded from the walk (and never descended into).
pub(crate) async fn list(
    ws: &WorkspaceHandle,
    session_id: Option<&str>,
    req: &FsListReq,
) -> WorkspaceResult<FsListRes> {
    let base = ws.client_fs_base(session_id).await?;
    let abs = resolve_in_base(&base, &req.path).await?;
    let req = req.clone();
    // The walk does synchronous traversal and metadata syscalls; run it off the async executor (matching the ext_fs ops)
    tokio::task::spawn_blocking(move || {
        list_blocking(&abs, &base.base, &base.canonical, &req, MAX_LIST_COLLECT)
    })
    .await
    .map_err(|e| WorkspaceError::JoinError(e.to_string()))?
}

fn list_blocking(
    abs_dir: &Path,
    base: &Path,
    canonical_base: &Path,
    req: &FsListReq,
    max_collect: usize,
) -> WorkspaceResult<FsListRes> {
    // Base confinement also holds mid-walk: a symlink inside the base pointing outside must not enumerate metadata of files outside it
    let page = super::walk::list_directory_paged(
        abs_dir,
        super::walk::ListOptions {
            depth: req.depth as usize,
            follow_symlinks: req.follow_symlinks,
            respect_git_ignore: req.respect_git_ignore,
            include_hidden: req.include_hidden,
            include_globs: &req.include_globs,
            exclude_globs: &req.exclude_globs,
            offset: req.offset,
            limit: req.limit.min(MAX_LIST_LIMIT) as usize,
            confine_to_canonical_root: Some(canonical_base.to_path_buf()),
        },
        max_collect,
    );

    let nodes: Vec<FsListNode> = page
        .entries
        .into_iter()
        .map(|e| FsListNode {
            node_type: if e.is_dir {
                FsNodeType::Directory
            } else {
                FsNodeType::File
            },
            size: e.size,
            mtime_ms: e.modified.map(system_time_ms),
            is_symlink: e.is_symlink.then_some(true),
            // Base-relative path (divergent from the shell's absolute path).
            // A walk under a symlinked base yields entries spelled with the canonical path, so strip either spelling
            path: e
                .abs_path
                .strip_prefix(base)
                .or_else(|_| e.abs_path.strip_prefix(canonical_base))
                .unwrap_or(&e.abs_path)
                .to_string_lossy()
                .into_owned(),
            name: e.name,
        })
        .collect();

    Ok(FsListRes {
        nodes,
        truncated: page.truncated,
    })
}

/// Stat `req.path`: existence, kind, size, mtime, and (for files) a full-content SHA-256 served through the workspace hash memo.
pub(crate) async fn stat(
    ws: &WorkspaceHandle,
    session_id: Option<&str>,
    req: &FsStatReq,
) -> WorkspaceResult<FsStatRes> {
    let abs = resolve(ws, session_id, &req.path).await?;
    let md = match tokio::fs::metadata(&abs).await {
        Ok(md) => md,
        // NotADirectory: a *file* sits mid-path (e.g. `a.txt/sub`); for an existence probe that is a miss, not an RPC error.
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                || e.kind() == std::io::ErrorKind::NotADirectory =>
        {
            return Ok(FsStatRes {
                exists: false,
                node_type: None,
                size: None,
                mtime_ms: None,
                hash: None,
            });
        }
        Err(e) => {
            return Err(WorkspaceError::HubError(format!(
                "stat failed for {}: {e}",
                req.path
            )));
        }
    };
    let mtime_ms = md.modified().ok().map(system_time_ms);
    if md.is_dir() {
        return Ok(FsStatRes {
            exists: true,
            node_type: Some(FsNodeType::Directory),
            size: None,
            mtime_ms,
            hash: None,
        });
    }
    let size = md.len();
    let memo = &ws.shared.client_fs_hash_memo;
    let hash = match mtime_ms.and_then(|m| memo.lookup(&abs, size, m)) {
        Some(hash) => hash,
        None => {
            let (hash, _, _) = crate::handle::stream_hash_and_range(&abs, 0, 0)
                .await
                .map_err(|e| {
                    WorkspaceError::HubError(format!("hash failed for {}: {e}", req.path))
                })?;
            if let Some(m) = mtime_ms {
                memo.store(&abs, size, m, hash.clone());
            }
            hash
        }
    };
    Ok(FsStatRes {
        exists: true,
        node_type: Some(FsNodeType::File),
        size: Some(size),
        mtime_ms,
        hash: Some(hash),
    })
}

/// Read a byte range of `req.path` (binary-safe, capped at `min(req.max_bytes, MAX_READ_BYTES)`) together with the full-file SHA-256.
/// When the hash is memoized for the current `(size, mtime)` only the requested range is read.
/// Otherwise the whole file streams once (via the shared [`crate::handle::stream_hash_and_range`]) to hash it.
pub(crate) async fn read_file(
    ws: &WorkspaceHandle,
    session_id: Option<&str>,
    req: &FsReadFileReq,
) -> WorkspaceResult<FsReadFileRes> {
    let abs = resolve(ws, session_id, &req.path).await?;
    let read_err =
        |e: std::io::Error| WorkspaceError::HubError(format!("read failed for {}: {e}", req.path));
    let md = tokio::fs::metadata(&abs).await.map_err(read_err)?;
    if md.is_dir() {
        return Err(WorkspaceError::HubError(format!(
            "not a file: {}",
            req.path
        )));
    }
    let size = md.len();
    let mtime_ms = md.modified().ok().map(system_time_ms);
    let offset = req.offset.unwrap_or(0);
    // Server-side clamp: a hostile/buggy caller cannot lift the per-chunk budget past MAX_READ_BYTES regardless of `maxBytes`
    let length = super::walk::clamp_read_length(req.length, req.max_bytes);

    let memo = &ws.shared.client_fs_hash_memo;
    let (hash, chunk, size) = match mtime_ms.and_then(|m| memo.lookup(&abs, size, m)) {
        Some(hash) => {
            let chunk = super::walk::read_range(&abs, offset, length)
                .await
                .map_err(read_err)?;
            (hash, chunk, size)
        }
        None => {
            let (hash, chunk, streamed) =
                crate::handle::stream_hash_and_range(&abs, offset, length)
                    .await
                    .map_err(read_err)?;
            if let Some(m) = mtime_ms {
                memo.store(&abs, streamed, m, hash.clone());
            }
            (hash, chunk, streamed)
        }
    };

    // Shared encoder keeps the paired wire fields consistent with one UTF-8 validation pass; `type` is text only when the bytes were valid UTF-8
    let (payload, is_text) = super::walk::encode_chunk(chunk, req.encoding);
    let (content, content_base64) = match payload {
        super::walk::ChunkPayload::Text(t) => (Some(t), None),
        super::walk::ChunkPayload::Base64(b) => (None, Some(b)),
    };
    let content_type = if is_text {
        FsContentType::Text
    } else {
        FsContentType::Binary
    };
    Ok(FsReadFileRes {
        content,
        content_base64,
        size,
        hash,
        content_type,
    })
}

/// Staged uploads idle longer than this are dropped by [`gc_staged_uploads`].
pub(crate) const STAGED_UPLOAD_MAX_IDLE: Duration = Duration::from_secs(10 * 60);

/// Period of the daemon's staged-upload GC tick.
pub(crate) const STAGED_UPLOAD_GC_TICK: Duration = Duration::from_secs(60);

/// Longest accepted `uploadId`; it becomes part of the staging file name.
const MAX_UPLOAD_ID_LEN: usize = 64;

/// Infix of every staging file name: `.<name>.a3s-upload-<uploadId>`.
const STAGING_INFIX: &str = ".grok-upload-";

/// Bound on entries visited by the startup orphan sweep so a huge folder cannot stall it indefinitely.
const ORPHAN_SWEEP_MAX_ENTRIES: usize = 500_000;

/// Directories the orphan sweep never descends into: dependency and build trees that would spend the entry budget before user directories.
const ORPHAN_SWEEP_SKIP_DIRS: &[&str] = &[".git", "node_modules", "target"];

/// Base64 text length that can decode to at most [`MAX_CLIENT_FS_WRITE_CHUNK_BYTES`] (padded, 4 chars per 3 bytes).
const MAX_CHUNK_BASE64_LEN: usize = MAX_CLIENT_FS_WRITE_CHUNK_BYTES.div_ceil(3) * 4;

/// Longest file name most filesystems accept (`NAME_MAX`); the staging name must fit it.
const MAX_FILE_NAME_BYTES: usize = 255;

/// Ceiling on concurrently staged uploads per session.
pub(crate) const MAX_STAGED_UPLOADS_PER_SESSION: usize = 16;

/// Ceiling on bytes staged per session across all its uploads.
/// Checked against a snapshot taken at chunk start, so concurrent chunks of other ids may overshoot it by at most one chunk each.
pub(crate) const MAX_STAGED_BYTES_PER_SESSION: u64 = 512 * 1024 * 1024;

/// One in-progress `client_fs_write_file` upload: the temp file beside its target plus the running length and digest.
/// Dropping it deletes the temp file (`NamedTempFile`), so every abandonment path cleans up by dropping.
struct StagedUpload {
    target: PathBuf,
    /// Final absolute path, validated as UTF-8 before any byte is staged.
    file_path: String,
    temp: tempfile::NamedTempFile,
    len: u64,
    hasher: Sha256,
    /// `overwrite` of the first chunk; later chunks cannot widen or narrow it.
    overwrite: bool,
    last_seen: Instant,
}

enum UploadSlot {
    /// A chunk for this id is being written; a concurrent chunk is a protocol error.
    /// Carries the bytes staged at checkout so the session byte ceiling still counts the upload.
    Busy(u64),
    Idle(Box<StagedUpload>),
}

impl UploadSlot {
    fn staged_len(&self) -> u64 {
        match self {
            UploadSlot::Busy(len) => *len,
            UploadSlot::Idle(upload) => upload.len,
        }
    }
}

/// Staged `client_fs_write_file` uploads of one session, keyed by `uploadId`.
#[derive(Default)]
pub(crate) struct StagedUploads {
    entries: parking_lot::Mutex<HashMap<String, UploadSlot>>,
}

/// Why [`StagedUploads::take`] handed out nothing.
enum TakeMiss {
    /// No entry for the id.
    Absent,
    /// A chunk for the id was in flight; the entry has been dropped.
    Busy,
}

/// Why [`StagedUploads::reserve`] refused a first chunk.
enum ReserveMiss {
    /// Another chunk already claimed the id.
    Claimed,
    /// The session is at [`MAX_STAGED_UPLOADS_PER_SESSION`].
    Full,
}

impl StagedUploads {
    /// Check out the upload for `upload_id`, leaving a `Busy` marker; also returns the bytes staged by the session's other uploads.
    /// A concurrent chunk finds the marker, removes it, and fails, and the in-flight writer then fails at [`Self::restore`] and drops its temp file.
    fn take(&self, upload_id: &str) -> Result<(StagedUpload, u64), TakeMiss> {
        let mut entries = self.entries.lock();
        let Some(slot) = entries.get_mut(upload_id) else {
            return Err(TakeMiss::Absent);
        };
        let UploadSlot::Idle(upload) = std::mem::replace(slot, UploadSlot::Busy(0)) else {
            entries.remove(upload_id);
            return Err(TakeMiss::Busy);
        };
        *slot = UploadSlot::Busy(upload.len);
        let elsewhere = Self::staged_bytes_except(&entries, upload_id);
        Ok((*upload, elsewhere))
    }

    /// Reserve `upload_id` for a first chunk; returns the bytes staged by the session's other uploads.
    fn reserve(&self, upload_id: &str) -> Result<u64, ReserveMiss> {
        let mut entries = self.entries.lock();
        if entries.contains_key(upload_id) {
            return Err(ReserveMiss::Claimed);
        }
        if entries.len() >= MAX_STAGED_UPLOADS_PER_SESSION {
            return Err(ReserveMiss::Full);
        }
        entries.insert(upload_id.to_owned(), UploadSlot::Busy(0));
        Ok(Self::staged_bytes_except(&entries, upload_id))
    }

    fn staged_bytes_except(entries: &HashMap<String, UploadSlot>, upload_id: &str) -> u64 {
        entries
            .iter()
            .filter(|(id, _)| id.as_str() != upload_id)
            .map(|(_, slot)| slot.staged_len())
            .sum()
    }

    /// Put a checked-out upload back. `Err` returns it when the `Busy` marker was removed by a concurrent chunk; the caller drops it.
    fn restore(&self, upload_id: &str, upload: StagedUpload) -> Result<(), StagedUpload> {
        let mut entries = self.entries.lock();
        match entries.get_mut(upload_id) {
            Some(slot) if matches!(slot, UploadSlot::Busy(_)) => {
                *slot = UploadSlot::Idle(Box::new(upload));
                Ok(())
            }
            _ => Err(upload),
        }
    }

    /// Release the `Busy` marker after a finalize or a failed chunk.
    fn release(&self, upload_id: &str) {
        let mut entries = self.entries.lock();
        if matches!(entries.get(upload_id), Some(UploadSlot::Busy(_))) {
            entries.remove(upload_id);
        }
    }

    /// Whether `upload_id` is staged or in flight for this session.
    pub(crate) fn contains(&self, upload_id: &str) -> bool {
        self.entries.lock().contains_key(upload_id)
    }

    /// Drop idle uploads unseen for longer than `max_idle` as of `now`; returns how many were dropped.
    pub(crate) fn gc(&self, now: Instant, max_idle: Duration) -> usize {
        let stale: Vec<Box<StagedUpload>> = {
            let mut entries = self.entries.lock();
            let stale_ids: Vec<String> = entries
                .iter()
                .filter_map(|(id, slot)| match slot {
                    UploadSlot::Idle(upload)
                        if now.saturating_duration_since(upload.last_seen) > max_idle =>
                    {
                        Some(id.clone())
                    }
                    _ => None,
                })
                .collect();
            stale_ids
                .iter()
                .filter_map(|id| match entries.remove(id) {
                    Some(UploadSlot::Idle(upload)) => Some(upload),
                    _ => None,
                })
                .collect()
        };
        // Temp files are unlinked here, outside the lock.
        let dropped = stale.len();
        for upload in stale {
            tracing::info!(target = %upload.target.display(), staged = upload.len,
                "dropped stale client-fs staged upload");
        }
        dropped
    }

    /// Drop every upload (session teardown); their temp files go with them.
    pub(crate) fn abandon_all(&self) {
        let entries = std::mem::take(&mut *self.entries.lock());
        drop(entries);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.lock().len()
    }

    #[cfg(test)]
    fn set_last_seen(&self, upload_id: &str, last_seen: Instant) {
        if let Some(UploadSlot::Idle(upload)) = self.entries.lock().get_mut(upload_id) {
            upload.last_seen = last_seen;
        }
    }
}

/// `uploadId` becomes a file-name component, so it is restricted to a short filesystem-neutral alphabet.
fn validate_upload_id(upload_id: &str) -> WorkspaceResult<()> {
    let valid = !upload_id.is_empty()
        && upload_id.len() <= MAX_UPLOAD_ID_LEN
        && upload_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if valid {
        Ok(())
    } else {
        Err(WorkspaceError::HubError(format!(
            "invalid_upload_id: uploadId must be 1-{MAX_UPLOAD_ID_LEN} ASCII letters, digits, '-' or '_'"
        )))
    }
}

fn out_of_order(detail: impl std::fmt::Display) -> WorkspaceError {
    WorkspaceError::HubError(format!("out_of_order: {detail}"))
}

/// Stage one chunk of `req.path` for the bound session and, on `finalize`, rename the staged file into place.
/// Bytes are appended to `.<name>.a3s-upload-<uploadId>` in the target's directory; nothing appears at `path` before finalize.
/// The target is pinned by the first chunk: a later chunk whose `path` resolves elsewhere drops the upload (`path_mismatch`).
/// A chunk whose `offset` is not the staged length drops the upload (`out_of_order`), except an exact replay of the last staged chunk,
/// which is a no-op returning the staged size so a retried request whose response was lost does not kill the upload.
/// A chunk that races another chunk of the same id drops the upload too.
pub(crate) async fn write_file(
    ws: &WorkspaceHandle,
    session_id: Option<&str>,
    req: &FsWriteFileReq,
) -> WorkspaceResult<FsWriteFileRes> {
    let session_id = session_id.ok_or_else(|| {
        WorkspaceError::HubError("client_fs_write_file requires a bound session".to_owned())
    })?;
    let session = ws
        .session(session_id)
        .ok_or_else(|| WorkspaceError::SessionNotFound(session_id.to_owned()))?;
    validate_upload_id(&req.upload_id)?;
    if req.content_base64.len() > MAX_CHUNK_BASE64_LEN {
        return Err(WorkspaceError::HubError(format!(
            "chunk_too_large: chunk exceeds {MAX_CLIENT_FS_WRITE_CHUNK_BYTES} bytes"
        )));
    }
    let uploads = session.staged_uploads();
    let upload_id = req.upload_id.as_str();

    let (upload, staged_elsewhere) = match uploads.take(upload_id) {
        Ok((upload, staged_elsewhere)) => {
            // The path is re-resolved on every chunk so a caller cannot retarget an upload after the first chunk.
            let target = match resolve_for_session(ws, session_id, &session, &req.path).await {
                Ok((target, _)) => target,
                Err(e) => {
                    uploads.release(upload_id);
                    drop(upload);
                    return Err(e);
                }
            };
            if target != upload.target {
                uploads.release(upload_id);
                drop(upload);
                return Err(WorkspaceError::HubError(format!(
                    "path_mismatch: {} does not resolve to the target of upload {upload_id}; upload dropped",
                    req.path
                )));
            }
            (upload, staged_elsewhere)
        }
        Err(TakeMiss::Busy) => {
            return Err(out_of_order(format!(
                "concurrent chunk for upload {upload_id}; upload dropped"
            )));
        }
        Err(TakeMiss::Absent) => {
            if req.offset != 0 {
                return Err(out_of_order(format!(
                    "no staged upload {upload_id} for offset {}",
                    req.offset
                )));
            }
            let (target, base) = resolve_for_session(ws, session_id, &session, &req.path).await?;
            if target == base.base || target == base.canonical {
                return Err(WorkspaceError::HubError(format!(
                    "not a file: {}",
                    req.path
                )));
            }
            let staged_elsewhere = match uploads.reserve(upload_id) {
                Ok(staged_elsewhere) => staged_elsewhere,
                Err(ReserveMiss::Claimed) => {
                    return Err(out_of_order(format!(
                        "concurrent first chunk for upload {upload_id}"
                    )));
                }
                Err(ReserveMiss::Full) => {
                    return Err(WorkspaceError::HubError(format!(
                        "too_many_uploads: session already has {MAX_STAGED_UPLOADS_PER_SESSION} staged uploads"
                    )));
                }
            };
            let begin = BeginParams {
                target,
                req_path: req.path.clone(),
                upload_id: req.upload_id.clone(),
                create_dirs: req.create_dirs,
                overwrite: req.overwrite,
            };
            let begun = tokio::task::spawn_blocking(move || begin_upload(begin))
                .await
                .map_err(|e| WorkspaceError::JoinError(e.to_string()));
            match begun {
                Ok(Ok(upload)) => (upload, staged_elsewhere),
                Ok(Err(e)) | Err(e) => {
                    uploads.release(upload_id);
                    return Err(e);
                }
            }
        }
    };

    let chunk = ChunkParams {
        req_path: req.path.clone(),
        content_base64: req.content_base64.clone(),
        offset: req.offset,
        finalize: req.finalize,
        session_budget: MAX_STAGED_BYTES_PER_SESSION.saturating_sub(staged_elsewhere),
    };
    // Decode, append and (on finalize) fsync + rename are blocking; the upload is dropped inside on any error.
    let outcome = tokio::task::spawn_blocking(move || append_chunk(upload, chunk))
        .await
        .map_err(|e| WorkspaceError::JoinError(e.to_string()));
    match outcome {
        Ok(Ok(ChunkOutcome::Staged(upload))) => {
            let size = upload.len;
            match uploads.restore(upload_id, upload) {
                Ok(()) => Ok(FsWriteFileRes {
                    size,
                    hash: None,
                    file_path: None,
                }),
                Err(upload) => {
                    drop(upload);
                    Err(out_of_order(format!(
                        "concurrent chunk for upload {upload_id}; upload dropped"
                    )))
                }
            }
        }
        Ok(Ok(ChunkOutcome::Finalized(res))) => {
            uploads.release(upload_id);
            tracing::info!(session_id, path = %req.path, size = res.size,
                "client-fs write finalized");
            Ok(res)
        }
        Ok(Err(e)) | Err(e) => {
            uploads.release(upload_id);
            Err(e)
        }
    }
}

/// Resolve `path` against the base of `session`, which the caller already holds, and confirm the session is still the one bound under `session_id`.
/// Resolving through the held `Arc` (not the session map) means an eviction during the await cannot rebase the write onto the workspace root.
async fn resolve_for_session(
    ws: &WorkspaceHandle,
    session_id: &str,
    session: &Arc<WorkspaceSession>,
    path: &str,
) -> WorkspaceResult<(PathBuf, ClientFsBase)> {
    let base = ws
        .client_fs_base_for_session(Some(session_id), Some(session))
        .await?;
    let target = resolve_in_base(&base, path).await?;
    let still_bound = ws
        .session(session_id)
        .is_some_and(|live| Arc::ptr_eq(&live, session));
    if !still_bound {
        return Err(WorkspaceError::SessionNotFound(session_id.to_owned()));
    }
    Ok((target, base))
}

struct BeginParams {
    target: PathBuf,
    req_path: String,
    upload_id: String,
    create_dirs: bool,
    overwrite: bool,
}

/// First-chunk setup: refuse an existing target unless `overwrite`, create parents, and create the staging file with `O_EXCL`.
fn begin_upload(params: BeginParams) -> WorkspaceResult<StagedUpload> {
    let BeginParams {
        target,
        req_path,
        upload_id,
        create_dirs,
        overwrite,
    } = params;
    let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
        return Err(WorkspaceError::HubError(format!("not a file: {req_path}")));
    };
    let file_path = target
        .to_str()
        .ok_or_else(|| WorkspaceError::HubError(format!("target path is not UTF-8: {req_path}")))?
        .to_owned();
    let staging_name = format!(".{}{STAGING_INFIX}{upload_id}", name.to_string_lossy());
    if staging_name.len() > MAX_FILE_NAME_BYTES {
        return Err(WorkspaceError::HubError(format!(
            "name_too_long: staging name for {req_path} exceeds {MAX_FILE_NAME_BYTES} bytes"
        )));
    }
    match std::fs::symlink_metadata(&target) {
        Ok(md) if md.is_dir() => {
            return Err(WorkspaceError::HubError(format!(
                "not a file: {req_path} is a directory"
            )));
        }
        Ok(_) if !overwrite => {
            return Err(WorkspaceError::HubError(format!(
                "exists: {req_path} already exists; set overwrite to replace it"
            )));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(WorkspaceError::HubError(format!(
                "stat failed for {req_path}: {e}"
            )));
        }
    }
    if create_dirs {
        std::fs::create_dir_all(parent).map_err(|e| {
            WorkspaceError::HubError(format!("failed to create directories for {req_path}: {e}"))
        })?;
    }
    // Same directory as the target, so the finalize rename stays on one filesystem.
    let temp = tempfile::Builder::new()
        .prefix(&staging_name)
        .rand_bytes(0)
        .tempfile_in(parent)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                WorkspaceError::HubError(format!(
                    "staging_conflict: {staging_name} already exists next to {req_path}"
                ))
            } else {
                WorkspaceError::HubError(format!("failed to stage {req_path}: {e}"))
            }
        })?;
    Ok(StagedUpload {
        target,
        file_path,
        temp,
        len: 0,
        hasher: Sha256::new(),
        overwrite,
        last_seen: Instant::now(),
    })
}

struct ChunkParams {
    req_path: String,
    content_base64: String,
    offset: u64,
    finalize: bool,
    /// Bytes this upload may still grow to before the session hits [`MAX_STAGED_BYTES_PER_SESSION`].
    session_budget: u64,
}

enum ChunkOutcome {
    Staged(StagedUpload),
    Finalized(FsWriteFileRes),
}

/// Append one decoded chunk; on `finalize`, fsync, rename onto the target and fsync its directory.
/// Any error drops `upload`, unlinking the staging file.
fn append_chunk(mut upload: StagedUpload, chunk: ChunkParams) -> WorkspaceResult<ChunkOutcome> {
    let ChunkParams {
        req_path,
        content_base64,
        offset,
        finalize,
        session_budget,
    } = chunk;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(content_base64)
        .map_err(|e| WorkspaceError::HubError(format!("invalid chunk base64: {e}")))?;
    if bytes.len() > MAX_CLIENT_FS_WRITE_CHUNK_BYTES {
        return Err(WorkspaceError::HubError(format!(
            "chunk_too_large: chunk exceeds {MAX_CLIENT_FS_WRITE_CHUNK_BYTES} bytes"
        )));
    }
    if offset != upload.len {
        // A retry of the chunk that was staged last (same span, and it did not finalize, or the upload would be gone) is a no-op.
        let is_replay = !finalize
            && offset
                .checked_add(bytes.len() as u64)
                .is_some_and(|end| end == upload.len);
        if is_replay {
            upload.last_seen = Instant::now();
            return Ok(ChunkOutcome::Staged(upload));
        }
        return Err(out_of_order(format!(
            "chunk offset {offset} does not match {} bytes staged for {req_path}; upload dropped",
            upload.len
        )));
    }
    let new_len = upload.len + bytes.len() as u64;
    if new_len > MAX_CLIENT_FS_WRITE_FILE_BYTES {
        return Err(WorkspaceError::HubError(format!(
            "too_large: {req_path} exceeds {MAX_CLIENT_FS_WRITE_FILE_BYTES} bytes; upload dropped"
        )));
    }
    if new_len > session_budget {
        return Err(WorkspaceError::HubError(format!(
            "too_many_uploads: session would exceed {MAX_STAGED_BYTES_PER_SESSION} staged bytes; upload dropped"
        )));
    }
    upload
        .temp
        .as_file_mut()
        .write_all(&bytes)
        .map_err(|e| WorkspaceError::HubError(format!("write failed for {req_path}: {e}")))?;
    upload.hasher.update(&bytes);
    upload.len = new_len;
    upload.last_seen = Instant::now();
    if !finalize {
        return Ok(ChunkOutcome::Staged(upload));
    }

    let StagedUpload {
        target,
        file_path,
        temp,
        len,
        hasher,
        overwrite,
        ..
    } = upload;
    temp.as_file()
        .sync_all()
        .map_err(|e| WorkspaceError::HubError(format!("sync failed for {req_path}: {e}")))?;
    let hash = format!("{:x}", hasher.finalize());
    let persisted = if overwrite {
        temp.persist(&target)
    } else {
        temp.persist_noclobber(&target)
    };
    // A failed persist hands the temp file back inside the error; dropping it unlinks the staging file.
    persisted.map_err(|e| {
        if e.error.kind() == std::io::ErrorKind::AlreadyExists {
            WorkspaceError::HubError(format!(
                "exists: {req_path} already exists; set overwrite to replace it"
            ))
        } else {
            WorkspaceError::HubError(format!("finalize failed for {req_path}: {}", e.error))
        }
    })?;
    // Rename gives visibility, not durability of the new directory entry; unsupported filesystems fail this quietly.
    if let Some(parent) = target.parent()
        && let Ok(dir) = std::fs::File::open(parent)
    {
        let _ = dir.sync_all();
    }
    Ok(ChunkOutcome::Finalized(FsWriteFileRes {
        size: len,
        hash: Some(hash),
        file_path: Some(file_path),
    }))
}

/// Run [`StagedUploads::gc`] over every session; returns the number of uploads dropped.
pub(crate) fn gc_staged_uploads(ws: &WorkspaceHandle, now: Instant, max_idle: Duration) -> usize {
    let sessions: Vec<_> = ws.shared.sessions.read().values().cloned().collect();
    sessions
        .iter()
        .map(|session| session.staged_uploads().gc(now, max_idle))
        .sum()
}

/// Whether `name` is a staging file name (`.<name>.a3s-upload-<uploadId>`); returns the id.
fn staging_upload_id(name: &str) -> Option<&str> {
    let (_, upload_id) = name.rsplit_once(STAGING_INFIX)?;
    (name.starts_with('.') && validate_upload_id(upload_id).is_ok()).then_some(upload_id)
}

/// Whether `upload_id` is staged or in flight for any session of `ws`.
fn upload_id_is_live(ws: &WorkspaceHandle, upload_id: &str) -> bool {
    ws.shared
        .sessions
        .read()
        .values()
        .any(|session| session.staged_uploads().contains(upload_id))
}

/// Delete staging files under `root` whose id `is_live` does not claim; returns how many were removed.
/// An upload registers its id before creating its staging file, so a file whose id is unregistered when it is examined is an orphan.
/// Symlinks are neither followed nor matched, and [`ORPHAN_SWEEP_SKIP_DIRS`] are not entered.
pub(crate) fn remove_orphaned_staging(root: &Path, is_live: &dyn Fn(&str) -> bool) -> usize {
    let walker = ignore::WalkBuilder::new(root)
        .standard_filters(false)
        .hidden(false)
        .follow_links(false)
        .same_file_system(true)
        .filter_entry(|entry| {
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            let skipped = entry
                .file_name()
                .to_str()
                .is_some_and(|name| ORPHAN_SWEEP_SKIP_DIRS.contains(&name));
            !(is_dir && skipped) || entry.depth() == 0
        })
        .build();
    let mut removed = 0usize;
    for (visited, entry) in walker.enumerate() {
        if visited >= ORPHAN_SWEEP_MAX_ENTRIES {
            tracing::warn!(root = %root.display(), limit = ORPHAN_SWEEP_MAX_ENTRIES,
                "client-fs orphan sweep stopped at the entry limit");
            break;
        }
        let Ok(entry) = entry else {
            continue;
        };
        let is_regular_file = entry
            .file_type()
            .is_some_and(|t| t.is_file() && !t.is_symlink());
        if !is_regular_file {
            continue;
        }
        let Some(upload_id) = entry.file_name().to_str().and_then(staging_upload_id) else {
            continue;
        };
        if is_live(upload_id) {
            continue;
        }
        match std::fs::remove_file(entry.path()) {
            Ok(()) => {
                removed += 1;
                tracing::info!(path = %entry.path().display(), "removed orphaned client-fs staging file");
            }
            Err(e) => {
                tracing::warn!(path = %entry.path().display(), error = %e,
                    "failed to remove orphaned client-fs staging file");
            }
        }
    }
    removed
}

/// [`remove_orphaned_staging`] under the workspace root, sparing every upload registered with one of its sessions.
pub(crate) fn sweep_orphaned_staging(ws: &WorkspaceHandle) -> WorkspaceResult<usize> {
    let root = ws.root_cwd()?;
    Ok(remove_orphaned_staging(&root, &|upload_id| {
        upload_id_is_live(ws, upload_id)
    }))
}

/// Start the daemon's staged-upload maintenance: sweep orphans under the root once, then GC stale uploads every [`STAGED_UPLOAD_GC_TICK`].
/// Returns `(sweep, gc)`: the sweep resolves to the number of files removed; the GC ticker runs until the workspace is dropped.
/// `None` when `WORKSPACE_CLIENT_FS_QUERIES` disables the client-fs ops: nothing is swept and no ticker runs.
/// The GC task holds a `Weak` handle and exits when the workspace is dropped; the sweep holds a strong one for its bounded run.
pub(crate) fn spawn_staged_upload_maintenance(
    ws: &WorkspaceHandle,
) -> Option<(tokio::task::JoinHandle<usize>, tokio::task::JoinHandle<()>)> {
    if !crate::hub_server::client_fs_queries_enabled() {
        tracing::info!("client-fs staged upload maintenance disabled with the client-fs ops");
        return None;
    }
    let sweep_ws = ws.clone();
    let sweep = tokio::task::spawn_blocking(move || match sweep_orphaned_staging(&sweep_ws) {
        Ok(removed) => {
            if removed > 0 {
                tracing::info!(removed, "client-fs orphan sweep complete");
            }
            removed
        }
        Err(error) => {
            tracing::warn!(%error, "client-fs orphan sweep skipped");
            0
        }
    });
    let shared = std::sync::Arc::downgrade(&ws.shared);
    let gc = tokio::spawn(async move {
        let mut tick = tokio::time::interval(STAGED_UPLOAD_GC_TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let Some(shared) = shared.upgrade() else {
                return;
            };
            let ws = WorkspaceHandle { shared };
            let dropped = tokio::task::spawn_blocking(move || {
                gc_staged_uploads(&ws, Instant::now(), STAGED_UPLOAD_MAX_IDLE)
            })
            .await
            .unwrap_or(0);
            if dropped > 0 {
                tracing::info!(dropped, "client-fs staged upload gc");
            }
        }
    });
    Some((sweep, gc))
}

#[cfg(test)]
mod tests {
    use a3s_code_workspace_types::rpc::fs::FsReadEncoding;
    use base64::Engine;

    use super::*;
    use crate::handle::tests::make_handle;

    fn list_req(path: &str) -> FsListReq {
        FsListReq {
            path: path.to_owned(),
            depth: 1,
            include_hidden: true,
            limit: 1000,
            offset: 0,
            follow_symlinks: true,
            respect_git_ignore: false,
            include_globs: vec![],
            exclude_globs: vec![],
        }
    }

    /// Fixture: root with files `b.txt`, `A.txt`, `c.txt` and dirs `Zeta`, `alpha`.
    /// Expected order: dirs first case-insensitive (`alpha`, `Zeta`), then files (`A.txt`, `b.txt`, `c.txt`).
    fn populate(root: &Path) {
        std::fs::write(root.join("b.txt"), b"bb").unwrap();
        std::fs::write(root.join("A.txt"), b"a").unwrap();
        std::fs::write(root.join("c.txt"), b"ccc").unwrap();
        std::fs::create_dir(root.join("Zeta")).unwrap();
        std::fs::create_dir(root.join("alpha")).unwrap();
    }

    /// `list_blocking` against `dir` as both walk root and workspace root (canonicalized for the confinement check, like production).
    fn list_dir(dir: &Path, req: &FsListReq, max_collect: usize) -> FsListRes {
        let canonical = dunce::canonicalize(dir).unwrap();
        list_blocking(dir, dir, &canonical, req, max_collect).unwrap()
    }

    #[test]
    fn list_sorts_dirs_first_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        populate(dir.path());
        let res = list_dir(dir.path(), &list_req(""), MAX_LIST_COLLECT);
        let names: Vec<&str> = res.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["alpha", "Zeta", "A.txt", "b.txt", "c.txt"]);
        assert!(!res.truncated);
        assert_eq!(
            res.nodes.first().map(|n| n.node_type),
            Some(FsNodeType::Directory)
        );
        assert_eq!(
            res.nodes.get(2).map(|n| n.node_type),
            Some(FsNodeType::File)
        );
        assert_eq!(res.nodes.get(2).and_then(|n| n.size), Some(1));
        assert!(res.nodes.get(2).is_some_and(|n| n.mtime_ms.is_some()));
        // Paths are workspace-root-relative.
        assert_eq!(res.nodes.get(2).map(|n| n.path.as_str()), Some("A.txt"));
    }

    /// Pagination slices the *sorted* listing, so consecutive pages have stable boundaries and concatenate to the full listing.
    #[test]
    fn list_paginates_post_sort_with_stable_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        populate(dir.path());
        let full = list_dir(dir.path(), &list_req(""), MAX_LIST_COLLECT);

        let mut paged = Vec::new();
        for page_start in [0u64, 2, 4] {
            let req = FsListReq {
                limit: 2,
                offset: page_start,
                ..list_req("")
            };
            let page = list_dir(dir.path(), &req, MAX_LIST_COLLECT);
            // truncated while more entries remain past this page.
            assert_eq!(page.truncated, page_start + 2 < full.nodes.len() as u64);
            paged.extend(page.nodes);
        }
        assert_eq!(paged, full.nodes);

        // Offset past the end yields an empty, non-truncated page.
        let req = FsListReq {
            offset: 100,
            ..list_req("")
        };
        let page = list_dir(dir.path(), &req, MAX_LIST_COLLECT);
        assert!(page.nodes.is_empty());
        assert!(!page.truncated);
    }

    /// The collection cap marks the result truncated even when the page itself is not full.
    #[test]
    fn list_collection_cap_truncates() {
        let dir = tempfile::tempdir().unwrap();
        populate(dir.path());
        let res = list_dir(dir.path(), &list_req(""), 2);
        assert_eq!(res.nodes.len(), 2);
        assert!(res.truncated);
    }

    #[test]
    fn list_caps_limit_at_server_max() {
        let dir = tempfile::tempdir().unwrap();
        populate(dir.path());
        let req = FsListReq {
            limit: u32::MAX,
            ..list_req("")
        };
        // Must not panic or overflow; the page is everything (fewer than 1000)
        let res = list_dir(dir.path(), &req, MAX_LIST_COLLECT);
        assert_eq!(res.nodes.len(), 5);
    }

    /// Regression: the list walk must not traverse (or even list) in-root symlinks that resolve outside the workspace root.
    /// Symlinks staying inside the root keep working.
    #[test]
    #[cfg(unix)]
    fn list_excludes_symlink_escapes_mid_walk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();
        // Escaping symlink: root/escape_link -> <outside>.
        std::os::unix::fs::symlink(outside.path(), root.join("escape_link")).unwrap();
        // In-root symlink: root/good_link -> root/real_dir.
        std::fs::create_dir(root.join("real_dir")).unwrap();
        std::fs::write(root.join("real_dir/inner.txt"), b"inner").unwrap();
        std::os::unix::fs::symlink(root.join("real_dir"), root.join("good_link")).unwrap();

        let req = FsListReq {
            depth: 2,
            follow_symlinks: true,
            ..list_req("")
        };
        let res = list_dir(root, &req, MAX_LIST_COLLECT);
        let paths: Vec<&str> = res.nodes.iter().map(|n| n.path.as_str()).collect();
        assert!(
            !paths.iter().any(|p| p.contains("escape_link")),
            "escaping symlink (and its subtree) must be excluded: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.contains("secret.txt")),
            "outside entries must not be enumerated: {paths:?}"
        );
        // Confinement must not over-filter: in-root symlinks survive, including descent through them
        assert!(paths.contains(&"good_link"), "{paths:?}");
        assert!(paths.contains(&"good_link/inner.txt"), "{paths:?}");
        assert!(paths.contains(&"real_dir/inner.txt"), "{paths:?}");
        let good = res.nodes.iter().find(|n| n.path == "good_link").unwrap();
        assert_eq!(good.is_symlink, Some(true));
    }

    #[test]
    fn memo_lookup_hits_and_invalidates_on_mismatch() {
        let memo = FileHashMemo::default();
        let path = Path::new("/ws/a.txt");
        memo.store(path, 10, 1000, "h1".into());
        assert_eq!(memo.lookup(path, 10, 1000).as_deref(), Some("h1"));
        // A size change is a miss
        assert_eq!(memo.lookup(path, 11, 1000), None);
        // An mtime change is a miss
        assert_eq!(memo.lookup(path, 10, 2000), None);
        // Re-store replaces the stale entry.
        memo.store(path, 11, 2000, "h2".into());
        assert_eq!(memo.lookup(path, 11, 2000).as_deref(), Some("h2"));
        assert_eq!(memo.lookup(path, 10, 1000), None);
    }

    /// `stat` consults the memo (no re-hash for an unchanged file) and recomputes when `(size, mtime)` no longer match.
    #[tokio::test]
    async fn stat_uses_memo_until_file_changes() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        std::fs::write(root.join("data.txt"), b"hello world").unwrap();

        let req = FsStatReq {
            path: "data.txt".into(),
        };
        let first = stat(&ws, None, &req).await.unwrap();
        assert!(first.exists);
        assert_eq!(first.node_type, Some(FsNodeType::File));
        assert_eq!(first.size, Some(11));
        let real_hash = first.hash.clone().expect("hash for files");

        // Plant a sentinel hash for the file's current (size, mtime)
        // A second stat must return the sentinel, proof it did not re-hash
        let abs = root.join("data.txt");
        let md = std::fs::metadata(&abs).unwrap();
        let mtime = system_time_ms(md.modified().unwrap());
        ws.shared
            .client_fs_hash_memo
            .store(&abs, md.len(), mtime, "sentinel".into());
        let memoized = stat(&ws, None, &req).await.unwrap();
        assert_eq!(memoized.hash.as_deref(), Some("sentinel"));

        // A size change invalidates the memo entry and re-hashes.
        std::fs::write(&abs, b"hello brave new world").unwrap();
        let rehashed = stat(&ws, None, &req).await.unwrap();
        let new_hash = rehashed.hash.expect("hash for files");
        assert_ne!(new_hash, "sentinel");
        assert_ne!(new_hash, real_hash);
    }

    /// A path with a *file* as an intermediate component (`ENOTDIR`) is an existence miss, not an RPC error.
    #[tokio::test]
    async fn stat_enotdir_intermediate_reports_not_exists() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        std::fs::write(root.join("file.txt"), b"x").unwrap();
        let res = stat(
            &ws,
            None,
            &FsStatReq {
                path: "file.txt/nested".into(),
            },
        )
        .await
        .unwrap();
        assert!(!res.exists);
        assert_eq!(res.node_type, None);
        assert_eq!(res.hash, None);
    }

    #[tokio::test]
    async fn stat_missing_path_reports_not_exists() {
        let ws = make_handle();
        let res = stat(
            &ws,
            None,
            &FsStatReq {
                path: "nope.txt".into(),
            },
        )
        .await
        .unwrap();
        assert!(!res.exists);
        assert_eq!(res.node_type, None);
        assert_eq!(res.hash, None);
    }

    #[tokio::test]
    async fn read_file_chunks_are_binary_safe_and_capped() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        // Non-UTF-8 payload: every byte value once.
        let payload: Vec<u8> = (0u8..=255).collect();
        std::fs::write(root.join("blob.bin"), &payload).unwrap();

        let req = FsReadFileReq {
            path: "blob.bin".into(),
            // Bytes 200..210 are bare continuation bytes, never valid UTF-8
            offset: Some(200),
            length: Some(50),
            max_bytes: 10, // cap below the requested length
            encoding: FsReadEncoding::Base64,
        };
        let res = read_file(&ws, None, &req).await.unwrap();
        assert_eq!(res.size, 256);
        assert_eq!(res.content, None);
        assert_eq!(res.content_type, FsContentType::Binary);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(res.content_base64.unwrap())
            .unwrap();
        assert_eq!(
            bytes,
            payload.get(200..210).unwrap_or(&[]),
            "maxBytes caps the chunk"
        );

        // Full-file hash regardless of the requested range.
        use sha2::{Digest, Sha256};
        assert_eq!(res.hash, format!("{:x}", Sha256::digest(&payload)));

        // Memoized second read (range-only fast path) returns the identical chunk and hash
        let again = read_file(&ws, None, &req).await.unwrap();
        assert_eq!(again.hash, res.hash);
        let again_bytes = base64::engine::general_purpose::STANDARD
            .decode(again.content_base64.unwrap())
            .unwrap();
        assert_eq!(again_bytes, payload.get(200..210).unwrap_or(&[]));
    }

    /// `maxBytes` is server-capped at [`MAX_READ_BYTES`]: a caller-supplied huge budget cannot make the workspace buffer the whole file.
    #[tokio::test]
    async fn read_file_server_caps_max_bytes() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        let payload = vec![0u8; (MAX_READ_BYTES + 100) as usize];
        std::fs::write(root.join("big.bin"), &payload).unwrap();

        let res = read_file(
            &ws,
            None,
            &FsReadFileReq {
                path: "big.bin".into(),
                offset: None,
                length: None,
                max_bytes: u64::MAX,
                encoding: FsReadEncoding::Base64,
            },
        )
        .await
        .unwrap();
        assert_eq!(res.size, payload.len() as u64);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(res.content_base64.unwrap())
            .unwrap();
        assert_eq!(
            bytes.len() as u64,
            MAX_READ_BYTES,
            "clamped to the server cap"
        );
    }

    #[tokio::test]
    async fn read_file_utf8_default_and_binary_fallback() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        std::fs::write(root.join("text.txt"), "héllo").unwrap();
        std::fs::write(root.join("bin.dat"), [0xff, 0xfe, 0x00]).unwrap();

        let text = read_file(
            &ws,
            None,
            &FsReadFileReq {
                path: "text.txt".into(),
                offset: None,
                length: None,
                max_bytes: 1_048_576,
                encoding: FsReadEncoding::Utf8,
            },
        )
        .await
        .unwrap();
        assert_eq!(text.content.as_deref(), Some("héllo"));
        assert_eq!(text.content_base64, None);
        assert_eq!(text.content_type, FsContentType::Text);

        // Invalid UTF-8 under the utf8 default degrades to base64.
        let bin = read_file(
            &ws,
            None,
            &FsReadFileReq {
                path: "bin.dat".into(),
                offset: None,
                length: None,
                max_bytes: 1_048_576,
                encoding: FsReadEncoding::Utf8,
            },
        )
        .await
        .unwrap();
        assert_eq!(bin.content, None);
        assert_eq!(bin.content_type, FsContentType::Binary);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(bin.content_base64.unwrap())
            .unwrap();
        assert_eq!(bytes, [0xff, 0xfe, 0x00]);
    }

    #[tokio::test]
    async fn resolve_rejects_escapes() {
        let ws = make_handle();
        for path in ["/etc/passwd", "../escape.txt"] {
            let err = stat(
                &ws,
                None,
                &FsStatReq {
                    path: path.to_owned(),
                },
            )
            .await
            .expect_err("escape must be rejected");
            assert!(matches!(err, WorkspaceError::HubError(_)), "{err:?}");
        }
    }

    /// An absolute path *inside* the workspace root is accepted and stats the same file as its root-relative form.
    #[tokio::test]
    async fn resolve_accepts_absolute_within_root() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        std::fs::write(root.join("data.txt"), b"hello").unwrap();

        let rel = stat(
            &ws,
            None,
            &FsStatReq {
                path: "data.txt".into(),
            },
        )
        .await
        .unwrap();
        assert!(rel.exists);

        let abs_path = root.join("data.txt").to_string_lossy().into_owned();
        let abs = stat(&ws, None, &FsStatReq { path: abs_path })
            .await
            .unwrap();
        assert!(abs.exists);
        assert_eq!(abs.node_type, rel.node_type);
        assert_eq!(abs.size, rel.size);
        assert_eq!(abs.hash, rel.hash);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn resolve_rejects_symlink_escape() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("escape_link")).unwrap();

        let err = read_file(
            &ws,
            None,
            &FsReadFileReq {
                path: "escape_link/secret.txt".into(),
                offset: None,
                length: None,
                max_bytes: 1_048_576,
                encoding: FsReadEncoding::Base64,
            },
        )
        .await
        .expect_err("symlink escape must be rejected");
        assert!(
            err.to_string().contains("symlink escape"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn list_empty_path_lists_root() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        std::fs::write(root.join("rooted.txt"), b"x").unwrap();
        let res = list(&ws, None, &list_req("")).await.unwrap();
        assert!(res.nodes.iter().any(|n| n.name == "rooted.txt"));
    }

    /// A session cwd that extends the root rebases the client-fs surface: paths are cwd-relative and root-level files are unreachable.
    #[tokio::test]
    async fn session_cwd_rebases_client_fs_surface() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        std::fs::create_dir(root.join("artifacts")).unwrap();
        std::fs::write(root.join("rooted.txt"), b"r").unwrap();
        std::fs::write(root.join("artifacts").join("out.txt"), b"out").unwrap();
        ws.create_session_with_cwd("cwd-session", Some(root.join("artifacts")))
            .unwrap();
        let session = Some("cwd-session");

        let res = list(&ws, session, &list_req("")).await.unwrap();
        let names: Vec<&str> = res.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["out.txt"]);
        assert_eq!(res.nodes.first().map(|n| n.path.as_str()), Some("out.txt"));

        let hit = stat(
            &ws,
            session,
            &FsStatReq {
                path: "out.txt".into(),
            },
        )
        .await
        .unwrap();
        assert!(hit.exists);
        assert_eq!(hit.size, Some(3));

        let read = read_file(
            &ws,
            session,
            &FsReadFileReq {
                path: "out.txt".into(),
                offset: None,
                length: None,
                max_bytes: 1_048_576,
                encoding: FsReadEncoding::Utf8,
            },
        )
        .await
        .unwrap();
        assert_eq!(read.content.as_deref(), Some("out"));

        let miss = stat(
            &ws,
            session,
            &FsStatReq {
                path: "rooted.txt".into(),
            },
        )
        .await
        .unwrap();
        assert!(!miss.exists, "root files are not visible under the cwd");
        let err = stat(
            &ws,
            session,
            &FsStatReq {
                path: "../rooted.txt".into(),
            },
        )
        .await
        .expect_err("escape above the session cwd must be rejected");
        assert!(matches!(err, WorkspaceError::HubError(_)), "{err:?}");
    }

    /// Root-cwd and unknown sessions keep the workspace-root base.
    #[tokio::test]
    async fn root_and_unknown_sessions_keep_root_base() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        std::fs::write(root.join("rooted.txt"), b"x").unwrap();
        for session in [Some("main"), Some("never-bound")] {
            let res = list(&ws, session, &list_req("")).await.unwrap();
            assert!(
                res.nodes.iter().any(|n| n.name == "rooted.txt"),
                "{session:?} must list the workspace root"
            );
        }
    }

    /// Unusable session cwds fall back to the root base instead of failing every op.
    /// The two cases here are a directory missing on disk (an artifacts mount not yet established) and a cwd containing `..`.
    #[tokio::test]
    async fn unusable_session_cwds_fall_back_to_root_base() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        std::fs::write(root.join("rooted.txt"), b"x").unwrap();
        ws.create_session_with_cwd("missing-dir", Some(root.join("artifacts")))
            .unwrap();
        ws.create_session_with_cwd("dot-dot", Some(root.join("..")))
            .unwrap();
        for session in [Some("missing-dir"), Some("dot-dot")] {
            let res = list(&ws, session, &list_req("")).await.unwrap();
            assert!(
                res.nodes.iter().any(|n| n.name == "rooted.txt"),
                "{session:?} must fall back to the workspace root"
            );
        }
    }

    /// A session cwd whose suffix is a symlink out of the root falls back to the root base (rebasing there would widen confinement).
    #[tokio::test]
    #[cfg(unix)]
    async fn symlink_escape_session_cwd_falls_back_to_root_base() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        std::fs::write(root.join("rooted.txt"), b"x").unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("escape_link")).unwrap();
        ws.create_session_with_cwd("escape", Some(root.join("escape_link")))
            .unwrap();
        let res = list(&ws, Some("escape"), &list_req("")).await.unwrap();
        assert!(res.nodes.iter().any(|n| n.name == "rooted.txt"));
    }

    // ---- client_fs_write_file ------------------------------------------------------------------

    const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

    fn sha256_hex(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn write_req(path: &str, upload_id: &str, bytes: &[u8], offset: u64) -> FsWriteFileReq {
        FsWriteFileReq {
            path: path.to_owned(),
            upload_id: upload_id.to_owned(),
            content_base64: B64.encode(bytes),
            offset,
            finalize: false,
            create_dirs: true,
            overwrite: false,
        }
    }

    fn staging_files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(STAGING_INFIX))
            .collect();
        names.sort();
        names
    }

    /// Upload `bytes` in `chunks` pieces and finalize; returns the finalize response.
    async fn upload_in_chunks(
        ws: &WorkspaceHandle,
        session: &str,
        path: &str,
        upload_id: &str,
        bytes: &[u8],
        chunks: usize,
    ) -> FsWriteFileRes {
        let chunk_len = bytes.len().div_ceil(chunks).max(1);
        let pieces: Vec<&[u8]> = bytes.chunks(chunk_len).collect();
        let mut offset = 0u64;
        let mut last = None;
        for (i, piece) in pieces.iter().enumerate() {
            let req = FsWriteFileReq {
                finalize: i + 1 == pieces.len(),
                ..write_req(path, upload_id, piece, offset)
            };
            let res = write_file(ws, Some(session), &req).await.unwrap();
            offset += piece.len() as u64;
            assert_eq!(res.size, offset);
            last = Some(res);
        }
        last.expect("at least one chunk")
    }

    #[tokio::test]
    async fn write_file_requires_bound_session() {
        let ws = make_handle();
        let req = write_req("a.bin", "u1", b"x", 0);
        let err = write_file(&ws, None, &req).await.unwrap_err();
        assert!(
            err.to_string().contains("requires a bound session"),
            "{err}"
        );
        let err = write_file(&ws, Some("never-bound"), &req)
            .await
            .unwrap_err();
        assert!(matches!(err, WorkspaceError::SessionNotFound(_)), "{err:?}");
        assert!(!ws.root_cwd().unwrap().join("a.bin").exists());
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn write_file_rejects_escapes() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("escape_link")).unwrap();

        for (path, needle) in [
            ("../x", "escapes workspace root"),
            ("/etc/passwd", "escapes workspace root"),
            ("escape_link/x.bin", "symlink escape"),
        ] {
            let req = FsWriteFileReq {
                finalize: true,
                ..write_req(path, "esc", b"pwned", 0)
            };
            let err = write_file(&ws, Some("main"), &req)
                .await
                .expect_err("escape must be rejected");
            assert!(err.to_string().contains(needle), "{path}: {err}");
        }
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
        assert!(staging_files(&root).is_empty());
        assert_eq!(ws.session("main").unwrap().staged_uploads().len(), 0);

        // The base itself is not a writable file.
        for path in ["", "."] {
            let err = write_file(&ws, Some("main"), &write_req(path, "base", b"x", 0))
                .await
                .expect_err("base must be rejected");
            assert!(err.to_string().contains("not a file"), "{path:?}: {err}");
        }
        assert!(
            staging_files(root.parent().unwrap()).is_empty(),
            "no staging file may land beside the root"
        );

        // The upload id is a file-name component and is restricted accordingly.
        for upload_id in ["", "../x", "a/b", &"x".repeat(65)] {
            let err = write_file(&ws, Some("main"), &write_req("ok.bin", upload_id, b"x", 0))
                .await
                .expect_err("bad upload id must be rejected");
            assert!(err.to_string().contains("invalid_upload_id"), "{err}");
        }
    }

    #[tokio::test]
    async fn write_file_stages_and_finalizes_atomically() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        let target = root.join("out").join("data.bin");

        let first = write_file(
            &ws,
            Some("main"),
            &write_req("out/data.bin", "up-1", b"hello ", 0),
        )
        .await
        .unwrap();
        assert_eq!(
            first,
            FsWriteFileRes {
                size: 6,
                hash: None,
                file_path: None
            }
        );
        assert!(
            !target.exists(),
            "nothing visible at the target before finalize"
        );
        assert_eq!(
            staging_files(&root.join("out")),
            [".data.bin.grok-upload-up-1"],
            "the staging file sits beside the target"
        );
        assert_eq!(ws.session("main").unwrap().staged_uploads().len(), 1);

        let done = write_file(
            &ws,
            Some("main"),
            &FsWriteFileReq {
                finalize: true,
                ..write_req("out/data.bin", "up-1", b"world", 6)
            },
        )
        .await
        .unwrap();
        assert_eq!(done.size, 11);
        assert_eq!(
            done.hash.as_deref(),
            Some(sha256_hex(b"hello world").as_str())
        );
        assert_eq!(
            done.file_path.as_deref(),
            Some(target.to_str().unwrap()),
            "finalize reports the absolute host path"
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"hello world");
        assert!(
            staging_files(&root.join("out")).is_empty(),
            "temp removed after finalize"
        );
        assert_eq!(ws.session("main").unwrap().staged_uploads().len(), 0);
    }

    #[tokio::test]
    async fn write_file_single_chunk_finalize() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        let bytes = b"\x00\x01\xff\xfeone-shot\x00";
        let res = write_file(
            &ws,
            Some("main"),
            &FsWriteFileReq {
                finalize: true,
                ..write_req("one.bin", "single", bytes, 0)
            },
        )
        .await
        .unwrap();
        assert_eq!(res.size, bytes.len() as u64);
        assert_eq!(res.hash, Some(sha256_hex(bytes)));
        assert_eq!(
            res.file_path.as_deref(),
            Some(root.join("one.bin").to_str().unwrap())
        );
        assert_eq!(std::fs::read(root.join("one.bin")).unwrap(), bytes);
        assert!(staging_files(&root).is_empty());
        assert_eq!(ws.session("main").unwrap().staged_uploads().len(), 0);

        // An empty file is a single finalizing chunk with no bytes.
        let res = write_file(
            &ws,
            Some("main"),
            &FsWriteFileReq {
                finalize: true,
                ..write_req("empty.bin", "empty", b"", 0)
            },
        )
        .await
        .unwrap();
        assert_eq!(res.size, 0);
        assert_eq!(res.hash, Some(sha256_hex(b"")));
        assert_eq!(std::fs::read(root.join("empty.bin")).unwrap(), b"");
    }

    #[tokio::test]
    async fn write_file_refuses_overwrite_by_default() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        std::fs::write(root.join("keep.txt"), b"original").unwrap();

        let err = write_file(
            &ws,
            Some("main"),
            &FsWriteFileReq {
                finalize: true,
                ..write_req("keep.txt", "ow-1", b"replacement", 0)
            },
        )
        .await
        .expect_err("existing target must be refused");
        assert!(err.to_string().contains("exists"), "{err}");
        assert_eq!(std::fs::read(root.join("keep.txt")).unwrap(), b"original");
        assert!(staging_files(&root).is_empty(), "refused before staging");
        assert_eq!(ws.session("main").unwrap().staged_uploads().len(), 0);

        // A directory at the target is refused even with overwrite.
        std::fs::create_dir(root.join("adir")).unwrap();
        let err = write_file(
            &ws,
            Some("main"),
            &FsWriteFileReq {
                overwrite: true,
                finalize: true,
                ..write_req("adir", "ow-dir", b"x", 0)
            },
        )
        .await
        .expect_err("directory target must be refused");
        assert!(err.to_string().contains("not a file"), "{err}");
        assert!(root.join("adir").is_dir());

        // A file that appears between the first chunk and finalize is still protected.
        write_file(&ws, Some("main"), &write_req("late.txt", "ow-2", b"new", 0))
            .await
            .unwrap();
        std::fs::write(root.join("late.txt"), b"raced in").unwrap();
        let err = write_file(
            &ws,
            Some("main"),
            &FsWriteFileReq {
                finalize: true,
                ..write_req("late.txt", "ow-2", b"er", 3)
            },
        )
        .await
        .expect_err("noclobber finalize must fail");
        assert!(err.to_string().contains("exists"), "{err}");
        assert_eq!(std::fs::read(root.join("late.txt")).unwrap(), b"raced in");
        assert!(
            staging_files(&root).is_empty(),
            "temp removed after failed finalize"
        );

        // Opt-in overwrite replaces the content atomically.
        let res = write_file(
            &ws,
            Some("main"),
            &FsWriteFileReq {
                overwrite: true,
                finalize: true,
                ..write_req("keep.txt", "ow-3", b"replacement", 0)
            },
        )
        .await
        .unwrap();
        assert_eq!(res.hash, Some(sha256_hex(b"replacement")));
        assert_eq!(
            std::fs::read(root.join("keep.txt")).unwrap(),
            b"replacement"
        );
        assert!(staging_files(&root).is_empty());
    }

    #[tokio::test]
    async fn write_file_enforces_chunk_and_file_caps() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();

        // Chunk cap: one byte over is refused and nothing is staged.
        let over = vec![0u8; MAX_CLIENT_FS_WRITE_CHUNK_BYTES + 1];
        let err = write_file(&ws, Some("main"), &write_req("big.bin", "chunk", &over, 0))
            .await
            .expect_err("oversized chunk must be refused");
        assert!(err.to_string().contains("chunk_too_large"), "{err}");
        assert!(staging_files(&root).is_empty());
        assert_eq!(ws.session("main").unwrap().staged_uploads().len(), 0);

        // Exactly the cap is accepted.
        let exact = vec![7u8; MAX_CLIENT_FS_WRITE_CHUNK_BYTES];
        let res = write_file(&ws, Some("main"), &write_req("big.bin", "exact", &exact, 0))
            .await
            .unwrap();
        assert_eq!(res.size, MAX_CLIENT_FS_WRITE_CHUNK_BYTES as u64);

        // File cap: the decoded running length is checked against the per-file cap and the upload is dropped.
        let session = ws.session("main").unwrap();
        {
            let mut entries = session.staged_uploads().entries.lock();
            let Some(UploadSlot::Idle(upload)) = entries.get_mut("exact") else {
                panic!("upload must be idle");
            };
            upload.len = MAX_CLIENT_FS_WRITE_FILE_BYTES - 1;
        }
        let err = write_file(
            &ws,
            Some("main"),
            &write_req(
                "big.bin",
                "exact",
                b"\x00\x00",
                MAX_CLIENT_FS_WRITE_FILE_BYTES - 1,
            ),
        )
        .await
        .expect_err("file cap must be enforced");
        assert!(err.to_string().contains("too_large"), "{err}");
        assert!(
            staging_files(&root).is_empty(),
            "temp removed when the cap trips"
        );
        assert_eq!(session.staged_uploads().len(), 0);
        assert!(!root.join("big.bin").exists());
    }

    #[tokio::test]
    async fn write_file_rejects_out_of_order_offset() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        let session = ws.session("main").unwrap();

        // A non-zero offset with nothing staged.
        let err = write_file(&ws, Some("main"), &write_req("o.bin", "ooo", b"abc", 3))
            .await
            .expect_err("no upload to continue");
        assert!(err.to_string().contains("out_of_order"), "{err}");
        assert!(staging_files(&root).is_empty());

        // A gap after a staged chunk drops the upload and its temp file.
        write_file(&ws, Some("main"), &write_req("o.bin", "ooo", b"abc", 0))
            .await
            .unwrap();
        assert_eq!(staging_files(&root), [".o.bin.grok-upload-ooo"]);
        let err = write_file(&ws, Some("main"), &write_req("o.bin", "ooo", b"def", 5))
            .await
            .expect_err("gap must be rejected");
        assert!(err.to_string().contains("out_of_order"), "{err}");
        assert!(
            staging_files(&root).is_empty(),
            "temp removed on out_of_order"
        );
        assert_eq!(session.staged_uploads().len(), 0);

        // Single writer: a chunk arriving while another chunk of the same id is in flight fails, and the in-flight writer loses the upload too.
        write_file(&ws, Some("main"), &write_req("o.bin", "busy", b"abc", 0))
            .await
            .unwrap();
        let uploads = session.staged_uploads();
        let (in_flight, _) = uploads.take("busy").ok().expect("checked out");
        let err = write_file(&ws, Some("main"), &write_req("o.bin", "busy", b"def", 3))
            .await
            .expect_err("concurrent chunk must be rejected");
        assert!(err.to_string().contains("out_of_order"), "{err}");
        let returned = uploads
            .restore("busy", in_flight)
            .expect_err("the entry was dropped under the in-flight writer");
        drop(returned);
        assert!(staging_files(&root).is_empty());
        assert_eq!(uploads.len(), 0);
        assert!(!root.join("o.bin").exists());
    }

    /// The ids are per session: two sessions may stage the same id for different targets.
    #[tokio::test]
    async fn write_file_upload_ids_are_session_scoped() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        ws.create_session_with_cwd("cwd-session", Some(root.join("sub")))
            .unwrap();

        write_file(&ws, Some("main"), &write_req("a.bin", "shared", b"main", 0))
            .await
            .unwrap();
        let done = write_file(
            &ws,
            Some("cwd-session"),
            &FsWriteFileReq {
                finalize: true,
                ..write_req("a.bin", "shared", b"sub", 0)
            },
        )
        .await
        .unwrap();
        assert_eq!(
            done.file_path.as_deref(),
            Some(root.join("sub").join("a.bin").to_str().unwrap()),
            "the session cwd rebases the write"
        );
        assert_eq!(std::fs::read(root.join("sub/a.bin")).unwrap(), b"sub");
        assert_eq!(ws.session("main").unwrap().staged_uploads().len(), 1);

        // The session cwd also bounds the write.
        let err = write_file(
            &ws,
            Some("cwd-session"),
            &write_req("../escape.bin", "esc", b"x", 0),
        )
        .await
        .expect_err("escape above the session cwd must be rejected");
        assert!(err.to_string().contains("escapes workspace root"), "{err}");
    }

    #[tokio::test]
    async fn write_file_round_trips_binary() {
        use rand::RngCore;

        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        // Random bytes with zero bytes and invalid UTF-8 forced in.
        let mut payload = vec![0u8; 3 * 1024 + 17];
        rand::rng().fill_bytes(&mut payload);
        payload.splice(0..3, [0u8, 0xff, 0xfe]);
        if let Some(byte) = payload.get_mut(100) {
            *byte = 0;
        }

        let done = upload_in_chunks(&ws, "main", "media/blob.bin", "rt", &payload, 3).await;
        assert_eq!(done.size, payload.len() as u64);
        assert_eq!(done.hash, Some(sha256_hex(&payload)));
        assert_eq!(std::fs::read(root.join("media/blob.bin")).unwrap(), payload);

        // Read it back through the read side, chunked, and compare bytes and hash.
        let mut read_back = Vec::new();
        let mut offset = 0u64;
        let mut hash: Option<String>;
        loop {
            let res = read_file(
                &ws,
                Some("main"),
                &FsReadFileReq {
                    path: "media/blob.bin".into(),
                    offset: Some(offset),
                    length: Some(1000),
                    max_bytes: 1000,
                    encoding: FsReadEncoding::Base64,
                },
            )
            .await
            .unwrap();
            assert_eq!(res.content_type, FsContentType::Binary);
            let chunk = B64.decode(res.content_base64.unwrap()).unwrap();
            hash = Some(res.hash);
            offset += chunk.len() as u64;
            read_back.extend(chunk);
            if offset >= res.size {
                break;
            }
        }
        assert_eq!(read_back, payload);
        assert_eq!(hash, done.hash);
    }

    #[tokio::test]
    async fn write_file_staging_conflict_is_reported() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        std::fs::write(root.join(".c.bin.grok-upload-dup"), b"foreign").unwrap();
        let err = write_file(&ws, Some("main"), &write_req("c.bin", "dup", b"x", 0))
            .await
            .expect_err("foreign staging file must not be reused");
        assert!(err.to_string().contains("staging_conflict"), "{err}");
        assert_eq!(
            std::fs::read(root.join(".c.bin.grok-upload-dup")).unwrap(),
            b"foreign",
            "the foreign file is left alone"
        );
        assert_eq!(ws.session("main").unwrap().staged_uploads().len(), 0);
    }

    #[tokio::test]
    async fn write_file_gc_removes_stale_staging() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        let session = ws.session("main").unwrap();
        write_file(&ws, Some("main"), &write_req("stale.bin", "old", b"abc", 0))
            .await
            .unwrap();
        write_file(&ws, Some("main"), &write_req("fresh.bin", "new", b"abc", 0))
            .await
            .unwrap();
        assert_eq!(
            staging_files(&root),
            [".fresh.bin.grok-upload-new", ".stale.bin.grok-upload-old"]
        );

        // Nothing is stale yet.
        assert_eq!(
            gc_staged_uploads(&ws, Instant::now(), STAGED_UPLOAD_MAX_IDLE),
            0
        );
        assert_eq!(session.staged_uploads().len(), 2);

        // Age "old" past the idle limit by moving the GC clock forward; "new" is touched as of that clock.
        let later = Instant::now() + STAGED_UPLOAD_MAX_IDLE + Duration::from_secs(1);
        session.staged_uploads().set_last_seen("new", later);
        assert_eq!(gc_staged_uploads(&ws, later, STAGED_UPLOAD_MAX_IDLE), 1);
        assert_eq!(staging_files(&root), [".fresh.bin.grok-upload-new"]);
        assert_eq!(session.staged_uploads().len(), 1);

        // A GC'd upload cannot be continued.
        let err = write_file(&ws, Some("main"), &write_req("stale.bin", "old", b"def", 3))
            .await
            .expect_err("dropped upload cannot continue");
        assert!(err.to_string().contains("out_of_order"), "{err}");

        // The live upload still finalizes.
        let done = write_file(
            &ws,
            Some("main"),
            &FsWriteFileReq {
                finalize: true,
                ..write_req("fresh.bin", "new", b"def", 3)
            },
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(root.join("fresh.bin")).unwrap(), b"abcdef");
        assert_eq!(done.hash, Some(sha256_hex(b"abcdef")));
        assert!(staging_files(&root).is_empty());

        // Session teardown abandons whatever is left.
        write_file(&ws, Some("main"), &write_req("gone.bin", "gone", b"abc", 0))
            .await
            .unwrap();
        assert_eq!(staging_files(&root), [".gone.bin.grok-upload-gone"]);
        session.staged_uploads().abandon_all();
        assert!(staging_files(&root).is_empty());
        assert_eq!(session.staged_uploads().len(), 0);
    }

    /// Startup sweeps staging files left by an earlier daemon life, and only those.
    #[test]
    #[cfg(unix)]
    fn write_file_orphan_sweep_removes_only_staging_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("deep/er")).unwrap();
        std::fs::write(root.join(".a.bin.grok-upload-abc"), b"orphan").unwrap();
        std::fs::write(root.join("deep/er/.b.bin.grok-upload-x_y-1"), b"orphan").unwrap();
        std::fs::write(root.join("deep/.hidden"), b"keep").unwrap();
        std::fs::write(root.join("a.bin.grok-upload-abc"), b"keep: no leading dot").unwrap();
        std::fs::write(root.join("keep.txt"), b"keep").unwrap();
        // A symlink named like a staging file is neither followed nor removed.
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("victim"), b"outside").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("victim"),
            root.join(".link.grok-upload-abc"),
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("escape_dir")).unwrap();

        assert_eq!(remove_orphaned_staging(root, &|_| false), 2);
        assert!(!root.join(".a.bin.grok-upload-abc").exists());
        assert!(!root.join("deep/er/.b.bin.grok-upload-x_y-1").exists());
        assert!(root.join("deep/.hidden").exists());
        assert!(root.join("a.bin.grok-upload-abc").exists());
        assert!(root.join("keep.txt").exists());
        assert!(
            root.join(".link.grok-upload-abc")
                .symlink_metadata()
                .is_ok()
        );
        assert!(outside.path().join("victim").exists());
        assert_eq!(staging_upload_id("plain.txt"), None);
        assert_eq!(staging_upload_id(".x.grok-upload-"), None);
        assert_eq!(staging_upload_id(".x.grok-upload-A1_-"), Some("A1_-"));
    }

    /// The sweep spares staging files whose id is registered with any session, so a sweep racing a live upload cannot unlink it.
    #[tokio::test]
    async fn write_file_orphan_sweep_spares_live_uploads() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        ws.create_session_with_cwd("other", Some(root.join("sub")))
            .unwrap();
        // Live uploads in two sessions, one of them checked out (in flight) while the sweep runs.
        write_file(
            &ws,
            Some("main"),
            &write_req("live.bin", "live-idle", b"abc", 0),
        )
        .await
        .unwrap();
        write_file(
            &ws,
            Some("main"),
            &write_req("busy.bin", "live-busy", b"abc", 0),
        )
        .await
        .unwrap();
        write_file(
            &ws,
            Some("other"),
            &write_req("sub.bin", "live-sub", b"abc", 0),
        )
        .await
        .unwrap();
        let main = ws.session("main").unwrap();
        let uploads = main.staged_uploads();
        let (in_flight, _) = uploads.take("live-busy").ok().expect("checked out");
        // Orphans: an unregistered id, and a registered id under a different name (spared until the next start).
        std::fs::write(root.join(".dead.bin.grok-upload-dead"), b"orphan").unwrap();
        std::fs::write(root.join("sub/.old.bin.grok-upload-live-idle"), b"orphan").unwrap();

        assert_eq!(sweep_orphaned_staging(&ws).unwrap(), 1);
        assert!(!root.join(".dead.bin.grok-upload-dead").exists());
        assert_eq!(
            staging_files(&root),
            [
                ".busy.bin.grok-upload-live-busy",
                ".live.bin.grok-upload-live-idle"
            ]
        );
        assert_eq!(
            staging_files(&root.join("sub")),
            [
                ".old.bin.grok-upload-live-idle",
                ".sub.bin.grok-upload-live-sub"
            ]
        );

        // The live uploads finish normally after the sweep.
        uploads.restore("live-busy", in_flight).ok().unwrap();
        for (session, path, id) in [
            ("main", "live.bin", "live-idle"),
            ("main", "busy.bin", "live-busy"),
            ("other", "sub.bin", "live-sub"),
        ] {
            let done = write_file(
                &ws,
                Some(session),
                &FsWriteFileReq {
                    finalize: true,
                    ..write_req(path, id, b"def", 3)
                },
            )
            .await
            .unwrap();
            assert_eq!(done.hash, Some(sha256_hex(b"abcdef")));
        }
        assert_eq!(std::fs::read(root.join("sub/sub.bin")).unwrap(), b"abcdef");
    }

    /// The sweep does not enter dependency or build trees, so the entry budget reaches user directories behind them.
    #[test]
    fn write_file_orphan_sweep_skips_heavy_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // More entries than the sweep budget would allow if it descended: a wide tree per skipped directory.
        for heavy in ORPHAN_SWEEP_SKIP_DIRS {
            let base = root.join(heavy);
            for i in 0..200 {
                let pkg = base.join(format!("pkg{i}"));
                std::fs::create_dir_all(&pkg).unwrap();
                for j in 0..20 {
                    std::fs::write(pkg.join(format!("f{j}")), b"").unwrap();
                }
            }
            std::fs::write(base.join(".inside.grok-upload-heavy"), b"left alone").unwrap();
        }
        // A user directory sorted after the heavy ones, and a same-named file (not a directory) that is still swept.
        std::fs::create_dir_all(root.join("zz-user/docs")).unwrap();
        std::fs::write(
            root.join("zz-user/docs/.report.pdf.grok-upload-u1"),
            b"orphan",
        )
        .unwrap();
        std::fs::write(root.join("zz-user/.node_modules.grok-upload-u2"), b"orphan").unwrap();

        assert_eq!(remove_orphaned_staging(root, &|_| false), 2);
        assert!(
            !root
                .join("zz-user/docs/.report.pdf.grok-upload-u1")
                .exists()
        );
        assert!(!root.join("zz-user/.node_modules.grok-upload-u2").exists());
        for heavy in ORPHAN_SWEEP_SKIP_DIRS {
            assert!(
                root.join(heavy).join(".inside.grok-upload-heavy").exists(),
                "{heavy} must not be entered"
            );
        }
    }

    /// `WORKSPACE_CLIENT_FS_QUERIES=0` disables the maintenance with the ops: no sweep, no GC ticker.
    /// Sync `block_on` so the env lock is not held across `.await`.
    #[test]
    fn write_file_maintenance_respects_client_fs_gate() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let ws = rt.block_on(async { make_handle() });
        let root = ws.root_cwd().unwrap();
        std::fs::write(root.join(".x.bin.grok-upload-orphan"), b"orphan").unwrap();

        {
            let _env =
                crate::LockedTestEnv::lock().set("WORKSPACE_CLIENT_FS_QUERIES", Path::new("0"));
            let maintenance = rt.block_on(async { spawn_staged_upload_maintenance(&ws) });
            assert!(maintenance.is_none(), "gated: nothing spawned");
        }
        assert!(
            root.join(".x.bin.grok-upload-orphan").exists(),
            "gated: no sweep"
        );

        let _env = crate::LockedTestEnv::lock();
        let _unset = crate::TestEnvGuard::unset("WORKSPACE_CLIENT_FS_QUERIES");
        let (sweep, gc) = rt
            .block_on(async { spawn_staged_upload_maintenance(&ws) })
            .expect("ungated: maintenance runs");
        assert_eq!(rt.block_on(sweep).unwrap(), 1);
        assert!(!root.join(".x.bin.grok-upload-orphan").exists());
        assert!(!gc.is_finished(), "the GC ticker is running");
        gc.abort();
    }

    /// A write for a session evicted between the session check and the target resolution is refused, never rebased onto the root.
    #[tokio::test]
    async fn write_file_refuses_evicted_session() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("victim.bin"), b"root file").unwrap();
        ws.create_session_with_cwd("evicted", Some(root.join("sub")))
            .unwrap();
        let session = ws.session("evicted").unwrap();
        ws.drop_session("evicted", "evicted").unwrap();

        // The base of a held session stays its cwd even though the map no longer knows it.
        let pinned = ws
            .client_fs_base_for_session(Some("evicted"), Some(&session))
            .await
            .unwrap();
        assert_eq!(pinned.base, root.join("sub"));
        let from_map = ws.client_fs_base(Some("evicted")).await.unwrap();
        assert_eq!(
            from_map.base, root,
            "reads of an unknown session keep the root base"
        );

        // The write path refuses once the held session is no longer the bound one.
        let err = resolve_for_session(&ws, "evicted", &session, "victim.bin")
            .await
            .expect_err("evicted session must be refused");
        assert!(matches!(err, WorkspaceError::SessionNotFound(_)), "{err:?}");
        let err = write_file(
            &ws,
            Some("evicted"),
            &FsWriteFileReq {
                overwrite: true,
                finalize: true,
                ..write_req("victim.bin", "ev", b"clobber", 0)
            },
        )
        .await
        .expect_err("evicted session must be refused");
        assert!(matches!(err, WorkspaceError::SessionNotFound(_)), "{err:?}");
        assert_eq!(
            std::fs::read(root.join("victim.bin")).unwrap(),
            b"root file"
        );
        assert!(!root.join("sub/victim.bin").exists());
        assert!(staging_files(&root).is_empty());
        assert!(staging_files(&root.join("sub")).is_empty());

        // A successor session under the same id is a different session: the held one is still refused.
        ws.create_session_with_cwd("evicted", Some(root.join("sub")))
            .unwrap();
        let err = resolve_for_session(&ws, "evicted", &session, "victim.bin")
            .await
            .expect_err("stale session handle must be refused");
        assert!(matches!(err, WorkspaceError::SessionNotFound(_)), "{err:?}");
        let live = ws.session("evicted").unwrap();
        let (target, _) = resolve_for_session(&ws, "evicted", &live, "victim.bin")
            .await
            .unwrap();
        assert_eq!(target, root.join("sub/victim.bin"));
    }

    /// `overwrite` is read from the first chunk only.
    #[tokio::test]
    async fn write_file_pins_overwrite_at_first_chunk() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();

        // Set on the first chunk only: the finalize replaces the file.
        std::fs::write(root.join("first.txt"), b"original").unwrap();
        write_file(
            &ws,
            Some("main"),
            &FsWriteFileReq {
                overwrite: true,
                ..write_req("first.txt", "pin-1", b"new ", 0)
            },
        )
        .await
        .unwrap();
        let done = write_file(
            &ws,
            Some("main"),
            &FsWriteFileReq {
                finalize: true,
                overwrite: false,
                ..write_req("first.txt", "pin-1", b"text", 4)
            },
        )
        .await
        .unwrap();
        assert_eq!(done.hash, Some(sha256_hex(b"new text")));
        assert_eq!(std::fs::read(root.join("first.txt")).unwrap(), b"new text");

        // Set on the last chunk only: the first chunk's noclobber still protects a file that appeared meanwhile.
        write_file(
            &ws,
            Some("main"),
            &write_req("last.txt", "pin-2", b"new ", 0),
        )
        .await
        .unwrap();
        std::fs::write(root.join("last.txt"), b"raced in").unwrap();
        let err = write_file(
            &ws,
            Some("main"),
            &FsWriteFileReq {
                finalize: true,
                overwrite: true,
                ..write_req("last.txt", "pin-2", b"text", 4)
            },
        )
        .await
        .expect_err("a late overwrite must not clobber");
        assert!(err.to_string().contains("exists"), "{err}");
        assert_eq!(std::fs::read(root.join("last.txt")).unwrap(), b"raced in");
        assert!(staging_files(&root).is_empty());
        assert_eq!(ws.session("main").unwrap().staged_uploads().len(), 0);
    }

    /// Per-session ceilings on concurrent uploads and on staged bytes.
    #[tokio::test]
    async fn write_file_enforces_per_session_ceilings() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        let session = ws.session("main").unwrap();

        for i in 0..MAX_STAGED_UPLOADS_PER_SESSION {
            write_file(
                &ws,
                Some("main"),
                &write_req(&format!("many{i}.bin"), &format!("many-{i}"), b"x", 0),
            )
            .await
            .unwrap();
        }
        assert_eq!(
            session.staged_uploads().len(),
            MAX_STAGED_UPLOADS_PER_SESSION
        );
        let err = write_file(
            &ws,
            Some("main"),
            &write_req("one-more.bin", "extra", b"x", 0),
        )
        .await
        .expect_err("count ceiling must be enforced");
        assert!(err.to_string().contains("too_many_uploads"), "{err}");
        assert_eq!(
            session.staged_uploads().len(),
            MAX_STAGED_UPLOADS_PER_SESSION
        );
        assert!(!staging_files(&root).iter().any(|n| n.contains("one-more")));
        // Existing uploads continue; another session is not affected.
        write_file(
            &ws,
            Some("main"),
            &write_req("many0.bin", "many-0", b"y", 1),
        )
        .await
        .unwrap();
        let other = ws.create_session_with_cwd("other", None).unwrap();
        write_file(
            &ws,
            Some("other"),
            &write_req("other.bin", "extra", b"x", 0),
        )
        .await
        .unwrap();
        session.staged_uploads().abandon_all();
        other.staged_uploads().abandon_all();

        // Byte ceiling across uploads: two uploads at the per-file cap already fill the session.
        for id in ["big-a", "big-b"] {
            write_file(
                &ws,
                Some("main"),
                &write_req(&format!("{id}.bin"), id, b"x", 0),
            )
            .await
            .unwrap();
            let mut entries = session.staged_uploads().entries.lock();
            let Some(UploadSlot::Idle(upload)) = entries.get_mut(id) else {
                panic!("upload must be idle");
            };
            upload.len = MAX_STAGED_BYTES_PER_SESSION / 2;
        }
        let err = write_file(&ws, Some("main"), &write_req("small.bin", "small", b"x", 0))
            .await
            .expect_err("byte ceiling must be enforced");
        assert!(err.to_string().contains("too_many_uploads"), "{err}");
        assert_eq!(
            session.staged_uploads().len(),
            2,
            "the refused upload is dropped"
        );
        assert_eq!(
            staging_files(&root),
            [
                ".big-a.bin.grok-upload-big-a",
                ".big-b.bin.grok-upload-big-b"
            ]
        );
        // An in-flight upload still counts toward the ceiling.
        let uploads = session.staged_uploads();
        let (in_flight, elsewhere) = uploads.take("big-a").ok().expect("checked out");
        assert_eq!(elsewhere, MAX_STAGED_BYTES_PER_SESSION / 2);
        let err = write_file(&ws, Some("main"), &write_req("small.bin", "small", b"x", 0))
            .await
            .expect_err("byte ceiling counts in-flight uploads");
        assert!(err.to_string().contains("too_many_uploads"), "{err}");
        uploads.restore("big-a", in_flight).ok().unwrap();
        // Freeing one upload makes room again.
        uploads.abandon_all();
        write_file(&ws, Some("main"), &write_req("small.bin", "small", b"x", 0))
            .await
            .unwrap();
    }

    /// A target whose staging name would exceed `NAME_MAX` is refused before anything is staged.
    #[tokio::test]
    async fn write_file_rejects_too_long_staging_name() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        let session = ws.session("main").unwrap();
        // 200-byte name + 1 + 13 + 64 = 278 > 255; a 100-byte name fits.
        let long = "n".repeat(200);
        let fits = "n".repeat(100);
        let upload_id = "i".repeat(MAX_UPLOAD_ID_LEN);

        let err = write_file(&ws, Some("main"), &write_req(&long, &upload_id, b"x", 0))
            .await
            .expect_err("over NAME_MAX must be refused");
        assert!(err.to_string().contains("name_too_long"), "{err}");
        assert!(staging_files(&root).is_empty());
        assert_eq!(session.staged_uploads().len(), 0);
        assert!(!root.join(&long).exists());

        let done = write_file(
            &ws,
            Some("main"),
            &FsWriteFileReq {
                finalize: true,
                ..write_req(&fits, &upload_id, b"x", 0)
            },
        )
        .await
        .unwrap();
        assert_eq!(done.size, 1);
        assert_eq!(std::fs::read(root.join(&fits)).unwrap(), b"x");
    }

    /// Later chunks must name the same target as the first; an equivalent spelling is fine, a different file drops the upload.
    #[tokio::test]
    async fn write_file_rejects_path_mismatch() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        let session = ws.session("main").unwrap();

        write_file(&ws, Some("main"), &write_req("dir/a.bin", "pm", b"abc", 0))
            .await
            .unwrap();
        // Same target, different spelling.
        let res = write_file(
            &ws,
            Some("main"),
            &write_req("./dir/a.bin", "pm", b"def", 3),
        )
        .await
        .unwrap();
        assert_eq!(res.size, 6);
        // Different target.
        let err = write_file(
            &ws,
            Some("main"),
            &FsWriteFileReq {
                finalize: true,
                ..write_req("dir/b.bin", "pm", b"ghi", 6)
            },
        )
        .await
        .expect_err("retargeting must be refused");
        assert!(err.to_string().contains("path_mismatch"), "{err}");
        assert_eq!(session.staged_uploads().len(), 0);
        assert!(staging_files(&root.join("dir")).is_empty());
        assert!(!root.join("dir/a.bin").exists());
        assert!(!root.join("dir/b.bin").exists());

        // An escaping path on a later chunk is refused and drops the upload too.
        write_file(&ws, Some("main"), &write_req("dir/c.bin", "pm2", b"abc", 0))
            .await
            .unwrap();
        let err = write_file(&ws, Some("main"), &write_req("../c.bin", "pm2", b"def", 3))
            .await
            .expect_err("escape must be refused");
        assert!(err.to_string().contains("escapes workspace root"), "{err}");
        assert_eq!(session.staged_uploads().len(), 0);
        assert!(staging_files(&root.join("dir")).is_empty());
    }

    /// A retry of the last staged chunk is a no-op; any other offset mismatch is still `out_of_order`.
    #[tokio::test]
    async fn write_file_replay_of_last_chunk_is_idempotent() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        let session = ws.session("main").unwrap();

        write_file(&ws, Some("main"), &write_req("r.bin", "rp", b"abc", 0))
            .await
            .unwrap();
        write_file(&ws, Some("main"), &write_req("r.bin", "rp", b"defg", 3))
            .await
            .unwrap();
        // Replay of the last chunk: no-op, current size, upload intact.
        let res = write_file(&ws, Some("main"), &write_req("r.bin", "rp", b"defg", 3))
            .await
            .unwrap();
        assert_eq!(res.size, 7);
        assert_eq!(session.staged_uploads().len(), 1);
        // Replay of an earlier chunk is not a retry.
        let err = write_file(&ws, Some("main"), &write_req("r.bin", "rp", b"abc", 0))
            .await
            .expect_err("earlier chunk is out of order");
        assert!(err.to_string().contains("out_of_order"), "{err}");
        assert_eq!(session.staged_uploads().len(), 0);
        assert!(staging_files(&root).is_empty());

        // A replay whose length differs from the last chunk is out of order.
        write_file(&ws, Some("main"), &write_req("r.bin", "rp2", b"abc", 0))
            .await
            .unwrap();
        write_file(&ws, Some("main"), &write_req("r.bin", "rp2", b"defg", 3))
            .await
            .unwrap();
        let err = write_file(&ws, Some("main"), &write_req("r.bin", "rp2", b"de", 3))
            .await
            .expect_err("different span is out of order");
        assert!(err.to_string().contains("out_of_order"), "{err}");
        assert_eq!(session.staged_uploads().len(), 0);

        // A replay that also asks to finalize is not a retry of a non-finalizing chunk.
        write_file(&ws, Some("main"), &write_req("r.bin", "rp3", b"abc", 0))
            .await
            .unwrap();
        let err = write_file(
            &ws,
            Some("main"),
            &FsWriteFileReq {
                finalize: true,
                ..write_req("r.bin", "rp3", b"abc", 0)
            },
        )
        .await
        .expect_err("finalizing replay is out of order");
        assert!(err.to_string().contains("out_of_order"), "{err}");
        assert!(!root.join("r.bin").exists());

        // After a replay the upload continues and finalizes with the bytes written once.
        write_file(&ws, Some("main"), &write_req("r.bin", "rp4", b"abc", 0))
            .await
            .unwrap();
        write_file(&ws, Some("main"), &write_req("r.bin", "rp4", b"abc", 0))
            .await
            .unwrap();
        let done = write_file(
            &ws,
            Some("main"),
            &FsWriteFileReq {
                finalize: true,
                ..write_req("r.bin", "rp4", b"def", 3)
            },
        )
        .await
        .unwrap();
        assert_eq!(done.hash, Some(sha256_hex(b"abcdef")));
        assert_eq!(std::fs::read(root.join("r.bin")).unwrap(), b"abcdef");
    }
}
