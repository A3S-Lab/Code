//! Typed workspace source-snapshot authority (KRN-4).
//!
//! One immutable value binds every revision that derived workspace data
//! depends on: the manifest scan revision, the eligible file content
//! identity, the eligibility policy revision, the document/LSP revision,
//! the chunk catalog revision, and the derived index generation. Search
//! hits, symbol results, model context items, and evidence facts can carry
//! one snapshot digest, and a result bound to an older snapshot can never be
//! presented as current against a newer live snapshot.
//!
//! This is an identity and boundary layer. The manifest remains the scan
//! authority, the catalog remains the chunk authority, and each index
//! remains a rebuildable derived cache; none of them gains a second opinion
//! about revisions.

use crate::workspace::manifest::LocalWorkspaceManifestSnapshot;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const WORKSPACE_SOURCE_SNAPSHOT_SCHEMA_V1: &str = "a3s.code.workspace-source-snapshot.v1";
pub const WORKSPACE_SOURCE_SNAPSHOT_DIGEST_DOMAIN_V1: &str =
    "a3s.code.workspace-source-snapshot.identity.v1";
/// Digest domain binding the eligible file set of one manifest revision.
pub const WORKSPACE_SOURCE_CONTENT_DOMAIN_V1: &str =
    "a3s.code.workspace-source-snapshot.content.v1";
const MAX_ROOT_BYTES: usize = 1024;
const MAX_ELIGIBLE_FILES: u64 = u32::MAX as u64;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WorkspaceSourceSnapshotError {
    #[error("workspace source snapshot schema is unsupported")]
    UnsupportedSchema,
    #[error("workspace source snapshot field `{0}` is invalid")]
    InvalidField(&'static str),
    #[error("workspace source snapshot digest `{0}` is invalid")]
    InvalidDigest(&'static str),
    #[error("workspace source snapshot serialization failed: {0}")]
    Serialization(String),
}

/// The single revision authority for one workspace at one point in time.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceSourceSnapshotV1 {
    pub schema: String,
    /// Normalized workspace root (display form, bounded, no NULs).
    pub root: String,
    /// Manifest scan revision (`LocalWorkspaceManifestSnapshot::version`).
    pub manifest_version: u64,
    /// Eligible file count covered by this snapshot.
    pub eligible_files: u64,
    /// Domain-separated digest binding the eligible file identity set
    /// (normalized path, size, modified time) at this manifest revision.
    pub content_digest: String,
    /// Eligibility policy revision applied while selecting files.
    pub eligibility_revision: u64,
    /// Highest document/LSP revision settled into this snapshot.
    pub document_revision: u64,
    /// Chunk catalog revision the derived indexes were built from.
    pub catalog_revision: u64,
    /// Derived index generation published for this snapshot.
    pub index_generation: u64,
    /// Logical observation time in milliseconds since the epoch.
    pub observed_at_ms: u64,
    /// Canonical digest over every identity field above.
    pub snapshot_digest: String,
}

impl WorkspaceSourceSnapshotV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        root: impl Into<String>,
        manifest_version: u64,
        eligible_files: u64,
        content_digest: impl Into<String>,
        eligibility_revision: u64,
        document_revision: u64,
        catalog_revision: u64,
        index_generation: u64,
        observed_at_ms: u64,
    ) -> Result<Self, WorkspaceSourceSnapshotError> {
        let mut snapshot = Self {
            schema: WORKSPACE_SOURCE_SNAPSHOT_SCHEMA_V1.to_owned(),
            root: root.into(),
            manifest_version,
            eligible_files,
            content_digest: content_digest.into(),
            eligibility_revision,
            document_revision,
            catalog_revision,
            index_generation,
            observed_at_ms,
            snapshot_digest: String::new(),
        };
        snapshot.validate_without_digest()?;
        snapshot.snapshot_digest = snapshot.expected_digest()?;
        Ok(snapshot)
    }

    /// Derive the snapshot identity from a manifest snapshot plus the
    /// derived-data revisions that were admitted against it.
    ///
    /// `content_digest` must be computed with
    /// [`workspace_content_digest`] over the same
    /// eligible file set the manifest revision describes, so two callers that
    /// observe the same files derive the same identity.
    pub fn from_manifest(
        manifest: &LocalWorkspaceManifestSnapshot,
        content_digest: impl Into<String>,
        eligibility_revision: u64,
        document_revision: u64,
        catalog_revision: u64,
        index_generation: u64,
    ) -> Result<Self, WorkspaceSourceSnapshotError> {
        Self::new(
            manifest.root.display().to_string(),
            manifest.version,
            u64::try_from(manifest.files.len())
                .map_err(|_| WorkspaceSourceSnapshotError::InvalidField("eligible_files"))?,
            content_digest,
            eligibility_revision,
            document_revision,
            catalog_revision,
            index_generation,
            manifest.scanned_at_ms,
        )
    }

    pub fn validate(&self) -> Result<(), WorkspaceSourceSnapshotError> {
        self.validate_without_digest()?;
        validate_digest("snapshot_digest", &self.snapshot_digest)?;
        if self.snapshot_digest != self.expected_digest()? {
            return Err(WorkspaceSourceSnapshotError::InvalidDigest(
                "snapshot_digest",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, WorkspaceSourceSnapshotError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            root: &'a str,
            manifest_version: u64,
            eligible_files: u64,
            content_digest: &'a str,
            eligibility_revision: u64,
            document_revision: u64,
            catalog_revision: u64,
            index_generation: u64,
            observed_at_ms: u64,
        }
        let bytes = serde_json::to_vec(&Identity {
            schema: &self.schema,
            root: &self.root,
            manifest_version: self.manifest_version,
            eligible_files: self.eligible_files,
            content_digest: &self.content_digest,
            eligibility_revision: self.eligibility_revision,
            document_revision: self.document_revision,
            catalog_revision: self.catalog_revision,
            index_generation: self.index_generation,
            observed_at_ms: self.observed_at_ms,
        })
        .map_err(|error| WorkspaceSourceSnapshotError::Serialization(error.to_string()))?;
        Ok(digest_bytes(
            WORKSPACE_SOURCE_SNAPSHOT_DIGEST_DOMAIN_V1,
            &bytes,
        ))
    }

    fn validate_without_digest(&self) -> Result<(), WorkspaceSourceSnapshotError> {
        if self.schema != WORKSPACE_SOURCE_SNAPSHOT_SCHEMA_V1 {
            return Err(WorkspaceSourceSnapshotError::UnsupportedSchema);
        }
        if self.root.is_empty()
            || self.root.len() > MAX_ROOT_BYTES
            || self.root.contains('\0')
            || self.root.lines().count() != 1
        {
            return Err(WorkspaceSourceSnapshotError::InvalidField("root"));
        }
        if self.eligible_files > MAX_ELIGIBLE_FILES {
            return Err(WorkspaceSourceSnapshotError::InvalidField("eligible_files"));
        }
        validate_digest("content_digest", &self.content_digest)?;
        if self.observed_at_ms == 0 {
            return Err(WorkspaceSourceSnapshotError::InvalidField("observed_at_ms"));
        }
        Ok(())
    }

    /// Whether every identity revision of `self` is at least `other`'s.
    ///
    /// Derived work may only move forward: a newer snapshot never re-binds to
    /// older manifest, document, catalog, or index revisions.
    pub fn revision_at_least(&self, other: &Self) -> bool {
        self.manifest_version >= other.manifest_version
            && self.eligibility_revision >= other.eligibility_revision
            && self.document_revision >= other.document_revision
            && self.catalog_revision >= other.catalog_revision
            && self.index_generation >= other.index_generation
    }

    /// Whether a result produced at `self` may still be presented as current
    /// against `live`.
    ///
    /// A manifest change (new scan revision or different eligible content)
    /// always invalidates: stale results cannot cross a source snapshot
    /// boundary. Derived-revision advances alone (a rebuilt index or a newer
    /// settled document) do not invalidate older results bound to the same
    /// manifest content, because the source they were derived from is
    /// unchanged.
    pub fn is_current_against(&self, live: &Self) -> bool {
        self.root == live.root
            && self.manifest_version == live.manifest_version
            && self.content_digest == live.content_digest
            && self.eligibility_revision == live.eligibility_revision
            && self.catalog_revision <= live.catalog_revision
    }

    /// Whether a result produced at `self` is stale against `live`.
    pub fn is_stale_against(&self, live: &Self) -> bool {
        !self.is_current_against(live)
    }
}

impl<'de> Deserialize<'de> for WorkspaceSourceSnapshotV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Wire {
            schema: String,
            root: String,
            manifest_version: u64,
            eligible_files: u64,
            content_digest: String,
            eligibility_revision: u64,
            document_revision: u64,
            catalog_revision: u64,
            index_generation: u64,
            observed_at_ms: u64,
            snapshot_digest: String,
        }

        let wire = Wire::deserialize(deserializer)?;
        let value = Self {
            schema: wire.schema,
            root: wire.root,
            manifest_version: wire.manifest_version,
            eligible_files: wire.eligible_files,
            content_digest: wire.content_digest,
            eligibility_revision: wire.eligibility_revision,
            document_revision: wire.document_revision,
            catalog_revision: wire.catalog_revision,
            index_generation: wire.index_generation,
            observed_at_ms: wire.observed_at_ms,
            snapshot_digest: wire.snapshot_digest,
        };
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}

/// Compute the content identity of one manifest snapshot's eligible files.
///
/// The digest binds normalized path, size, and modified time in sorted path
/// order, so any observed change to the eligible set produces a different
/// identity. Two snapshots of the same content derive the same digest
/// regardless of scan order.
pub fn workspace_content_digest(manifest: &LocalWorkspaceManifestSnapshot) -> String {
    let mut entries: Vec<(&str, u64, Option<u64>)> = manifest
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.size, file.modified_ms))
        .collect();
    entries.sort_unstable_by(|left, right| left.0.cmp(right.0));
    let mut bytes = Vec::new();
    for (path, size, modified_ms) in entries {
        bytes.extend_from_slice(path.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&size.to_le_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&modified_ms.unwrap_or(0).to_le_bytes());
        bytes.push(0);
    }
    digest_bytes(WORKSPACE_SOURCE_CONTENT_DOMAIN_V1, &bytes)
}

fn validate_digest(field: &'static str, value: &str) -> Result<(), WorkspaceSourceSnapshotError> {
    if value.len() != 71
        || !value.starts_with("sha256:")
        || !value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(WorkspaceSourceSnapshotError::InvalidDigest(field));
    }
    Ok(())
}

fn digest_bytes(domain: &str, bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::manifest::{LocalWorkspaceFile, LocalWorkspaceFileStatus};

    fn manifest(version: u64, files: Vec<LocalWorkspaceFile>) -> LocalWorkspaceManifestSnapshot {
        LocalWorkspaceManifestSnapshot {
            version,
            root: std::path::PathBuf::from("/tmp/ws"),
            files,
            scanned_at_ms: 1_000 + version,
        }
    }

    fn file(path: &str, size: u64) -> LocalWorkspaceFile {
        LocalWorkspaceFile {
            path: path.to_owned(),
            size,
            modified_ms: Some(42),
            language: Some("rust".to_owned()),
            status: LocalWorkspaceFileStatus::Tracked,
            binary: false,
            generated: false,
        }
    }

    fn snapshot(manifest_version: u64) -> WorkspaceSourceSnapshotV1 {
        let m = manifest(manifest_version, vec![file("src/main.rs", 10)]);
        WorkspaceSourceSnapshotV1::from_manifest(&m, workspace_content_digest(&m), 1, 2, 3, 4)
            .unwrap()
    }

    #[test]
    fn identity_is_deterministic_and_order_independent() {
        let a = manifest(7, vec![file("a.rs", 1), file("b.rs", 2)]);
        let b = manifest(7, vec![file("b.rs", 2), file("a.rs", 1)]);
        assert_eq!(workspace_content_digest(&a), workspace_content_digest(&b));

        let first =
            WorkspaceSourceSnapshotV1::from_manifest(&a, workspace_content_digest(&a), 1, 2, 3, 4)
                .unwrap();
        assert!(first.validate().is_ok());
        let encoded = serde_json::to_string(&first).unwrap();
        let decoded: WorkspaceSourceSnapshotV1 = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, first);

        let mut unknown = serde_json::to_value(&first).unwrap();
        unknown
            .as_object_mut()
            .unwrap()
            .insert("future".to_owned(), serde_json::json!(true));
        assert!(serde_json::from_value::<WorkspaceSourceSnapshotV1>(unknown).is_err());
    }

    #[test]
    fn tampering_fails_closed() {
        let mut forged = snapshot(7);
        forged.catalog_revision = 99;
        assert!(forged.validate().is_err());
    }

    #[test]
    fn stale_results_cannot_cross_a_source_boundary() {
        let base = snapshot(7);
        // Same manifest content with a rebuilt index stays current.
        let rebuilt = WorkspaceSourceSnapshotV1::from_manifest(
            &manifest(7, vec![file("src/main.rs", 10)]),
            workspace_content_digest(&manifest(7, vec![file("src/main.rs", 10)])),
            1,
            2,
            3,
            9,
        )
        .unwrap();
        assert!(base.is_current_against(&rebuilt));
        assert!(!base.is_stale_against(&rebuilt));

        // A new manifest revision invalidates prior results.
        let advanced = snapshot(8);
        assert!(base.is_stale_against(&advanced));
        assert!(!base.is_current_against(&advanced));

        // Changed eligible content at the same revision invalidates.
        let changed = {
            let m = manifest(7, vec![file("src/main.rs", 11)]);
            WorkspaceSourceSnapshotV1::from_manifest(&m, workspace_content_digest(&m), 1, 2, 3, 4)
                .unwrap()
        };
        assert!(base.is_stale_against(&changed));

        // Revisions only move forward.
        assert!(rebuilt.revision_at_least(&base));
        assert!(!base.revision_at_least(&advanced));
    }

    #[test]
    fn roots_cannot_be_confused() {
        let other_root = {
            let m = manifest(7, vec![file("src/main.rs", 10)]);
            let mut s = WorkspaceSourceSnapshotV1::from_manifest(
                &m,
                workspace_content_digest(&m),
                1,
                2,
                3,
                4,
            )
            .unwrap();
            s.root = "/tmp/other".to_owned();
            s.snapshot_digest = s.expected_digest().unwrap();
            s
        };
        assert!(snapshot(7).is_stale_against(&other_root));
    }
}

#[cfg(test)]
mod qualification {
    use super::*;
    use crate::workspace::manifest::{LocalWorkspaceFile, LocalWorkspaceFileStatus};
    use crate::workspace::retrieval::{
        ChunkCatalogLimits, ChunkingConfig, WorkspaceChunkCatalog, WorkspaceIndexError,
    };
    use crate::workspace::WorkspacePath;

    fn catalog() -> std::sync::Arc<WorkspaceChunkCatalog> {
        WorkspaceChunkCatalog::new(
            ChunkingConfig::default(),
            ChunkCatalogLimits {
                max_files: 64,
                max_chunks: 512,
                max_text_bytes: 1024 * 1024,
                max_index_bytes: 8 * 1024 * 1024,
            },
        )
        .unwrap()
    }

    fn manifest_for(
        catalog_snapshot: &crate::workspace::retrieval::ChunkCatalogSnapshot,
    ) -> LocalWorkspaceManifestSnapshot {
        let files = catalog_snapshot
            .paths()
            .into_iter()
            .map(|path| LocalWorkspaceFile {
                path,
                size: 32,
                modified_ms: Some(7),
                language: Some("rust".to_owned()),
                status: LocalWorkspaceFileStatus::Tracked,
                binary: false,
                generated: false,
            })
            .collect();
        LocalWorkspaceManifestSnapshot {
            version: catalog_snapshot.source_revision(),
            root: std::path::PathBuf::from("/tmp/ws"),
            files,
            scanned_at_ms: 10_000,
        }
    }

    /// KRN-4 exit-gate slice: a search hit's chunk identity, the catalog
    /// revision it was served from, and the source snapshot that admitted it
    /// are one traceable chain, and a concurrent edit invalidates prior
    /// results instead of yielding a false current answer.
    #[test]
    fn retrieval_results_trace_to_one_snapshot_and_edits_invalidate() {
        let catalog = catalog();
        let path = WorkspacePath::from_normalized("src/main.rs");
        catalog
            .replace_file(&path, Some("rust"), 1, "fn main() {}\n")
            .unwrap();
        let admitted = catalog.snapshot().unwrap();
        let manifest = manifest_for(&admitted);
        let snapshot = WorkspaceSourceSnapshotV1::from_manifest(
            &manifest,
            workspace_content_digest(&manifest),
            1,
            0,
            admitted.revision(),
            1,
        )
        .unwrap();

        // The served chunk carries the catalog's content digest; the
        // snapshot binds the same catalog revision the hit was served from.
        let chunk_digest = admitted
            .content_digest(&path)
            .expect("admitted file carries a content digest");
        assert!(chunk_digest.starts_with("sha256:"));
        assert_eq!(admitted.revision(), snapshot.catalog_revision);
        assert_eq!(admitted.source_revision(), snapshot.manifest_version);
        assert_eq!(
            admitted.eligible_file_count() as u64,
            snapshot.eligible_files
        );

        // A rebuilt index generation against the same source stays current.
        let rebuilt = WorkspaceSourceSnapshotV1::from_manifest(
            &manifest,
            workspace_content_digest(&manifest),
            1,
            0,
            admitted.revision(),
            2,
        )
        .unwrap();
        assert!(snapshot.is_current_against(&rebuilt));

        // A concurrent edit advances the source revision; results bound to
        // the old snapshot are stale and cannot be presented as current.
        catalog
            .replace_file(&path, Some("rust"), 2, "fn main() { changed }\n")
            .unwrap();
        let edited = catalog.snapshot().unwrap();
        assert!(edited.source_revision() > admitted.source_revision());
        let edited_manifest = manifest_for(&edited);
        let live = WorkspaceSourceSnapshotV1::from_manifest(
            &edited_manifest,
            workspace_content_digest(&edited_manifest),
            1,
            0,
            edited.revision(),
            1,
        )
        .unwrap();
        assert!(snapshot.is_stale_against(&live));
        assert!(live.revision_at_least(&snapshot));

        let _ = std::any::type_name::<WorkspaceIndexError>();
    }
}
