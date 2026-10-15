//! Bounded host-owned byte capture. This does not qualify maker settlement,
//! native isolation, a behavioral verdict, or permission to execute a candidate.

use super::manifest::{
    validate_relative_path, AcceptanceManifestV1, CandidateDeclarationV1, DigestV1, MAX_FILES,
    MAX_FILE_BYTES, MAX_PATH_BYTES, MAX_TOTAL_BYTES,
};
use super::report::AcceptanceReasonV1;
use a3s_code_core::content_digest::digest_json;
use a3s_code_core::evaluation::ExecutionTargetV1;
use a3s_code_core::workspace::source_snapshot::WorkspaceSourceSnapshotV1;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::path::Path;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix;

pub const FROZEN_SCHEMA: &str = "a3s.code.frozen-candidate.v1";
const BYTES_DOMAIN: &[u8] = b"a3s.code.frozen-candidate.bytes.v1\0";

#[derive(Debug, thiserror::Error)]
#[error("candidate freeze {}: {detail}", .reason.as_str())]
pub struct FreezeError {
    pub reason: AcceptanceReasonV1,
    pub detail: String,
    #[source]
    source: Option<std::io::Error>,
}
impl FreezeError {
    pub fn source_unavailable(detail: impl Into<String>) -> Self {
        Self::new(AcceptanceReasonV1::EvidenceIncomplete, detail)
    }
    fn new(reason: AcceptanceReasonV1, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
            source: None,
        }
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn io(error: std::io::Error) -> Self {
        Self {
            reason: AcceptanceReasonV1::FreezeIncomplete,
            detail: error.to_string(),
            source: Some(error),
        }
    }
}
type Result<T> = std::result::Result<T, FreezeError>;

/// Correlation supplied by the trusted host after original proof validation.
/// Possession or structural validity of this data is not proof of settlement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OriginalFreezeBindingV1 {
    pub maker_target: ExecutionTargetV1,
    pub effect_digest: DigestV1,
    pub original_evidence_digest: DigestV1,
    pub original_receipt_capture_digest: DigestV1,
    pub settlement_evidence_digest: DigestV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenFileV1 {
    pub path: String,
    pub byte_count: u64,
    pub file_digest: DigestV1,
    /// Source permissions are diagnostic; the host fixes the interpreter.
    pub source_mode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenCandidateV1 {
    pub schema: String,
    pub original: OriginalFreezeBindingV1,
    pub manifest_digest: DigestV1,
    pub source_snapshot_digest: DigestV1,
    pub source_generation_before: u64,
    pub source_generation_after: u64,
    pub files: Vec<FrozenFileV1>,
    pub total_bytes: u64,
    pub candidate_byte_digest: DigestV1,
    pub frozen_root_device: u64,
    pub frozen_root_inode: u64,
    pub frozen_at_ms: u64,
}
impl FrozenCandidateV1 {
    pub fn record_digest(&self) -> Result<DigestV1> {
        self.validate()?;
        digest_json("a3s.code.frozen-candidate.record.v1", self)
            .map_err(|error| invalid(error.to_string()))
            .and_then(canonical_digest)
    }
    pub fn validate(&self) -> Result<()> {
        self.original
            .maker_target
            .validate()
            .map_err(|error| invalid(error.to_string()))?;
        if self.schema != FROZEN_SCHEMA
            || self.source_generation_before != self.source_generation_after
        {
            return Err(invalid("unsupported schema or changed source generation"));
        }
        let total = validate_entries(&self.files, &maximum_declaration())?;
        if total != self.total_bytes || candidate_digest(&self.files)? != self.candidate_byte_digest
        {
            return Err(invalid("candidate record byte identity differs"));
        }
        Ok(())
    }
}

/// `observe_source` reads the existing published workspace snapshot before and
/// after capture. It must not create a Run, scanner authority or settlement
/// proof. Actual source bytes are read again independently of that metadata.
/// A failed capture leaves an unpublished private partial root for diagnosis;
/// subsequent attempts require a fresh root rather than overwriting it.
pub fn freeze_candidate<F>(
    manifest: &AcceptanceManifestV1,
    original: OriginalFreezeBindingV1,
    observe_source: F,
) -> Result<FrozenCandidateV1>
where
    F: FnMut() -> Result<WorkspaceSourceSnapshotV1>,
{
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        freeze_supported(manifest, original, observe_source)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (manifest, original, observe_source);
        Err(invalid(
            "safe candidate capture is unavailable on this platform",
        ))
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn freeze_supported<F>(
    manifest: &AcceptanceManifestV1,
    original: OriginalFreezeBindingV1,
    mut observe_source: F,
) -> Result<FrozenCandidateV1>
where
    F: FnMut() -> Result<WorkspaceSourceSnapshotV1>,
{
    validate_declaration(&manifest.candidate)?;
    original
        .maker_target
        .validate()
        .map_err(|error| invalid(error.to_string()))?;
    let maker = &manifest.environment.maker_root;
    let candidate = &manifest.environment.candidate_root;
    if maker.starts_with(candidate) || candidate.starts_with(maker) {
        return Err(invalid("maker and candidate roots overlap"));
    }
    let before = observe_source()?;
    validate_snapshot(&before, maker)?;
    let mut source_generation_after = before.index_generation;
    let (files, device, inode) = unix::capture(maker, candidate, &manifest.candidate, || {
        let after = observe_source()?;
        validate_snapshot(&after, maker)?;
        if before.snapshot_digest != after.snapshot_digest {
            return Err(race("published source snapshot changed during capture"));
        }
        source_generation_after = after.index_generation;
        Ok(())
    })?;
    let total_bytes = validate_entries(&files, &manifest.candidate)?;
    let frozen_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| invalid(error.to_string()))?
        .as_millis();
    let record = FrozenCandidateV1 {
        schema: FROZEN_SCHEMA.into(),
        original,
        manifest_digest: manifest.manifest_digest.clone(),
        source_snapshot_digest: canonical_digest(before.snapshot_digest)?,
        source_generation_before: before.index_generation,
        source_generation_after,
        candidate_byte_digest: candidate_digest(&files)?,
        files,
        total_bytes,
        frozen_root_device: device,
        frozen_root_inode: inode,
        frozen_at_ms: u64::try_from(frozen_at_ms).map_err(|_| invalid("timestamp overflow"))?,
    };
    revalidate_frozen(manifest, &record)?;
    Ok(record)
}

/// Must be called before AND after any separately qualified oracle execution.
/// No store write, candidate invocation or historical-record mutation occurs.
pub fn revalidate_frozen(
    manifest: &AcceptanceManifestV1,
    record: &FrozenCandidateV1,
) -> Result<()> {
    record.validate()?;
    validate_declaration(&manifest.candidate)?;
    if record.manifest_digest != manifest.manifest_digest {
        return Err(invalid("frozen record belongs to another manifest"));
    }
    validate_entries(&record.files, &manifest.candidate)?;
    let expected = manifest
        .candidate
        .allowed_paths
        .iter()
        .collect::<BTreeSet<_>>();
    if record
        .files
        .iter()
        .map(|file| &file.path)
        .collect::<BTreeSet<_>>()
        != expected
    {
        return Err(invalid("frozen record differs from exact allowlist"));
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    return unix::revalidate(
        &manifest.environment.candidate_root,
        record,
        &manifest.candidate,
    );
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    Err(invalid(
        "safe candidate revalidation is unavailable on this platform",
    ))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn validate_snapshot(snapshot: &WorkspaceSourceSnapshotV1, root: &Path) -> Result<()> {
    snapshot
        .validate()
        .map_err(|error| invalid(error.to_string()))?;
    if Some(snapshot.root.as_str()) != root.to_str() {
        return Err(invalid("source snapshot belongs to another root"));
    }
    Ok(())
}
fn maximum_declaration() -> CandidateDeclarationV1 {
    CandidateDeclarationV1 {
        allowed_paths: Vec::new(),
        entrypoint: String::new(),
        max_files: MAX_FILES,
        max_path_bytes: MAX_PATH_BYTES,
        max_file_bytes: MAX_FILE_BYTES,
        max_total_bytes: MAX_TOTAL_BYTES,
    }
}
fn validate_declaration(value: &CandidateDeclarationV1) -> Result<()> {
    if value.max_files == 0
        || value.max_files > MAX_FILES
        || value.max_path_bytes == 0
        || value.max_path_bytes > MAX_PATH_BYTES
        || value.max_file_bytes == 0
        || value.max_file_bytes > MAX_FILE_BYTES
        || value.max_total_bytes == 0
        || value.max_total_bytes > MAX_TOTAL_BYTES
        || value.max_file_bytes > value.max_total_bytes
        || value.allowed_paths.is_empty()
        || value.allowed_paths.len() as u64 > value.max_files
        || !value.allowed_paths.contains(&value.entrypoint)
    {
        return Err(invalid("candidate bounds or entrypoint are invalid"));
    }
    let mut unique = BTreeSet::new();
    for path in &value.allowed_paths {
        validate_path(path, value.max_path_bytes)?;
        if !unique.insert(path) {
            return Err(invalid("duplicate candidate path"));
        }
    }
    Ok(())
}
fn validate_path(path: &str, ceiling: u64) -> Result<()> {
    validate_relative_path(path, ceiling).map_err(|error| invalid(error.to_string()))?;
    let first = path.split('/').next();
    if matches!(first, Some(".a3s" | ".a3s-code" | ".git"))
        || path == "docs/evidence"
        || path.starts_with("docs/evidence/")
    {
        return Err(invalid("reserved harness or evidence path"));
    }
    Ok(())
}
fn validate_entries(files: &[FrozenFileV1], limits: &CandidateDeclarationV1) -> Result<u64> {
    if files.is_empty() || files.len() as u64 > limits.max_files {
        return Err(invalid("empty or excessive frozen files"));
    }
    let mut previous: Option<&str> = None;
    let mut total = 0_u64;
    for file in files {
        validate_path(&file.path, limits.max_path_bytes)?;
        if previous.is_some_and(|path| path >= file.path.as_str())
            || file.byte_count > limits.max_file_bytes
        {
            return Err(invalid("noncanonical entries or excessive file bytes"));
        }
        total = total
            .checked_add(file.byte_count)
            .filter(|n| *n <= limits.max_total_bytes)
            .ok_or_else(|| invalid("frozen total byte limit exceeded"))?;
        previous = Some(&file.path);
    }
    Ok(total)
}
fn candidate_digest(files: &[FrozenFileV1]) -> Result<DigestV1> {
    let mut hash = Sha256::new();
    hash.update(BYTES_DOMAIN);
    for file in files {
        hash.update((file.path.len() as u64).to_be_bytes());
        hash.update(file.path.as_bytes());
        hash.update(file.byte_count.to_be_bytes());
        // DigestV1 already enforces canonical lower-case SHA256.
        let hex = file
            .file_digest
            .as_str()
            .strip_prefix("sha256:")
            .ok_or_else(|| invalid("invalid file digest"))?;
        for pair in hex.as_bytes().chunks_exact(2) {
            let text = std::str::from_utf8(pair).map_err(|error| invalid(error.to_string()))?;
            let byte = u8::from_str_radix(text, 16).map_err(|error| invalid(error.to_string()))?;
            hash.update([byte]);
        }
    }
    canonical_digest(format!("sha256:{:x}", hash.finalize()))
}
fn canonical_digest(value: String) -> Result<DigestV1> {
    DigestV1::new(value).map_err(|error| invalid(error.to_string()))
}
fn invalid(detail: impl Into<String>) -> FreezeError {
    FreezeError::new(AcceptanceReasonV1::FreezeInvalid, detail)
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn race(detail: impl Into<String>) -> FreezeError {
    FreezeError::new(AcceptanceReasonV1::FreezeRace, detail)
}

#[cfg(test)]
mod tests;
