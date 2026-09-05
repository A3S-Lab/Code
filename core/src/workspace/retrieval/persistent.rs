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
use std::sync::Arc;
#[cfg(feature = "zvec-rust-fts")]
use std::sync::{Mutex, RwLock};

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
    index: super::zvec_rust::ZvecRustLexicalIndex,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspacePersistentIndexPhase {
    Absent,
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
    state: RwLock<Option<PersistentState>>,
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
                state: RwLock::new(None),
            });
            if let Err(error) = index.load_current() {
                tracing::warn!(%error, path = %index.root.display(), "persistent workspace index will be rebuilt");
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
            self.state
                .read()
                .ok()
                .and_then(|state| {
                    state.as_ref().map(|state| WorkspacePersistentIndexStatus {
                        phase: WorkspacePersistentIndexPhase::Ready,
                        generation: Some(state.generation.clone()),
                        catalog_revision: state.catalog_revision,
                        source_revision: state.source_revision,
                        indexed_chunks: state.indexed_chunks.len(),
                    })
                })
                .unwrap_or(WorkspacePersistentIndexStatus {
                    phase: WorkspacePersistentIndexPhase::Absent,
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
            drop(current_state);

            let chunks = snapshot.chunks();
            let indexed_chunks: Arc<[Arc<WorkspaceChunk>]> = Arc::from(
                chunks
                    .iter()
                    .filter(|chunk| !super::lexical::tokenize(chunk.text.as_ref()).is_empty())
                    .cloned()
                    .collect::<Vec<_>>(),
            );
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

            let next = PersistentState {
                generation: generation.clone(),
                catalog_revision: manifest.catalog_revision,
                source_revision: manifest.source_revision,
                indexed_chunks,
                index,
            };
            *self
                .state
                .write()
                .map_err(|_| WorkspaceIndexError::LockPoisoned)? = Some(next);
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

            let matching_paths = state
                .indexed_chunks
                .iter()
                .filter(|chunk| path_matches(chunk.path.as_ref(), &request.path, glob.as_ref()))
                .map(|chunk| chunk.path.clone())
                .collect::<HashSet<_>>();
            let matching_files = matching_paths.len();
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
        *self
            .state
            .write()
            .map_err(|_| WorkspaceIndexError::LockPoisoned)? = Some(PersistentState {
            generation,
            catalog_revision: manifest.catalog_revision,
            source_revision: manifest.source_revision,
            indexed_chunks,
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
}
