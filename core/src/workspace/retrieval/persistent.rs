//! Workspace-owned persistent lexical index.
//!
//! The session catalog remains the source of truth for admission and source
//! verification. This module adds an optional durable a3s-vec generation that can
//! be reopened after a process restart. The generation is replaced atomically
//! after a catalog snapshot has been fully built.

use super::catalog::ChunkCatalogSnapshot;
#[cfg(feature = "a3s-vec-fts")]
use super::chunk::{chunk_id, digest_content};
use super::lexical::LexicalSearchRequest;
#[cfg(feature = "a3s-vec-fts")]
use super::lexical::{path_matches, query_terms};
#[cfg(feature = "a3s-vec-fts")]
use super::types::WorkspaceChunk;
use super::types::{WorkspaceIndexError, WorkspaceIndexResult, WorkspaceLexicalEngine};
use serde::{Deserialize, Serialize};
#[cfg(feature = "a3s-vec-fts")]
use std::collections::HashSet;
#[cfg(feature = "a3s-vec-fts")]
use std::fs;
#[cfg(feature = "a3s-vec-fts")]
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(feature = "a3s-vec-fts")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
#[cfg(feature = "a3s-vec-fts")]
use std::sync::{Condvar, Mutex, RwLock};

#[cfg(feature = "a3s-vec-fts")]
/// v2 manifests carry no engine primary keys (dense ordinals are derived);
/// v3 persists the key per chunk so incrementally built generations with
/// sparse ordinals reopen with the exact mapping.
const MANIFEST_SCHEMA_VERSION: u32 = 3;
const LEGACY_MANIFEST_SCHEMA_VERSION: u32 = 2;
#[cfg(feature = "a3s-vec-fts")]
const CURRENT_FILE: &str = "CURRENT";
#[cfg(feature = "a3s-vec-fts")]
const MANIFEST_FILE: &str = "manifest.json";
#[cfg(feature = "a3s-vec-fts")]
const STORAGE_LOCK_FILE: &str = ".index.lock";

#[cfg(feature = "a3s-vec-fts")]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedChunk {
    id: String,
    /// Engine primary key of this chunk's document. `None` on v2 manifests
    /// (dense ordinals are derived on load) and for token-empty chunks that
    /// have no engine document.
    #[serde(default)]
    pk: Option<String>,
    path: String,
    language: Option<String>,
    start_line: usize,
    end_line: usize,
    start_byte: usize,
    end_byte: usize,
    content_digest: String,
    /// Digest of the exact chunk text retained in the durable manifest.
    /// `content_digest` identifies the complete source file and therefore
    /// cannot by itself detect a damaged chunk payload after restart.
    text_digest: String,
    source_revision: u64,
    text: String,
}

#[cfg(feature = "a3s-vec-fts")]
impl PersistedChunk {
    fn from_chunk(chunk: &WorkspaceChunk) -> Self {
        Self {
            id: chunk.id.as_str().to_owned(),
            pk: None,
            path: chunk.path.to_string(),
            language: chunk.language.as_deref().map(str::to_owned),
            start_line: chunk.start_line,
            end_line: chunk.end_line,
            start_byte: chunk.start_byte,
            end_byte: chunk.end_byte,
            content_digest: chunk.content_digest.to_string(),
            text_digest: digest_content(chunk.text.as_ref()).to_string(),
            source_revision: chunk.source_revision,
            text: chunk.text.to_string(),
        }
    }

    fn into_chunk(self) -> WorkspaceChunk {
        WorkspaceChunk {
            id: super::types::WorkspaceChunkId::new(self.id),
            path: Arc::from(self.path),
            language: self.language.map(Arc::from),
            start_line: self.start_line,
            end_line: self.end_line,
            start_byte: self.start_byte,
            end_byte: self.end_byte,
            content_digest: Arc::from(self.content_digest),
            source_revision: self.source_revision,
            text: Arc::from(self.text),
        }
    }
}

#[cfg(feature = "a3s-vec-fts")]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GenerationManifest {
    schema_version: u32,
    lexical_engine: String,
    catalog_revision: u64,
    source_revision: u64,
    chunks: Vec<PersistedChunk>,
}

#[cfg(feature = "a3s-vec-fts")]
struct PersistentState {
    generation: String,
    catalog_revision: u64,
    source_revision: u64,
    indexed_chunks: Arc<[Arc<WorkspaceChunk>]>,
    indexed_files: usize,
    /// Engine primary key per indexed position. Dense for full builds and
    /// legacy generations; sparse (with appended ordinals) for delta builds.
    pks: Arc<[String]>,
    index: super::a3s_vec::A3sVecLexicalIndex,
    /// Whether this generation was loaded from disk by another process
    /// epoch. Revision counters are per-epoch: a loaded generation's big
    /// counters must not veto a fresh epoch's smaller ones, or every
    /// post-restart edit would leave the durable projection permanently
    /// stale. Numeric staleness is only authoritative within the epoch that
    /// published the numbers.
    from_disk: bool,
}

#[cfg(feature = "a3s-vec-fts")]
struct PersistentOperationGuard {
    active: Arc<(Mutex<usize>, Condvar)>,
}

#[cfg(feature = "a3s-vec-fts")]
impl Drop for PersistentOperationGuard {
    fn drop(&mut self) {
        let (lock, wake) = &*self.active;
        if let Ok(mut count) = lock.lock() {
            *count = count.saturating_sub(1);
            wake.notify_all();
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspacePersistentIndexPhase {
    Absent,
    Building,
    Ready,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePersistentIndexStatus {
    pub phase: WorkspacePersistentIndexPhase,
    pub generation: Option<String>,
    pub catalog_revision: u64,
    pub source_revision: u64,
    pub indexed_chunks: usize,
}

/// Durable workspace FTS state. A missing `CURRENT` file is a valid empty
/// state and is populated by the catalog coordinator's first reconciliation.
pub struct WorkspacePersistentIndex {
    root: PathBuf,
    engine: WorkspaceLexicalEngine,
    /// Per-instance epoch nonce. Revision counters restart with every
    /// process, so generation directory names built from them alone collide
    /// across epochs (a restart rebuild at revision 1 would target the dead
    /// epoch's `generation-1`, and two live processes at equal revisions
    /// would write the same directory). The nonce keeps names globally
    /// unique; old-epoch directories are collected by the generation GC.
    #[cfg(feature = "a3s-vec-fts")]
    epoch: String,
    #[cfg(feature = "a3s-vec-fts")]
    writer: Mutex<()>,
    #[cfg(feature = "a3s-vec-fts")]
    active_operations: Arc<(Mutex<usize>, Condvar)>,
    #[cfg(feature = "a3s-vec-fts")]
    building: AtomicBool,
    #[cfg(feature = "a3s-vec-fts")]
    state: RwLock<Option<PersistentState>>,
    /// One-shot sync failure owned by this index. A process-global switch is
    /// stolen by whichever durable worker runs first under parallel tests.
    #[cfg(all(test, feature = "a3s-vec-fts"))]
    fail_next_sync: AtomicBool,
}

#[cfg(feature = "a3s-vec-fts")]
struct BuildActivityGuard<'a>(&'a AtomicBool);

#[cfg(feature = "a3s-vec-fts")]
impl Drop for BuildActivityGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl std::fmt::Debug for WorkspacePersistentIndex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspacePersistentIndex")
            .field("root", &self.root)
            .field("engine", &self.engine)
            .finish_non_exhaustive()
    }
}

impl WorkspacePersistentIndex {
    pub fn open(
        root: impl Into<PathBuf>,
        engine: WorkspaceLexicalEngine,
    ) -> WorkspaceIndexResult<Arc<Self>> {
        Self::open_with_epoch(root, engine, None)
    }

    /// Open with a pinned epoch nonce so tests can predict generation
    /// directory names (`generation-<revision>-<epoch>`).
    #[cfg(all(test, feature = "a3s-vec-fts"))]
    pub(crate) fn open_with_pinned_epoch(
        root: impl Into<PathBuf>,
        engine: WorkspaceLexicalEngine,
        epoch: &str,
    ) -> WorkspaceIndexResult<Arc<Self>> {
        Self::open_with_epoch(root, engine, Some(epoch.to_owned()))
    }

    fn open_with_epoch(
        root: impl Into<PathBuf>,
        engine: WorkspaceLexicalEngine,
        pinned_epoch: Option<String>,
    ) -> WorkspaceIndexResult<Arc<Self>> {
        // a3s-vec rejects Windows verbatim (`\\?\`) paths from `canonicalize()`.
        // Workspace roots are commonly canonicalized before the index is
        // configured, so strip the prefix at the durable-index boundary.
        let root = strip_windows_verbatim_prefix(root.into());
        #[cfg(not(feature = "a3s-vec-fts"))]
        {
            let _ = (&root, engine);
            Err(WorkspaceIndexError::InvalidConfig(
                "persistent workspace indexing requires the a3s-vec-fts feature".to_owned(),
            ))
        }

        #[cfg(feature = "a3s-vec-fts")]
        {
            if engine != WorkspaceLexicalEngine::A3sVec {
                return Err(WorkspaceIndexError::InvalidConfig(
                    "persistent workspace indexing currently requires the a3s-vec lexical engine"
                        .to_owned(),
                ));
            }
            fs::create_dir_all(&root).map_err(|error| WorkspaceIndexError::ReadFailed {
                path: root.display().to_string(),
                message: error.to_string(),
            })?;
            let index = Arc::new(Self {
                root,
                engine,
                epoch: pinned_epoch
                    .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string()[..8].to_owned()),
                writer: Mutex::new(()),
                active_operations: Arc::new((Mutex::new(0), Condvar::new())),
                building: AtomicBool::new(false),
                state: RwLock::new(None),
                #[cfg(test)]
                fail_next_sync: AtomicBool::new(false),
            });
            // CURRENT publication and generation collection are shared by
            // every session that points at this workspace. Serialize those
            // operations across processes before inspecting the directory.
            let storage_guard = index.acquire_storage_lock()?;
            if let Err(error) = index.load_current() {
                tracing::warn!(%error, path = %index.root.display(), "persistent workspace index will be rebuilt");
            } else if let Some(generation) = index.status().generation {
                if let Err(error) = gc_generations(&index.root, &generation) {
                    tracing::warn!(%error, path = %index.root.display(), "persistent workspace generation cleanup failed");
                }
            }
            drop(storage_guard);
            Ok(index)
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn engine(&self) -> WorkspaceLexicalEngine {
        self.engine
    }

    /// Fail the next `sync_snapshot` once with a non-retryable query error.
    #[cfg(all(test, feature = "a3s-vec-fts"))]
    pub(crate) fn fail_next_sync_for_test(&self) {
        self.fail_next_sync.store(true, Ordering::SeqCst);
    }

    /// Whether [`Self::fail_next_sync_for_test`] is still waiting to fire.
    #[cfg(all(test, feature = "a3s-vec-fts"))]
    pub(crate) fn non_retryable_sync_failure_is_armed(&self) -> bool {
        self.fail_next_sync.load(Ordering::SeqCst)
    }

    pub fn is_ready(&self) -> bool {
        #[cfg(feature = "a3s-vec-fts")]
        {
            self.state
                .read()
                .map(|state| state.is_some())
                .unwrap_or(false)
        }
        #[cfg(not(feature = "a3s-vec-fts"))]
        {
            false
        }
    }

    pub fn status(&self) -> WorkspacePersistentIndexStatus {
        #[cfg(feature = "a3s-vec-fts")]
        {
            let building = self.building.load(Ordering::Acquire);
            self.state
                .read()
                .ok()
                .and_then(|state| {
                    state.as_ref().map(|state| WorkspacePersistentIndexStatus {
                        phase: if building {
                            WorkspacePersistentIndexPhase::Building
                        } else {
                            WorkspacePersistentIndexPhase::Ready
                        },
                        generation: Some(state.generation.clone()),
                        catalog_revision: state.catalog_revision,
                        source_revision: state.source_revision,
                        indexed_chunks: state.indexed_chunks.len(),
                    })
                })
                .unwrap_or(WorkspacePersistentIndexStatus {
                    phase: if building {
                        WorkspacePersistentIndexPhase::Building
                    } else {
                        WorkspacePersistentIndexPhase::Absent
                    },
                    generation: None,
                    catalog_revision: 0,
                    source_revision: 0,
                    indexed_chunks: 0,
                })
        }
        #[cfg(not(feature = "a3s-vec-fts"))]
        {
            WorkspacePersistentIndexStatus {
                phase: WorkspacePersistentIndexPhase::Absent,
                generation: None,
                catalog_revision: 0,
                source_revision: 0,
                indexed_chunks: 0,
            }
        }
    }

    pub fn rebuild(&self, snapshot: &ChunkCatalogSnapshot) -> WorkspaceIndexResult<()> {
        self.sync_snapshot(snapshot)
    }

    pub fn drop_index(&self) -> WorkspaceIndexResult<()> {
        #[cfg(not(feature = "a3s-vec-fts"))]
        {
            Err(WorkspaceIndexError::InvalidConfig(
                "persistent workspace indexing requires the a3s-vec-fts feature".to_owned(),
            ))
        }

        #[cfg(feature = "a3s-vec-fts")]
        {
            let _storage_guard = self.acquire_storage_lock()?;
            let _write_guard = self
                .writer
                .lock()
                .map_err(|_| WorkspaceIndexError::LockPoisoned)?;
            self.wait_for_idle();
            let previous = self
                .state
                .write()
                .map_err(|_| WorkspaceIndexError::LockPoisoned)?
                .take();
            drop(previous);
            let current = self.root.join(CURRENT_FILE);
            if let Err(error) = fs::remove_file(&current) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(WorkspaceIndexError::ReadFailed {
                        path: current.display().to_string(),
                        message: error.to_string(),
                    });
                }
            }
            for entry in
                fs::read_dir(&self.root).map_err(|error| WorkspaceIndexError::ReadFailed {
                    path: self.root.display().to_string(),
                    message: error.to_string(),
                })?
            {
                let path = entry
                    .map_err(|error| WorkspaceIndexError::ReadFailed {
                        path: self.root.display().to_string(),
                        message: error.to_string(),
                    })?
                    .path();
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default();
                if name.starts_with("generation-") || name.starts_with(".generation-") {
                    remove_path_if_exists(&path)?;
                }
            }
            Ok(())
        }
    }

    pub fn sync_snapshot(&self, snapshot: &ChunkCatalogSnapshot) -> WorkspaceIndexResult<()> {
        #[cfg(not(feature = "a3s-vec-fts"))]
        {
            let _ = snapshot;
            Err(WorkspaceIndexError::InvalidConfig(
                "persistent workspace indexing requires the a3s-vec-fts feature".to_owned(),
            ))
        }

        #[cfg(feature = "a3s-vec-fts")]
        {
            #[cfg(test)]
            if self.fail_next_sync.swap(false, Ordering::SeqCst) {
                return Err(WorkspaceIndexError::InvalidQuery(
                    "forced non-retryable sync failure".into(),
                ));
            }
            let _operation = self.begin_operation()?;
            self.building.store(true, Ordering::Release);
            let _building = BuildActivityGuard(&self.building);
            // The directory contains one shared CURRENT pointer and shared
            // generation names. The instance-local writer is insufficient
            // when multiple sessions/processes use the same workspace.
            let _storage_guard = self.acquire_storage_lock()?;
            let _write_guard = self
                .writer
                .lock()
                .map_err(|_| WorkspaceIndexError::LockPoisoned)?;
            let current_epoch_state = self
                .state
                .read()
                .map_err(|_| WorkspaceIndexError::LockPoisoned)?;
            let persisted = match self.read_current_manifest() {
                Ok(manifest) => manifest,
                Err(error) => {
                    tracing::warn!(%error, path = %self.root.display(), "ignoring unreadable persistent CURRENT while rebuilding");
                    None
                }
            };
            if let Some((_, manifest)) = persisted {
                let requested = (snapshot.source_revision(), snapshot.revision());
                let on_disk = (manifest.source_revision, manifest.catalog_revision);
                // Only an in-epoch publication may veto smaller numbers: the
                // on-disk generation must be the one this instance published.
                // A loaded (cross-epoch) generation keeps big counters from a
                // dead process; those numbers cannot order a fresh epoch's
                // snapshots, so content comparison below decides instead.
                let published_here = current_epoch_state
                    .as_ref()
                    .is_some_and(|state| !state.from_disk);
                if published_here && on_disk > requested {
                    return Err(WorkspaceIndexError::StaleRevision {
                        requested: snapshot.source_revision(),
                        current: manifest.source_revision,
                    });
                }
            }
            let current_state = current_epoch_state;
            if current_state.as_ref().is_some_and(|state| {
                !state.from_disk && state.source_revision > snapshot.source_revision()
            }) {
                let current = current_state
                    .as_ref()
                    .map(|state| state.source_revision)
                    .unwrap_or_default();
                return Err(WorkspaceIndexError::StaleRevision {
                    requested: snapshot.source_revision(),
                    current,
                });
            }
            // Tuple equality is only a no-op within the publishing epoch.
            // A loaded (cross-epoch) generation can carry the same
            // (source, catalog) numbers as a fresh epoch's first snapshot
            // while the underlying content differs — for example when the
            // restart's manifest scan completes after an early edit. The
            // content comparison below must decide those apart.
            if current_state.as_ref().is_some_and(|state| {
                !state.from_disk
                    && state.catalog_revision == snapshot.revision()
                    && state.source_revision == snapshot.source_revision()
            }) {
                return Ok(());
            }
            let chunks = snapshot.chunks();
            let indexed_chunks: Arc<[Arc<WorkspaceChunk>]> = Arc::from(
                chunks
                    .iter()
                    .filter(|chunk| !super::lexical::tokenize(chunk.text.as_ref()).is_empty())
                    .cloned()
                    .collect::<Vec<_>>(),
            );

            // Manifest versions can advance when the source watcher observes
            // metadata-only changes (or when a file is rewritten with the
            // same bytes). The native postings are content-addressed by chunk
            // identity, so rebuilding the whole collection would waste the
            // dominant indexing cost. Reuse the published generation and
            // advance only the in-memory freshness fence. A restart may read
            // the prior manifest revision briefly, but the shared manifest
            // coordinator reconciles the current snapshot before normal
            // indexed results are considered fresh.
            if let Some(state) = current_state.as_ref() {
                if indexed_chunks_match(&state.indexed_chunks, &indexed_chunks) {
                    let generation = state.generation.clone();
                    drop(current_state);
                    let mut current = self
                        .state
                        .write()
                        .map_err(|_| WorkspaceIndexError::LockPoisoned)?;
                    let Some(current) = current.as_mut() else {
                        return Err(WorkspaceIndexError::InvalidConfig(
                            "persistent workspace index disappeared during reuse".to_owned(),
                        ));
                    };
                    if current.generation != generation {
                        return Err(WorkspaceIndexError::StaleRevision {
                            requested: snapshot.source_revision(),
                            current: current.source_revision,
                        });
                    }
                    current.catalog_revision = snapshot.revision();
                    current.source_revision = snapshot.source_revision();
                    current.indexed_files = distinct_path_count(&indexed_chunks);
                    current.indexed_chunks = indexed_chunks;
                    drop(_operation);
                    if let Err(error) = gc_generations(&self.root, &generation) {
                        tracing::warn!(%error, path = %self.root.display(), "persistent workspace generation cleanup failed");
                    }
                    return Ok(());
                }
            }
            // Capture the previous generation facts before releasing the
            // read guard: the delta arm needs its chunk order and engine keys.
            let previous = current_state.as_ref().map(|state| {
                (
                    state.generation.clone(),
                    Arc::clone(&state.indexed_chunks),
                    Arc::clone(&state.pks),
                )
            });
            drop(current_state);

            let generation = format!("generation-{}-{}", snapshot.revision(), self.epoch);
            let staging = self.root.join(format!(".{generation}.staging"));
            let destination = self.root.join(&generation);
            remove_path_if_exists(&staging)?;
            remove_path_if_exists(&destination)?;
            fs::create_dir_all(&staging).map_err(|error| WorkspaceIndexError::ReadFailed {
                path: staging.display().to_string(),
                message: error.to_string(),
            })?;

            let collection_path = staging.join("collection");
            // Delta arm first: a bounded chunk delta builds the generation by
            // copying the previous collection and applying engine-level
            // delete/upsert, instead of rewriting every posting. Any failure
            // falls back to the full build below (fail closed to today's
            // behavior); the reopen-time digest verification remains the
            // final guard for delta correctness.
            let mut indexed_chunks = indexed_chunks;
            // The incremental arm's outcome: the built index, the engine keys
            // aligned to the reordered chunk list, and that reordered list.
            struct IncrementalGeneration {
                index: super::a3s_vec::A3sVecLexicalIndex,
                pks: Arc<[String]>,
                chunks: Arc<[Arc<WorkspaceChunk>]>,
            }
            let mut incremental: Option<IncrementalGeneration> = None;
            if let Some((previous_generation, previous_chunks, previous_pks)) = previous.as_ref() {
                if let Some(delta) =
                    plan_generation_delta(previous_chunks, previous_pks, &indexed_chunks)
                {
                    let source = self.root.join(previous_generation).join("collection");
                    match super::a3s_vec::A3sVecLexicalIndex::apply_delta_at_path(
                        &collection_path,
                        &source,
                        delta.kept,
                        &delta.removed_pks,
                        delta.appended,
                        previous_chunks.len(),
                    ) {
                        Ok(built) => {
                            tracing::debug!(
                                generation = %generation,
                                removed = delta.removed_pks.len(),
                                appended = delta.appended_count,
                                "persistent a3s-vec generation built incrementally"
                            );
                            incremental = Some(IncrementalGeneration {
                                index: built,
                                pks: delta.new_pks,
                                chunks: Arc::from(
                                    delta.kept_chunks.iter().map(Arc::clone).collect::<Vec<_>>(),
                                ),
                            });
                        }
                        Err(error) => {
                            tracing::warn!(
                                %error,
                                generation = %generation,
                                "incremental a3s-vec generation build failed; rebuilding fully"
                            );
                            remove_path_if_exists(&staging)?;
                            fs::create_dir_all(&staging).map_err(|error| {
                                WorkspaceIndexError::ReadFailed {
                                    path: staging.display().to_string(),
                                    message: error.to_string(),
                                }
                            })?;
                        }
                    }
                }
            }
            let (mut index, pks) = match incremental {
                Some(generated) => {
                    indexed_chunks = generated.chunks;
                    (generated.index, generated.pks)
                }
                None => {
                    let pks = Arc::from(
                        (0..indexed_chunks.len())
                            .map(|ordinal| String::from("d") + &ordinal.to_string())
                            .collect::<Vec<_>>(),
                    );
                    let built = super::a3s_vec::A3sVecLexicalIndex::build_at_path(
                        &collection_path,
                        indexed_chunks
                            .iter()
                            .map(|chunk| {
                                (chunk.id.as_str().to_owned(), chunk.text.as_ref().to_owned())
                            })
                            .collect(),
                    )
                    .map_err(|error| {
                        WorkspaceIndexError::IndexBuildFailed {
                            retryable: error.retryable(),
                            message: error.to_string(),
                        }
                    })?;
                    (built, pks)
                }
            };
            let pk_by_id: std::collections::HashMap<&str, &str> = indexed_chunks
                .iter()
                .zip(pks.iter())
                .map(|(chunk, pk)| (chunk.id.as_str(), pk.as_str()))
                .collect();
            let manifest = GenerationManifest {
                schema_version: MANIFEST_SCHEMA_VERSION,
                lexical_engine: self.engine.stable_id().to_owned(),
                catalog_revision: snapshot.revision(),
                source_revision: snapshot.source_revision(),
                chunks: chunks
                    .iter()
                    .map(|chunk| {
                        let mut persisted = PersistedChunk::from_chunk(chunk);
                        persisted.pk = pk_by_id.get(chunk.id.as_str()).copied().map(str::to_owned);
                        persisted
                    })
                    .collect(),
            };
            write_json_atomic(&staging.join(MANIFEST_FILE), &manifest)?;
            fs::rename(&staging, &destination).map_err(|error| {
                WorkspaceIndexError::ReadFailed {
                    path: destination.display().to_string(),
                    message: error.to_string(),
                }
            })?;
            index.relocate_collection_path(destination.join("collection"));
            write_current(&self.root, &generation)?;
            let indexed_files = distinct_path_count(&indexed_chunks);

            let next = PersistentState {
                generation: generation.clone(),
                catalog_revision: manifest.catalog_revision,
                source_revision: manifest.source_revision,
                indexed_chunks,
                indexed_files,
                pks,
                index,
                from_disk: false,
            };
            *self
                .state
                .write()
                .map_err(|_| WorkspaceIndexError::LockPoisoned)? = Some(next);
            drop(_operation);
            if let Err(error) = gc_generations(&self.root, &generation) {
                tracing::warn!(%error, path = %self.root.display(), "persistent workspace generation cleanup failed");
            }
            Ok(())
        }
    }

    pub fn search(
        &self,
        request: &LexicalSearchRequest,
    ) -> WorkspaceIndexResult<super::lexical::LexicalSearchResult> {
        #[cfg(not(feature = "a3s-vec-fts"))]
        {
            let _ = request;
            Err(WorkspaceIndexError::InvalidConfig(
                "persistent workspace indexing requires the a3s-vec-fts feature".to_owned(),
            ))
        }

        #[cfg(feature = "a3s-vec-fts")]
        {
            let _operation = self.begin_operation()?;
            super::lexical::validate_request(request)?;
            // Keep the shared generation alive while the native query is
            // executing. Writers take the exclusive counterpart before
            // replacing/collecting generations, including from another
            // process that points at the same workspace.
            let _storage_guard = self.acquire_storage_read_lock()?;
            let terms = query_terms(request.query.trim(), 32);
            if terms.is_empty() {
                return Err(WorkspaceIndexError::InvalidQuery(
                    "query must contain a letter, number, underscore, or CJK character".to_owned(),
                ));
            }
            let glob = request
                .glob
                .as_deref()
                .map(glob::Pattern::new)
                .transpose()
                .map_err(|error| WorkspaceIndexError::InvalidQuery(error.to_string()))?;
            let state_guard = self
                .state
                .read()
                .map_err(|_| WorkspaceIndexError::LockPoisoned)?;
            let Some(state) = state_guard.as_ref() else {
                return Ok(super::lexical::LexicalSearchResult {
                    catalog_revision: 0,
                    source_revision: 0,
                    lexical_engine: self.engine,
                    query_terms: terms,
                    matching_files: 0,
                    selected_files: 0,
                    scored_chunks: 0,
                    candidate_truncated: false,
                    hits: Vec::new(),
                });
            };

            let has_path_filter = !request.path.is_root() || glob.is_some();
            let matching_files = if has_path_filter {
                state
                    .indexed_chunks
                    .iter()
                    .filter(|chunk| path_matches(chunk.path.as_ref(), &request.path, glob.as_ref()))
                    .map(|chunk| chunk.path.clone())
                    .collect::<HashSet<_>>()
                    .len()
            } else {
                state.indexed_files
            };
            let query_limit = request
                .limit
                .saturating_mul(request.max_candidate_files.max(1))
                .min(state.index.document_count().max(request.limit));
            let ranked = state.index.search(&terms, query_limit).map_err(|error| {
                WorkspaceIndexError::InvalidQuery(format!("a3s-vec FTS search failed: {error}"))
            })?;
            let mut selected_files = HashSet::new();
            let mut hits = Vec::new();
            let mut per_file = std::collections::HashMap::<Arc<str>, usize>::new();
            for (ordinal, score) in ranked {
                let Some(chunk) = state.indexed_chunks.get(ordinal).cloned() else {
                    continue;
                };
                if !path_matches(chunk.path.as_ref(), &request.path, glob.as_ref()) {
                    continue;
                }
                if !selected_files.contains(&chunk.path)
                    && selected_files.len() >= request.max_candidate_files
                {
                    continue;
                }
                selected_files.insert(chunk.path.clone());
                let count = per_file.entry(Arc::clone(&chunk.path)).or_default();
                if *count >= request.max_results_per_file {
                    continue;
                }
                *count += 1;
                hits.push(super::lexical::LexicalSearchHit { chunk, score });
                if hits.len() >= request.limit {
                    break;
                }
            }
            let candidate_truncated = matching_files > request.max_candidate_files;
            Ok(super::lexical::LexicalSearchResult {
                catalog_revision: state.catalog_revision,
                source_revision: state.source_revision,
                lexical_engine: self.engine,
                query_terms: terms,
                matching_files,
                selected_files: selected_files.len(),
                scored_chunks: state.indexed_chunks.len(),
                candidate_truncated,
                hits,
            })
        }
    }

    /// Wait until all index work owned by this workspace has finished.
    ///
    /// Runtime task cancellation cannot interrupt `spawn_blocking`, so backend
    /// teardown uses this boundary before allowing a temporary workspace or
    /// caller-owned root to disappear underneath an open collection.
    pub(crate) fn wait_for_idle(&self) {
        #[cfg(feature = "a3s-vec-fts")]
        {
            let (lock, wake) = &*self.active_operations;
            let Ok(mut count) = lock.lock() else {
                return;
            };
            while *count != 0 {
                match wake.wait(count) {
                    Ok(next) => count = next,
                    Err(_) => return,
                }
            }
        }
    }

    #[cfg(feature = "a3s-vec-fts")]
    fn begin_operation(&self) -> WorkspaceIndexResult<PersistentOperationGuard> {
        let (lock, _) = &*self.active_operations;
        let mut count = lock.lock().map_err(|_| WorkspaceIndexError::LockPoisoned)?;
        *count = count.checked_add(1).ok_or_else(|| {
            WorkspaceIndexError::InvalidConfig("persistent operation count overflow".to_owned())
        })?;
        Ok(PersistentOperationGuard {
            active: Arc::clone(&self.active_operations),
        })
    }

    #[cfg(feature = "a3s-vec-fts")]
    fn acquire_storage_lock(&self) -> WorkspaceIndexResult<std::fs::File> {
        use fs2::FileExt;

        let file = self.open_storage_lock_file()?;
        let path = self.root.join(STORAGE_LOCK_FILE);
        file.lock_exclusive()
            .map_err(|error| WorkspaceIndexError::ReadFailed {
                path: path.display().to_string(),
                message: format!("failed to lock persistent index: {error}"),
            })?;
        Ok(file)
    }

    #[cfg(feature = "a3s-vec-fts")]
    fn acquire_storage_read_lock(&self) -> WorkspaceIndexResult<std::fs::File> {
        let file = self.open_storage_lock_file()?;
        let path = self.root.join(STORAGE_LOCK_FILE);
        fs2::FileExt::lock_shared(&file).map_err(|error| WorkspaceIndexError::ReadFailed {
            path: path.display().to_string(),
            message: format!("failed to share-lock persistent index: {error}"),
        })?;
        Ok(file)
    }

    #[cfg(feature = "a3s-vec-fts")]
    fn open_storage_lock_file(&self) -> WorkspaceIndexResult<std::fs::File> {
        let path = self.root.join(STORAGE_LOCK_FILE);
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| WorkspaceIndexError::ReadFailed {
                path: path.display().to_string(),
                message: format!("failed to open persistent index lock: {error}"),
            })?;
        Ok(file)
    }

    #[cfg(feature = "a3s-vec-fts")]
    fn load_current(&self) -> WorkspaceIndexResult<()> {
        let Some((generation, manifest)) = self.read_current_manifest()? else {
            return Ok(());
        };
        let generation_root = self.root.join(&generation);
        let manifest_pks: std::collections::HashMap<String, String> = manifest
            .chunks
            .iter()
            .filter_map(|chunk| chunk.pk.as_ref().map(|pk| (chunk.id.clone(), pk.clone())))
            .collect();
        let chunks: Arc<[Arc<WorkspaceChunk>]> = Arc::from(
            manifest
                .chunks
                .into_iter()
                .map(PersistedChunk::into_chunk)
                .map(Arc::new)
                .collect::<Vec<_>>(),
        );
        let indexed_chunks: Arc<[Arc<WorkspaceChunk>]> = Arc::from(
            chunks
                .iter()
                .filter(|chunk| !super::lexical::tokenize(chunk.text.as_ref()).is_empty())
                .cloned()
                .collect::<Vec<_>>(),
        );
        let index = super::a3s_vec::A3sVecLexicalIndex::open_persistent(
            generation_root.join("collection"),
            indexed_chunks
                .iter()
                .map(|chunk| (chunk.id.as_str().to_owned(), chunk.text.as_ref().to_owned()))
                .collect(),
        )
        .map_err(|error| {
            WorkspaceIndexError::InvalidConfig(format!(
                "persistent a3s-vec index failed to open: {error}"
            ))
        })?;
        let indexed_files = distinct_path_count(&indexed_chunks);
        let legacy = manifest.schema_version == LEGACY_MANIFEST_SCHEMA_VERSION;
        let mut pks = Vec::with_capacity(indexed_chunks.len());
        let mut next_dense = 0usize;
        for chunk in indexed_chunks.iter() {
            match if legacy {
                None
            } else {
                manifest_pks.get(chunk.id.as_str())
            } {
                Some(pk) => pks.push(pk.clone()),
                None => {
                    pks.push(String::from("d") + &next_dense.to_string());
                    next_dense += 1;
                }
            }
        }
        *self
            .state
            .write()
            .map_err(|_| WorkspaceIndexError::LockPoisoned)? = Some(PersistentState {
            generation,
            catalog_revision: manifest.catalog_revision,
            source_revision: manifest.source_revision,
            indexed_chunks,
            indexed_files,
            pks: Arc::from(pks),
            index,
            from_disk: true,
        });
        Ok(())
    }

    #[cfg(feature = "a3s-vec-fts")]
    fn read_current_manifest(&self) -> WorkspaceIndexResult<Option<(String, GenerationManifest)>> {
        let current = self.root.join(CURRENT_FILE);
        let generation = match fs::read_to_string(&current) {
            Ok(value) => value.trim().to_owned(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(WorkspaceIndexError::ReadFailed {
                    path: current.display().to_string(),
                    message: error.to_string(),
                })
            }
        };
        if generation.is_empty() || !safe_generation_name(&generation) {
            return Err(WorkspaceIndexError::InvalidConfig(
                "persistent a3s-vec CURRENT contains an invalid generation name".to_owned(),
            ));
        }
        let generation_root = self.root.join(&generation);
        let manifest: GenerationManifest = read_json(&generation_root.join(MANIFEST_FILE))?;
        if (manifest.schema_version != MANIFEST_SCHEMA_VERSION
            && manifest.schema_version != LEGACY_MANIFEST_SCHEMA_VERSION)
            || manifest.lexical_engine != self.engine.stable_id()
        {
            return Err(WorkspaceIndexError::InvalidConfig(
                "persistent a3s-vec index schema or lexical engine is incompatible".to_owned(),
            ));
        }
        validate_persisted_chunks(&manifest.chunks)?;
        Ok(Some((generation, manifest)))
    }
}

#[cfg(feature = "a3s-vec-fts")]
fn safe_generation_name(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
}

#[cfg(feature = "a3s-vec-fts")]
fn gc_generations(root: &Path, current_generation: &str) -> WorkspaceIndexResult<()> {
    if !safe_generation_name(current_generation) {
        return Err(WorkspaceIndexError::InvalidConfig(
            "persistent a3s-vec cleanup received an invalid current generation".to_owned(),
        ));
    }
    for entry in fs::read_dir(root).map_err(|error| WorkspaceIndexError::ReadFailed {
        path: root.display().to_string(),
        message: error.to_string(),
    })? {
        let path = entry
            .map_err(|error| WorkspaceIndexError::ReadFailed {
                path: root.display().to_string(),
                message: error.to_string(),
            })?
            .path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if name == current_generation {
            continue;
        }
        if name.starts_with("generation-") || name.starts_with(".generation-") {
            remove_path_if_exists(&path)?;
        }
    }
    Ok(())
}

#[cfg(feature = "a3s-vec-fts")]
fn remove_path_if_exists(path: &Path) -> WorkspaceIndexResult<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(WorkspaceIndexError::ReadFailed {
            path: path.display().to_string(),
            message: error.to_string(),
        }),
    }
}

/// Strip Windows extended-length prefixes that `canonicalize()` adds.
///
/// Native a3s-vec rejects `\\?\C:\...` paths as invalid. Persistent index roots
/// often inherit a canonicalized workspace path, so normalize at this boundary.
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        if let Some(stripped) = value.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{stripped}"));
        }
        if let Some(stripped) = value.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
    }
    path
}

#[cfg(feature = "a3s-vec-fts")]
fn write_current(root: &Path, generation: &str) -> WorkspaceIndexResult<()> {
    let temporary = root.join(".CURRENT.tmp");
    fs::write(&temporary, format!("{generation}\n")).map_err(|error| {
        WorkspaceIndexError::ReadFailed {
            path: temporary.display().to_string(),
            message: error.to_string(),
        }
    })?;
    #[cfg(windows)]
    let _ = fs::remove_file(root.join(CURRENT_FILE));
    fs::rename(&temporary, root.join(CURRENT_FILE)).map_err(|error| {
        WorkspaceIndexError::ReadFailed {
            path: root.join(CURRENT_FILE).display().to_string(),
            message: error.to_string(),
        }
    })
}

#[cfg(feature = "a3s-vec-fts")]
fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> WorkspaceIndexResult<()> {
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(value).map_err(|error| {
        WorkspaceIndexError::InvalidConfig(format!(
            "persistent a3s-vec manifest serialization failed: {error}"
        ))
    })?;
    fs::write(&temporary, bytes).map_err(|error| WorkspaceIndexError::ReadFailed {
        path: temporary.display().to_string(),
        message: error.to_string(),
    })?;
    fs::rename(&temporary, path).map_err(|error| WorkspaceIndexError::ReadFailed {
        path: path.display().to_string(),
        message: error.to_string(),
    })
}

#[cfg(feature = "a3s-vec-fts")]
fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> WorkspaceIndexResult<T> {
    const MAX_MANIFEST_BYTES: u64 = 256 * 1024 * 1024;
    let file = fs::File::open(path).map_err(|error| WorkspaceIndexError::ReadFailed {
        path: path.display().to_string(),
        message: error.to_string(),
    })?;
    let declared_len = file
        .metadata()
        .map_err(|error| WorkspaceIndexError::ReadFailed {
            path: path.display().to_string(),
            message: error.to_string(),
        })?
        .len();
    if declared_len > MAX_MANIFEST_BYTES {
        return Err(WorkspaceIndexError::InvalidConfig(format!(
            "persistent a3s-vec manifest exceeds the {MAX_MANIFEST_BYTES} byte limit"
        )));
    }
    let mut reader = file.take(MAX_MANIFEST_BYTES + 1);
    let mut bytes = Vec::with_capacity(
        usize::try_from(declared_len)
            .unwrap_or(usize::MAX)
            .min(1024 * 1024),
    );
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| WorkspaceIndexError::ReadFailed {
            path: path.display().to_string(),
            message: error.to_string(),
        })?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(WorkspaceIndexError::InvalidConfig(format!(
            "persistent a3s-vec manifest exceeds the {MAX_MANIFEST_BYTES} byte limit"
        )));
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        WorkspaceIndexError::InvalidConfig(format!(
            "persistent a3s-vec manifest is invalid: {error}"
        ))
    })
}

#[cfg(feature = "a3s-vec-fts")]
fn validate_persisted_chunks(chunks: &[PersistedChunk]) -> WorkspaceIndexResult<()> {
    let mut ids = HashSet::with_capacity(chunks.len());
    for (index, chunk) in chunks.iter().enumerate() {
        if chunk.path.is_empty() {
            return Err(WorkspaceIndexError::InvalidConfig(format!(
                "persistent a3s-vec manifest chunk {index} has an empty path"
            )));
        }
        if chunk.start_byte >= chunk.end_byte {
            return Err(WorkspaceIndexError::InvalidConfig(format!(
                "persistent a3s-vec manifest chunk {index} has an invalid byte range"
            )));
        }
        if chunk.start_line == 0 || chunk.start_line > chunk.end_line {
            return Err(WorkspaceIndexError::InvalidConfig(format!(
                "persistent a3s-vec manifest chunk {index} has an invalid line range"
            )));
        }
        if !valid_sha256_digest(&chunk.content_digest) || !valid_sha256_digest(&chunk.text_digest) {
            return Err(WorkspaceIndexError::InvalidConfig(format!(
                "persistent a3s-vec manifest chunk {index} has a non-canonical digest"
            )));
        }
        let expected_text_digest = digest_content(&chunk.text);
        if chunk.text.is_empty() || expected_text_digest.as_ref() != chunk.text_digest {
            return Err(WorkspaceIndexError::InvalidConfig(format!(
                "persistent a3s-vec manifest chunk {index} text digest does not match its payload"
            )));
        }
        let expected_id = chunk_id(
            &chunk.path,
            &chunk.content_digest,
            chunk.start_byte,
            chunk.end_byte,
        );
        if chunk.id != expected_id.as_str() {
            return Err(WorkspaceIndexError::InvalidConfig(format!(
                "persistent a3s-vec manifest chunk {index} id does not bind its metadata"
            )));
        }
        if !ids.insert(chunk.id.as_str()) {
            return Err(WorkspaceIndexError::InvalidConfig(format!(
                "persistent a3s-vec manifest contains duplicate chunk id at index {index}"
            )));
        }
    }
    Ok(())
}

#[cfg(feature = "a3s-vec-fts")]
fn valid_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

#[cfg(feature = "a3s-vec-fts")]
/// A bounded chunk delta between two catalog generations.
struct GenerationDelta {
    /// (engine primary key, text) of the chunks carried over, in the new
    /// indexed order (previous order preserved).
    kept: Vec<(String, String)>,
    /// Engine primary keys whose documents must be deleted.
    removed_pks: Vec<String>,
    /// (chunk id, text) of the newly admitted chunks, in snapshot order.
    appended: Vec<(String, String)>,
    appended_count: usize,
    /// The new indexed chunk order (previous order for kept chunks, then
    /// appended chunks); the caller adopts this as the generation's
    /// indexed_chunks so positions stay aligned with `new_pks`.
    kept_chunks: Vec<Arc<WorkspaceChunk>>,
    /// Engine primary keys aligned with the new indexed chunk order.
    new_pks: Arc<[String]>,
}

/// Below this share of the corpus (and always for tiny deltas) a generation
/// is built incrementally; larger churn amortizes worse than a full build.
#[cfg(feature = "a3s-vec-fts")]
const DELTA_MAX_SHARE: usize = 3;
#[cfg(feature = "a3s-vec-fts")]
const DELTA_FLOOR: usize = 64;

#[cfg(feature = "a3s-vec-fts")]
fn plan_generation_delta(
    previous_chunks: &[Arc<WorkspaceChunk>],
    previous_pks: &[String],
    next_chunks: &Arc<[Arc<WorkspaceChunk>]>,
) -> Option<GenerationDelta> {
    if previous_chunks.len() != previous_pks.len() || previous_chunks.is_empty() {
        return None;
    }
    let cap = DELTA_FLOOR.max(previous_chunks.len() / DELTA_MAX_SHARE);
    let next_ids: std::collections::HashSet<&str> =
        next_chunks.iter().map(|chunk| chunk.id.as_str()).collect();
    let mut kept = Vec::new();
    let mut kept_chunks = Vec::with_capacity(next_chunks.len());
    let mut removed_pks = Vec::new();
    for (chunk, pk) in previous_chunks.iter().zip(previous_pks.iter()) {
        if next_ids.contains(chunk.id.as_str()) {
            kept.push((pk.clone(), chunk.text.to_string()));
            kept_chunks.push(Arc::clone(chunk));
        } else {
            removed_pks.push(pk.clone());
        }
    }
    let previous_ids: std::collections::HashSet<&str> = previous_chunks
        .iter()
        .map(|chunk| chunk.id.as_str())
        .collect();
    let mut appended = Vec::new();
    let mut appended_chunks = Vec::new();
    let mut appended_pks = Vec::new();
    for chunk in next_chunks.iter() {
        if !previous_ids.contains(chunk.id.as_str()) {
            appended.push((chunk.id.as_str().to_owned(), chunk.text.to_string()));
            appended_chunks.push(Arc::clone(chunk));
        }
    }
    let churn = removed_pks.len() + appended.len();
    if churn > cap {
        return None;
    }
    for next_ordinal in (previous_chunks.len()..).take(appended.len()) {
        appended_pks.push(String::from("d") + &next_ordinal.to_string());
    }
    let appended_count = appended.len();
    let mut new_pks = kept.iter().map(|(pk, _)| pk.clone()).collect::<Vec<_>>();
    new_pks.extend(appended_pks);
    kept_chunks.extend(appended_chunks);
    Some(GenerationDelta {
        kept,
        removed_pks,
        appended,
        appended_count,
        kept_chunks,
        new_pks: Arc::from(new_pks),
    })
}

#[cfg(feature = "a3s-vec-fts")]
fn indexed_chunks_match(left: &[Arc<WorkspaceChunk>], right: &[Arc<WorkspaceChunk>]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.id.as_str() == right.id.as_str()
                && left.path == right.path
                && left.language == right.language
                && left.start_line == right.start_line
                && left.end_line == right.end_line
                && left.start_byte == right.start_byte
                && left.end_byte == right.end_byte
                && left.content_digest == right.content_digest
        })
}

#[cfg(feature = "a3s-vec-fts")]
fn distinct_path_count(chunks: &[Arc<WorkspaceChunk>]) -> usize {
    chunks
        .iter()
        .map(|chunk| Arc::clone(&chunk.path))
        .collect::<HashSet<_>>()
        .len()
}

#[cfg(all(test, feature = "a3s-vec-fts"))]
mod tests {
    use super::{strip_windows_verbatim_prefix, WorkspacePersistentIndex, MANIFEST_FILE};
    use crate::workspace::retrieval::{
        ChunkCatalogLimits, ChunkingConfig, LexicalSearchRequest, WorkspaceChunkCatalog,
        WorkspaceLexicalEngine,
    };
    use crate::workspace::WorkspacePath;
    use std::path::PathBuf;

    #[test]
    fn strip_windows_verbatim_prefix_removes_extended_length_form() {
        let stripped = strip_windows_verbatim_prefix(PathBuf::from(
            r"\\?\C:\Users\runneradmin\AppData\Local\Temp\.tmpIndex\.a3s-code\index",
        ));
        #[cfg(windows)]
        assert_eq!(
            stripped,
            PathBuf::from(r"C:\Users\runneradmin\AppData\Local\Temp\.tmpIndex\.a3s-code\index")
        );
        #[cfg(not(windows))]
        assert_eq!(
            stripped,
            PathBuf::from(r"\\?\C:\Users\runneradmin\AppData\Local\Temp\.tmpIndex\.a3s-code\index")
        );
    }

    #[cfg(feature = "a3s-vec-fts")]
    #[test]
    fn persistent_index_opens_under_windows_verbatim_prefix() {
        let directory = tempfile::tempdir().expect("temporary index directory");
        let root = directory.path().join(".a3s-code").join("index");
        std::fs::create_dir_all(&root).expect("index root");
        let open_root = {
            #[cfg(windows)]
            {
                let canonical = std::fs::canonicalize(&root).expect("canonicalize index root");
                if canonical.to_string_lossy().starts_with(r"\\?\") {
                    canonical
                } else {
                    PathBuf::from(format!(r"\\?\{}", canonical.display()))
                }
            }
            #[cfg(not(windows))]
            {
                root.clone()
            }
        };
        let catalog = WorkspaceChunkCatalog::new_with_engine(
            ChunkingConfig::default(),
            ChunkCatalogLimits::default(),
            WorkspaceLexicalEngine::A3sVec,
        )
        .expect("catalog");
        catalog
            .replace_file(
                &WorkspacePath::from_normalized("src/lib.rs"),
                Some("rust"),
                1,
                "verbatim path persistent sentinel\n",
            )
            .expect("catalog replacement");
        let index = WorkspacePersistentIndex::open(open_root, WorkspaceLexicalEngine::A3sVec)
            .expect("persistent index under verbatim prefix");
        index
            .sync_snapshot(&catalog.snapshot().expect("snapshot"))
            .expect("generation write under verbatim prefix");
        assert!(index.is_ready());
        assert!(
            !index.root().to_string_lossy().starts_with(r"\\?\"),
            "persistent index root should strip the Windows verbatim prefix"
        );
    }

    #[test]
    fn persistent_generation_survives_reopen_and_replaces_removed_content() {
        let directory = tempfile::tempdir().expect("temporary index directory");
        let catalog = WorkspaceChunkCatalog::new_with_engine(
            ChunkingConfig::default(),
            ChunkCatalogLimits::default(),
            WorkspaceLexicalEngine::A3sVec,
        )
        .expect("catalog");
        catalog
            .replace_file(
                &WorkspacePath::from_normalized("src/lib.rs"),
                Some("rust"),
                1,
                "persistent workspace search sentinel\n",
            )
            .expect("catalog replacement");

        let index =
            WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
                .expect("persistent index");
        index
            .sync_snapshot(&catalog.snapshot().expect("snapshot"))
            .expect("generation write");
        assert!(index.is_ready());

        let request = LexicalSearchRequest::new("workspace sentinel");
        let result = index.search(&request).expect("persistent query");
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].chunk.path.as_ref(), "src/lib.rs");
        drop(index);

        let reopened =
            WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
                .expect("reopen persistent index");
        let result = reopened.search(&request).expect("reopened query");
        assert_eq!(result.hits.len(), 1);
        assert_eq!(reopened.status().indexed_chunks, 1);

        catalog
            .remove_file(&WorkspacePath::from_normalized("src/lib.rs"), 2)
            .expect("catalog removal");
        reopened
            .sync_snapshot(&catalog.snapshot().expect("empty snapshot"))
            .expect("replacement generation");
        assert!(reopened
            .search(&request)
            .expect("empty query")
            .hits
            .is_empty());
        reopened.drop_index().expect("drop persistent index");
        assert_eq!(
            reopened.status().phase,
            super::WorkspacePersistentIndexPhase::Absent
        );
    }

    #[cfg(feature = "a3s-vec-fts")]
    #[test]
    fn cross_epoch_reopen_accepts_fresher_content_below_the_persisted_counters() {
        // Epoch 1 publishes with high per-epoch counters, then exits.
        let directory = tempfile::tempdir().expect("temporary index directory");
        let catalog = WorkspaceChunkCatalog::new_with_engine(
            ChunkingConfig::default(),
            ChunkCatalogLimits::default(),
            WorkspaceLexicalEngine::A3sVec,
        )
        .expect("catalog");
        let path = WorkspacePath::from_normalized("src/lib.rs");
        catalog
            .replace_file(&path, Some("rust"), 6, "epoch one payload sentinel\n")
            .expect("epoch one replacement");
        {
            let index =
                WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
                    .expect("epoch one persistent index");
            index
                .sync_snapshot(&catalog.snapshot().expect("epoch one snapshot"))
                .expect("epoch one generation");
            assert_eq!(index.status().source_revision, 6);
        }

        // Epoch 2 rescans the workspace with fresh counters and observes an
        // edit. The loaded generation's (6, 1) counters must not veto the
        // fresh epoch's (1, 1): content decides, so the durable projection
        // publishes instead of going permanently stale.
        let fresh = WorkspaceChunkCatalog::new_with_engine(
            ChunkingConfig::default(),
            ChunkCatalogLimits::default(),
            WorkspaceLexicalEngine::A3sVec,
        )
        .expect("epoch two catalog");
        let index =
            WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
                .expect("epoch two persistent index");
        assert_eq!(index.status().source_revision, 6, "generation must load");
        fresh
            .replace_file(&path, Some("rust"), 1, "epoch two edited beacon payload\n")
            .expect("epoch two edit");
        index
            .sync_snapshot(&fresh.snapshot().expect("epoch two snapshot"))
            .expect("a fresh epoch content change must publish, not go stale");
        let status = index.status();
        assert_eq!(status.source_revision, 1);
        assert!(
            status.indexed_chunks > 0,
            "the edited content must be indexed"
        );
        let hits = index
            .search(&LexicalSearchRequest::new("edited beacon"))
            .expect("epoch two query")
            .hits;
        assert_eq!(hits.len(), 1, "the beacon must be retrievable durably");
        assert!(hits[0].chunk.text.contains("epoch two edited beacon"));

        // A second cross-epoch snapshot with identical content reuses the
        // generation instead of rebuilding it.
        let generation_before = status.generation.clone();
        fresh
            .replace_file(&path, Some("rust"), 2, "epoch two edited beacon payload\n")
            .expect("same-content replacement");
        index
            .sync_snapshot(&fresh.snapshot().expect("same-content snapshot"))
            .expect("cross-epoch identical content must reuse");
        assert_eq!(
            index.status().generation,
            generation_before,
            "identical content must not mint a new generation cross-epoch"
        );
    }

    #[cfg(feature = "a3s-vec-fts")]
    #[test]
    fn cross_epoch_identical_counters_with_different_content_still_publishes() {
        // Restart race: the fresh epoch's manifest scan completes after an
        // edit, so its first snapshot carries the same (1, 1) counters as the
        // loaded generation while the content differs. Tuple equality must
        // not silently pin the stale generation.
        let directory = tempfile::tempdir().expect("temporary index directory");
        let catalog = WorkspaceChunkCatalog::new_with_engine(
            ChunkingConfig::default(),
            ChunkCatalogLimits::default(),
            WorkspaceLexicalEngine::A3sVec,
        )
        .expect("catalog");
        let path = WorkspacePath::from_normalized("src/lib.rs");
        catalog
            .replace_file(&path, Some("rust"), 1, "pre-edit payload sentinel\n")
            .expect("epoch one replacement");
        {
            let index =
                WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
                    .expect("epoch one persistent index");
            index
                .sync_snapshot(&catalog.snapshot().expect("epoch one snapshot"))
                .expect("epoch one generation");
        }

        let raced = WorkspaceChunkCatalog::new_with_engine(
            ChunkingConfig::default(),
            ChunkCatalogLimits::default(),
            WorkspaceLexicalEngine::A3sVec,
        )
        .expect("raced catalog");
        raced
            .replace_file(&path, Some("rust"), 1, "post-edit beacon payload\n")
            .expect("scanned-after-edit replacement");
        let index =
            WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
                .expect("reopened persistent index");
        assert_eq!(index.status().source_revision, 1);
        index
            .sync_snapshot(&raced.snapshot().expect("raced snapshot"))
            .expect("identical counters with different content must publish");
        let hits = index
            .search(&LexicalSearchRequest::new("post-edit beacon"))
            .expect("query after raced publish")
            .hits;
        assert_eq!(hits.len(), 1, "the raced edit must be retrievable durably");
        assert!(hits[0].chunk.text.contains("post-edit beacon"));
    }

    #[cfg(feature = "a3s-vec-fts")]
    #[test]
    fn incremental_delta_serves_edits_renames_and_reopens() {
        let directory = tempfile::tempdir().expect("temporary index directory");
        let catalog = WorkspaceChunkCatalog::new_with_engine(
            ChunkingConfig::default(),
            ChunkCatalogLimits::default(),
            WorkspaceLexicalEngine::A3sVec,
        )
        .expect("catalog");
        const KEEPMARKERS: [&str; 3] = ["maplefjord", "zebraquill", "harborpine"];
        for (position, name) in ["alpha", "beta", "gamma"].into_iter().enumerate() {
            catalog
                .replace_file(
                    &WorkspacePath::from_normalized(format!("src/{name}.rs")),
                    Some("rust"),
                    1,
                    &format!("pub fn {name}_{}() {{}}\n", KEEPMARKERS[position]),
                )
                .expect("initial admission");
        }
        let index =
            WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
                .expect("persistent index");
        index
            .sync_snapshot(&catalog.snapshot().expect("initial snapshot"))
            .expect("initial generation");
        let initial = index.status();

        // Single-file edit: one changed chunk → bounded delta path. The
        // stale sentinel token exists only in the replaced text, so an OR
        // FTS query against it must return nothing after the delta.
        catalog
            .replace_file(
                &WorkspacePath::from_normalized("src/beta.rs"),
                Some("rust"),
                2,
                "pub fn pinotnoir() {}\n",
            )
            .expect("edit");
        index
            .sync_snapshot(&catalog.snapshot().expect("edited snapshot"))
            .expect("delta generation");
        let edited = index.status();
        assert_ne!(edited.generation, initial.generation);
        let hits = index
            .search(&LexicalSearchRequest::new("pinotnoir"))
            .expect("edited query")
            .hits;
        assert_eq!(hits.len(), 1, "the edited chunk must be retrievable");
        assert!(hits[0].chunk.text.contains("pinotnoir"));
        // The removed posting must be gone: the stale sentinel token
        // existed only in the pre-edit beta text (FTS queries are OR across
        // terms, so the probe must be token-unique).
        assert!(
            index
                .search(&LexicalSearchRequest::new("zebraquill"))
                .expect("stale query")
                .hits
                .is_empty(),
            "the replaced posting must not survive the delta"
        );
        // Untouched chunks keep serving.
        assert_eq!(
            index
                .search(&LexicalSearchRequest::new("maplefjord"))
                .expect("kept query")
                .hits
                .len(),
            1
        );

        // Rename shape: alpha.rs → renamed.rs with identical content is a
        // remove+append pair (chunk ids hash the path).
        catalog
            .remove_file(&WorkspacePath::from_normalized("src/alpha.rs"), 3)
            .expect("remove old path");
        catalog
            .replace_file(
                &WorkspacePath::from_normalized("src/renamed.rs"),
                Some("rust"),
                3,
                "pub fn maplefjord() {}\n",
            )
            .expect("admit renamed path");
        index
            .sync_snapshot(&catalog.snapshot().expect("renamed snapshot"))
            .expect("rename delta generation");
        let renamed_hits = index
            .search(&LexicalSearchRequest::new("maplefjord"))
            .expect("renamed query")
            .hits;
        assert_eq!(renamed_hits.len(), 1, "exactly the renamed file remains");
        assert!(renamed_hits[0].chunk.path.ends_with("renamed.rs"));

        // Reopen: the sparse v3 primary keys must reconstruct the exact
        // mapping (this is the misalignment class the pk field exists for).
        drop(index);
        let reopened =
            WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
                .expect("reopen after delta generations");
        assert_eq!(reopened.status().indexed_chunks, 3);
        let reopened_hits = reopened
            .search(&LexicalSearchRequest::new("maplefjord"))
            .expect("reopened query")
            .hits;
        assert_eq!(reopened_hits.len(), 1);
        assert!(reopened_hits[0].chunk.path.ends_with("renamed.rs"));
        assert_eq!(
            reopened
                .search(&LexicalSearchRequest::new("pinotnoir"))
                .expect("reopened edited query")
                .hits
                .len(),
            1
        );
    }

    #[cfg(feature = "a3s-vec-fts")]
    #[test]
    fn oversized_delta_and_missing_source_fall_back_to_full_builds() {
        let directory = tempfile::tempdir().expect("temporary index directory");
        let catalog = WorkspaceChunkCatalog::new_with_engine(
            ChunkingConfig::default(),
            ChunkCatalogLimits::default(),
            WorkspaceLexicalEngine::A3sVec,
        )
        .expect("catalog");
        const STALE: [&str; 10] = [
            "stalequilla",
            "stalequillb",
            "stalequillc",
            "stalequilld",
            "stalequille",
            "stalequillf",
            "stalequillg",
            "stalequillh",
            "stalequilli",
            "stalequillj",
        ];
        const FRESH: [&str; 10] = [
            "freshferna",
            "freshfernb",
            "freshfernc",
            "freshfernd",
            "freshferne",
            "freshfernf",
            "freshferng",
            "freshfernh",
            "freshferni",
            "freshfern",
        ];
        for (index, stale) in STALE.into_iter().enumerate() {
            catalog
                .replace_file(
                    &WorkspacePath::from_normalized(format!("src/m{index:02}.rs")),
                    Some("rust"),
                    1,
                    &format!("pub fn {stale}() {{}}\n"),
                )
                .expect("initial admission");
        }
        let index =
            WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
                .expect("persistent index");
        index
            .sync_snapshot(&catalog.snapshot().expect("initial snapshot"))
            .expect("initial generation");

        // Churn beyond the cap (> max(64, 30%)): the full build path must
        // still serve the new content.
        for (index, fresh) in FRESH.into_iter().enumerate() {
            catalog
                .replace_file(
                    &WorkspacePath::from_normalized(format!("src/m{index:02}.rs")),
                    Some("rust"),
                    2,
                    &format!("pub fn {fresh}() {{}}\n"),
                )
                .expect("churn admission");
        }
        index
            .sync_snapshot(&catalog.snapshot().expect("churned snapshot"))
            .expect("full rebuild after oversized churn");
        assert_eq!(
            index
                .search(&LexicalSearchRequest::new("freshfernf"))
                .expect("churned query")
                .hits
                .len(),
            1
        );
        assert!(index
            .search(&LexicalSearchRequest::new("stalequillf"))
            .expect("stale query")
            .hits
            .is_empty());

        // A missing delta source (in-memory state references a collection
        // that disappeared) must fail closed to a full build, not error out.
        let generation_dir = index
            .status()
            .generation
            .expect("current generation")
            .to_owned();
        std::fs::remove_dir_all(directory.path().join(&generation_dir))
            .expect("remove source collection");
        catalog
            .replace_file(
                &WorkspacePath::from_normalized("src/m00.rs"),
                Some("rust"),
                3,
                "pub fn recoveredmoss() {}\n",
            )
            .expect("recovery edit");
        index
            .sync_snapshot(&catalog.snapshot().expect("recovery snapshot"))
            .expect("full build recovery after delta source loss");
        assert_eq!(
            index
                .search(&LexicalSearchRequest::new("recoveredmoss"))
                .expect("recovery query")
                .hits
                .len(),
            1
        );
    }

    #[test]
    fn metadata_only_revision_reuses_the_published_generation() {
        let directory = tempfile::tempdir().expect("temporary index directory");
        let catalog = WorkspaceChunkCatalog::new_with_engine(
            ChunkingConfig::default(),
            ChunkCatalogLimits::default(),
            WorkspaceLexicalEngine::A3sVec,
        )
        .expect("catalog");
        let path = WorkspacePath::from_normalized("src/lib.rs");
        catalog
            .replace_file(&path, Some("rust"), 1, "stable content sentinel\n")
            .expect("first replacement");
        let index =
            WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
                .expect("persistent index");
        index
            .sync_snapshot(&catalog.snapshot().expect("first snapshot"))
            .expect("first generation");
        let first = index.status();

        // Rechunk the same bytes under a newer source revision. The chunk
        // identity and FTS postings are unchanged, so a full native rebuild
        // would be pure update amplification.
        catalog
            .replace_file(&path, Some("rust"), 2, "stable content sentinel\n")
            .expect("same-content replacement");
        index
            .sync_snapshot(&catalog.snapshot().expect("second snapshot"))
            .expect("metadata-only update");
        let second = index.status();
        assert_eq!(second.generation, first.generation);
        assert_eq!(second.source_revision, 2);
        assert_eq!(second.catalog_revision, first.catalog_revision + 1);
        assert_eq!(second.indexed_chunks, first.indexed_chunks);
        assert_eq!(
            index
                .search(&LexicalSearchRequest::new("stable sentinel"))
                .expect("reused query")
                .hits
                .len(),
            1
        );
    }

    #[test]
    fn published_replacement_collects_old_generations() {
        let directory = tempfile::tempdir().expect("temporary index directory");
        let catalog = WorkspaceChunkCatalog::new_with_engine(
            ChunkingConfig::default(),
            ChunkCatalogLimits::default(),
            WorkspaceLexicalEngine::A3sVec,
        )
        .expect("catalog");
        let path = WorkspacePath::from_normalized("src/lib.rs");
        let index =
            WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
                .expect("persistent index");

        catalog
            .replace_file(&path, Some("rust"), 1, "generation one sentinel\n")
            .expect("first replacement");
        index
            .sync_snapshot(&catalog.snapshot().expect("first snapshot"))
            .expect("first generation");
        let first_generation = index.status().generation.expect("first generation name");

        catalog
            .replace_file(&path, Some("rust"), 2, "generation two sentinel\n")
            .expect("second replacement");
        index
            .sync_snapshot(&catalog.snapshot().expect("second snapshot"))
            .expect("second generation");
        let second_generation = index.status().generation.expect("second generation name");
        assert_ne!(first_generation, second_generation);

        let generations = std::fs::read_dir(directory.path())
            .expect("persistent index directory")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("generation-"))
            })
            .map(|entry| entry.file_name())
            .collect::<Vec<_>>();
        assert_eq!(
            generations,
            vec![std::ffi::OsString::from(&second_generation)]
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("CURRENT")).expect("CURRENT"),
            format!("{}\n", second_generation)
        );
    }

    #[test]
    fn corrupted_generation_manifest_is_rejected_before_query() {
        let directory = tempfile::tempdir().expect("temporary index directory");
        let catalog = WorkspaceChunkCatalog::new_with_engine(
            ChunkingConfig::default(),
            ChunkCatalogLimits::default(),
            WorkspaceLexicalEngine::A3sVec,
        )
        .expect("catalog");
        catalog
            .replace_file(
                &WorkspacePath::from_normalized("src/lib.rs"),
                Some("rust"),
                1,
                "durable manifest integrity sentinel\n",
            )
            .expect("catalog replacement");

        let index =
            WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
                .expect("persistent index");
        let snapshot = catalog.snapshot().expect("snapshot");
        index.sync_snapshot(&snapshot).expect("generation write");
        let generation = index.status().generation.expect("generation");
        drop(index);

        let manifest_path = directory.path().join(generation).join(MANIFEST_FILE);
        let bytes = std::fs::read(&manifest_path).expect("manifest");
        let mut manifest: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        manifest["chunks"][0]["text"] = serde_json::Value::String("tampered\n".to_owned());
        std::fs::write(
            &manifest_path,
            serde_json::to_vec(&manifest).expect("manifest json"),
        )
        .expect("tampered manifest");

        let reopened =
            WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
                .expect("reopen should remain recoverable");
        assert!(!reopened.is_ready());
        assert_eq!(
            reopened.status().phase,
            super::WorkspacePersistentIndexPhase::Absent
        );

        reopened
            .sync_snapshot(&snapshot)
            .expect("catalog snapshot rebuilds corrupted generation");
        let result = reopened
            .search(&LexicalSearchRequest::new("manifest integrity sentinel"))
            .expect("rebuilt query");
        assert_eq!(result.hits.len(), 1);
        assert_eq!(
            result.hits[0].chunk.text.as_ref(),
            "durable manifest integrity sentinel\n"
        );
    }

    #[test]
    fn status_reports_building_without_hiding_the_last_published_generation() {
        let directory = tempfile::tempdir().expect("temporary index directory");
        let catalog = WorkspaceChunkCatalog::new_with_engine(
            ChunkingConfig::default(),
            ChunkCatalogLimits::default(),
            WorkspaceLexicalEngine::A3sVec,
        )
        .expect("catalog");
        catalog
            .replace_file(
                &WorkspacePath::from_normalized("src/lib.rs"),
                Some("rust"),
                1,
                "published status sentinel\n",
            )
            .expect("catalog replacement");
        let index =
            WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
                .expect("persistent index");
        index
            .sync_snapshot(&catalog.snapshot().expect("snapshot"))
            .expect("generation");
        let ready = index.status();
        index
            .building
            .store(true, std::sync::atomic::Ordering::Release);
        let building = index.status();
        assert_eq!(
            building.phase,
            super::WorkspacePersistentIndexPhase::Building
        );
        assert_eq!(building.generation, ready.generation);
        assert_eq!(building.indexed_chunks, ready.indexed_chunks);
        index
            .building
            .store(false, std::sync::atomic::Ordering::Release);
    }
}
