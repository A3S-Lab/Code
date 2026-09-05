//! Workspace-owned persistent lexical index.
//!
//! The session catalog remains the source of truth for admission and source
//! verification. This module adds an optional durable zvec generation that can
//! be reopened after a process restart. The generation is replaced atomically
//! after a catalog snapshot has been fully built.

use super::catalog::ChunkCatalogSnapshot;
use super::lexical::LexicalSearchRequest;
#[cfg(feature = "zvec-rust-fts")]
use super::lexical::{path_matches, query_terms};
#[cfg(feature = "zvec-rust-fts")]
use super::types::WorkspaceChunk;
use super::types::{WorkspaceIndexError, WorkspaceIndexResult, WorkspaceLexicalEngine};
use serde::{Deserialize, Serialize};
#[cfg(feature = "zvec-rust-fts")]
use std::collections::HashSet;
#[cfg(feature = "zvec-rust-fts")]
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(feature = "zvec-rust-fts")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
#[cfg(feature = "zvec-rust-fts")]
use std::sync::{Condvar, Mutex, RwLock};

#[cfg(feature = "zvec-rust-fts")]
const MANIFEST_SCHEMA_VERSION: u32 = 1;
#[cfg(feature = "zvec-rust-fts")]
const CURRENT_FILE: &str = "CURRENT";
#[cfg(feature = "zvec-rust-fts")]
const MANIFEST_FILE: &str = "manifest.json";

#[cfg(feature = "zvec-rust-fts")]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedChunk {
    id: String,
    path: String,
    language: Option<String>,
    start_line: usize,
    end_line: usize,
    start_byte: usize,
    end_byte: usize,
    content_digest: String,
    source_revision: u64,
    text: String,
}

#[cfg(feature = "zvec-rust-fts")]
impl PersistedChunk {
    fn from_chunk(chunk: &WorkspaceChunk) -> Self {
        Self {
            id: chunk.id.as_str().to_owned(),
            path: chunk.path.to_string(),
            language: chunk.language.as_deref().map(str::to_owned),
            start_line: chunk.start_line,
            end_line: chunk.end_line,
            start_byte: chunk.start_byte,
            end_byte: chunk.end_byte,
            content_digest: chunk.content_digest.to_string(),
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

#[cfg(feature = "zvec-rust-fts")]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerationManifest {
    schema_version: u32,
    lexical_engine: String,
    catalog_revision: u64,
    source_revision: u64,
    chunks: Vec<PersistedChunk>,
}

#[cfg(feature = "zvec-rust-fts")]
struct PersistentState {
    generation: String,
    catalog_revision: u64,
    source_revision: u64,
    indexed_chunks: Arc<[Arc<WorkspaceChunk>]>,
    indexed_files: usize,
    index: super::zvec_rust::ZvecRustLexicalIndex,
}

#[cfg(feature = "zvec-rust-fts")]
struct PersistentOperationGuard {
    active: Arc<(Mutex<usize>, Condvar)>,
}

#[cfg(feature = "zvec-rust-fts")]
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
    #[cfg(feature = "zvec-rust-fts")]
    writer: Mutex<()>,
    #[cfg(feature = "zvec-rust-fts")]
    active_operations: Arc<(Mutex<usize>, Condvar)>,
    #[cfg(feature = "zvec-rust-fts")]
    building: AtomicBool,
    #[cfg(feature = "zvec-rust-fts")]
    state: RwLock<Option<PersistentState>>,
}

#[cfg(feature = "zvec-rust-fts")]
struct BuildActivityGuard<'a>(&'a AtomicBool);

#[cfg(feature = "zvec-rust-fts")]
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
        let root = root.into();
        #[cfg(not(feature = "zvec-rust-fts"))]
        {
            let _ = (&root, engine);
            Err(WorkspaceIndexError::InvalidConfig(
                "persistent workspace indexing requires the zvec-rust-fts feature".to_owned(),
            ))
        }

        #[cfg(feature = "zvec-rust-fts")]
        {
            if engine != WorkspaceLexicalEngine::ZvecRust {
                return Err(WorkspaceIndexError::InvalidConfig(
                    "persistent workspace indexing currently requires the zvec-rust lexical engine"
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
                writer: Mutex::new(()),
                active_operations: Arc::new((Mutex::new(0), Condvar::new())),
                building: AtomicBool::new(false),
                state: RwLock::new(None),
            });
            if let Err(error) = index.load_current() {
                tracing::warn!(%error, path = %index.root.display(), "persistent workspace index will be rebuilt");
            } else if let Some(generation) = index.status().generation {
                if let Err(error) = gc_generations(&index.root, &generation) {
                    tracing::warn!(%error, path = %index.root.display(), "persistent workspace generation cleanup failed");
                }
            }
            Ok(index)
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn engine(&self) -> WorkspaceLexicalEngine {
        self.engine
    }

    pub fn is_ready(&self) -> bool {
        #[cfg(feature = "zvec-rust-fts")]
        {
            self.state
                .read()
                .map(|state| state.is_some())
                .unwrap_or(false)
        }
        #[cfg(not(feature = "zvec-rust-fts"))]
        {
            false
        }
    }

    pub fn status(&self) -> WorkspacePersistentIndexStatus {
        #[cfg(feature = "zvec-rust-fts")]
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
        #[cfg(not(feature = "zvec-rust-fts"))]
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
        #[cfg(not(feature = "zvec-rust-fts"))]
        {
            Err(WorkspaceIndexError::InvalidConfig(
                "persistent workspace indexing requires the zvec-rust-fts feature".to_owned(),
            ))
        }

        #[cfg(feature = "zvec-rust-fts")]
        {
            let _write_guard = self
                .writer
                .lock()
                .map_err(|_| WorkspaceIndexError::LockPoisoned)?;
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
        #[cfg(not(feature = "zvec-rust-fts"))]
        {
            let _ = snapshot;
            Err(WorkspaceIndexError::InvalidConfig(
                "persistent workspace indexing requires the zvec-rust-fts feature".to_owned(),
            ))
        }

        #[cfg(feature = "zvec-rust-fts")]
        {
            let _operation = self.begin_operation()?;
            self.building.store(true, Ordering::Release);
            let _building = BuildActivityGuard(&self.building);
            let _write_guard = self
                .writer
                .lock()
                .map_err(|_| WorkspaceIndexError::LockPoisoned)?;
            let current_state = self
                .state
                .read()
                .map_err(|_| WorkspaceIndexError::LockPoisoned)?;
            if current_state
                .as_ref()
                .is_some_and(|state| state.source_revision > snapshot.source_revision())
            {
                let current = current_state
                    .as_ref()
                    .map(|state| state.source_revision)
                    .unwrap_or_default();
                return Err(WorkspaceIndexError::StaleRevision {
                    requested: snapshot.source_revision(),
                    current,
                });
            }
            if current_state.as_ref().is_some_and(|state| {
                state.catalog_revision == snapshot.revision()
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
                    if let Err(error) = gc_generations(&self.root, &generation) {
                        tracing::warn!(%error, path = %self.root.display(), "persistent workspace generation cleanup failed");
                    }
                    return Ok(());
                }
            }
            drop(current_state);

            let generation = format!("generation-{}", snapshot.revision());
            let staging = self.root.join(format!(".{generation}.staging"));
            let destination = self.root.join(&generation);
            remove_path_if_exists(&staging)?;
            remove_path_if_exists(&destination)?;
            fs::create_dir_all(&staging).map_err(|error| WorkspaceIndexError::ReadFailed {
                path: staging.display().to_string(),
                message: error.to_string(),
            })?;

            let collection_path = staging.join("collection");
            let mut index = super::zvec_rust::ZvecRustLexicalIndex::build_at_path(
                &collection_path,
                indexed_chunks
                    .iter()
                    .map(|chunk| (chunk.id.as_str(), chunk.text.as_ref())),
            )
            .map_err(|error| {
                WorkspaceIndexError::InvalidConfig(format!("persistent zvec index failed: {error}"))
            })?;
            let manifest = GenerationManifest {
                schema_version: MANIFEST_SCHEMA_VERSION,
                lexical_engine: self.engine.stable_id().to_owned(),
                catalog_revision: snapshot.revision(),
                source_revision: snapshot.source_revision(),
                chunks: chunks
                    .iter()
                    .map(|chunk| PersistedChunk::from_chunk(chunk))
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
                index,
            };
            *self
                .state
                .write()
                .map_err(|_| WorkspaceIndexError::LockPoisoned)? = Some(next);
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
        #[cfg(not(feature = "zvec-rust-fts"))]
        {
            let _ = request;
            Err(WorkspaceIndexError::InvalidConfig(
                "persistent workspace indexing requires the zvec-rust-fts feature".to_owned(),
            ))
        }

        #[cfg(feature = "zvec-rust-fts")]
        {
            let _operation = self.begin_operation()?;
            super::lexical::validate_request(request)?;
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
                WorkspaceIndexError::InvalidQuery(format!("zvec-rust FTS search failed: {error}"))
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

    /// Wait until all native work owned by this workspace index has finished.
    ///
    /// Runtime task cancellation cannot interrupt `spawn_blocking`, so backend
    /// teardown uses this boundary before allowing a temporary workspace or
    /// caller-owned root to disappear underneath RocksDB.
    pub(crate) fn wait_for_idle(&self) {
        #[cfg(feature = "zvec-rust-fts")]
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

    #[cfg(feature = "zvec-rust-fts")]
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

    #[cfg(feature = "zvec-rust-fts")]
    fn load_current(&self) -> WorkspaceIndexResult<()> {
        let current = self.root.join(CURRENT_FILE);
        let generation = match fs::read_to_string(&current) {
            Ok(value) => value.trim().to_owned(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(WorkspaceIndexError::ReadFailed {
                    path: current.display().to_string(),
                    message: error.to_string(),
                })
            }
        };
        if generation.is_empty() || !safe_generation_name(&generation) {
            return Err(WorkspaceIndexError::InvalidConfig(
                "persistent zvec CURRENT contains an invalid generation name".to_owned(),
            ));
        }
        let generation_root = self.root.join(&generation);
        let manifest: GenerationManifest = read_json(&generation_root.join(MANIFEST_FILE))?;
        if manifest.schema_version != MANIFEST_SCHEMA_VERSION
            || manifest.lexical_engine != self.engine.stable_id()
        {
            return Err(WorkspaceIndexError::InvalidConfig(
                "persistent zvec index schema or lexical engine is incompatible".to_owned(),
            ));
        }
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
        let index = super::zvec_rust::ZvecRustLexicalIndex::open_persistent(
            generation_root.join("collection"),
            indexed_chunks
                .iter()
                .map(|chunk| (chunk.id.as_str(), chunk.text.as_ref())),
        )
        .map_err(|error| {
            WorkspaceIndexError::InvalidConfig(format!(
                "persistent zvec index failed to open: {error}"
            ))
        })?;
        let indexed_files = distinct_path_count(&indexed_chunks);
        *self
            .state
            .write()
            .map_err(|_| WorkspaceIndexError::LockPoisoned)? = Some(PersistentState {
            generation,
            catalog_revision: manifest.catalog_revision,
            source_revision: manifest.source_revision,
            indexed_chunks,
            indexed_files,
            index,
        });
        Ok(())
    }
}

#[cfg(feature = "zvec-rust-fts")]
fn safe_generation_name(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
}

#[cfg(feature = "zvec-rust-fts")]
fn gc_generations(root: &Path, current_generation: &str) -> WorkspaceIndexResult<()> {
    if !safe_generation_name(current_generation) {
        return Err(WorkspaceIndexError::InvalidConfig(
            "persistent zvec cleanup received an invalid current generation".to_owned(),
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

#[cfg(feature = "zvec-rust-fts")]
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

#[cfg(feature = "zvec-rust-fts")]
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

#[cfg(feature = "zvec-rust-fts")]
fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> WorkspaceIndexResult<()> {
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(value).map_err(|error| {
        WorkspaceIndexError::InvalidConfig(format!(
            "persistent zvec manifest serialization failed: {error}"
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

#[cfg(feature = "zvec-rust-fts")]
fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> WorkspaceIndexResult<T> {
    let bytes = fs::read(path).map_err(|error| WorkspaceIndexError::ReadFailed {
        path: path.display().to_string(),
        message: error.to_string(),
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        WorkspaceIndexError::InvalidConfig(format!("persistent zvec manifest is invalid: {error}"))
    })
}

#[cfg(feature = "zvec-rust-fts")]
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

#[cfg(feature = "zvec-rust-fts")]
fn distinct_path_count(chunks: &[Arc<WorkspaceChunk>]) -> usize {
    chunks
        .iter()
        .map(|chunk| Arc::clone(&chunk.path))
        .collect::<HashSet<_>>()
        .len()
}

#[cfg(all(test, feature = "zvec-rust-fts"))]
mod tests {
    use super::WorkspacePersistentIndex;
    use crate::workspace::retrieval::{
        ChunkCatalogLimits, ChunkingConfig, LexicalSearchRequest, WorkspaceChunkCatalog,
        WorkspaceLexicalEngine,
    };
    use crate::workspace::WorkspacePath;

    #[test]
    fn persistent_generation_survives_reopen_and_replaces_removed_content() {
        let directory = tempfile::tempdir().expect("temporary index directory");
        let catalog = WorkspaceChunkCatalog::new_with_engine(
            ChunkingConfig::default(),
            ChunkCatalogLimits::default(),
            WorkspaceLexicalEngine::ZvecRust,
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
            WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::ZvecRust)
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
            WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::ZvecRust)
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

    #[test]
    fn metadata_only_revision_reuses_the_published_generation() {
        let directory = tempfile::tempdir().expect("temporary index directory");
        let catalog = WorkspaceChunkCatalog::new_with_engine(
            ChunkingConfig::default(),
            ChunkCatalogLimits::default(),
            WorkspaceLexicalEngine::ZvecRust,
        )
        .expect("catalog");
        let path = WorkspacePath::from_normalized("src/lib.rs");
        catalog
            .replace_file(&path, Some("rust"), 1, "stable content sentinel\n")
            .expect("first replacement");
        let index =
            WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::ZvecRust)
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
            WorkspaceLexicalEngine::ZvecRust,
        )
        .expect("catalog");
        let path = WorkspacePath::from_normalized("src/lib.rs");
        let index =
            WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::ZvecRust)
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
    fn status_reports_building_without_hiding_the_last_published_generation() {
        let directory = tempfile::tempdir().expect("temporary index directory");
        let catalog = WorkspaceChunkCatalog::new_with_engine(
            ChunkingConfig::default(),
            ChunkCatalogLimits::default(),
            WorkspaceLexicalEngine::ZvecRust,
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
            WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::ZvecRust)
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
