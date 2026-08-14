use super::chunk::{chunk_file_with_strategy, ChunkFileRequest};
use super::lexical::{search_catalog, LexicalPartition, LexicalSearchRequest, LexicalSearchResult};
use super::types::{
    ChunkCatalogLimits, ChunkingConfig, WorkspaceChunk, WorkspaceIndexError, WorkspaceIndexResult,
};
use super::WorkspaceChunkingStrategy;
use crate::workspace::{LocalWorkspaceFile, LocalWorkspaceFileStatus, WorkspacePath};
use std::collections::BTreeMap;
use std::path::{Component, Path};
use std::sync::{Arc, RwLock};
use tokio::sync::watch;

/// Immutable, query-safe view of one catalog revision.
#[derive(Clone)]
pub struct ChunkCatalogSnapshot {
    pub(crate) state: Arc<CatalogState>,
}

impl std::fmt::Debug for ChunkCatalogSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChunkCatalogSnapshot")
            .field("revision", &self.revision())
            .field("source_revision", &self.source_revision())
            .field("file_count", &self.file_count())
            .field("chunk_count", &self.chunk_count())
            .field("text_bytes", &self.text_bytes())
            .field("estimated_index_bytes", &self.estimated_index_bytes())
            .finish()
    }
}

impl ChunkCatalogSnapshot {
    pub fn revision(&self) -> u64 {
        self.state.revision
    }

    pub fn source_revision(&self) -> u64 {
        self.state.source_revision
    }

    pub fn file_count(&self) -> usize {
        self.state.files.len()
    }

    pub fn chunk_count(&self) -> usize {
        self.state.chunks.len()
    }

    pub fn text_bytes(&self) -> usize {
        self.state.text_bytes
    }

    pub fn estimated_index_bytes(&self) -> usize {
        self.state.estimated_index_bytes
    }

    /// Number of files admitted by the workspace policy for this source
    /// revision, including files whose catalog build failed.
    pub fn eligible_file_count(&self) -> usize {
        self.state.eligible_file_count
    }

    /// Number of admitted files that failed catalog construction for this
    /// source revision.
    pub fn failed_file_count(&self) -> usize {
        self.state.failed_file_count
    }

    pub fn paths(&self) -> Vec<String> {
        self.state.files.keys().cloned().collect()
    }

    pub fn content_digest(&self, path: &WorkspacePath) -> Option<Arc<str>> {
        self.state
            .files
            .get(path.as_str())
            .map(|file| Arc::clone(&file.content_digest))
    }

    pub fn chunks(&self) -> Arc<[Arc<WorkspaceChunk>]> {
        Arc::clone(&self.state.chunks)
    }

    pub fn lexical_search(
        &self,
        request: &LexicalSearchRequest,
    ) -> Result<LexicalSearchResult, WorkspaceIndexError> {
        search_catalog(self, request)
    }
}

/// Atomic, bounded, session-owned catalog of workspace source chunks.
pub struct WorkspaceChunkCatalog {
    chunking: ChunkingConfig,
    chunking_strategy: WorkspaceChunkingStrategy,
    limits: ChunkCatalogLimits,
    state: RwLock<Arc<CatalogState>>,
    updates: watch::Sender<ChunkCatalogSnapshot>,
}

impl std::fmt::Debug for WorkspaceChunkCatalog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceChunkCatalog")
            .field("chunking", &self.chunking)
            .field("chunking_strategy", &self.chunking_strategy)
            .field("limits", &self.limits)
            .field("snapshot", &self.snapshot().ok())
            .finish()
    }
}

impl WorkspaceChunkCatalog {
    pub fn new(
        chunking: ChunkingConfig,
        limits: ChunkCatalogLimits,
    ) -> WorkspaceIndexResult<Arc<Self>> {
        Self::new_with_strategy(WorkspaceChunkingStrategy::Lines, chunking, limits)
    }

    /// Construct a bounded catalog with an explicit text splitting strategy.
    pub fn new_with_strategy(
        chunking_strategy: WorkspaceChunkingStrategy,
        chunking: ChunkingConfig,
        limits: ChunkCatalogLimits,
    ) -> WorkspaceIndexResult<Arc<Self>> {
        let chunking = chunking.validate()?;
        chunking_strategy
            .validate_for(chunking)
            .map_err(|error| super::chunking_strategy::map_strategy_error("<catalog>", error))?;
        let limits = limits.validate()?;
        let state = Arc::new(CatalogState::default());
        let (updates, _) = watch::channel(ChunkCatalogSnapshot {
            state: Arc::clone(&state),
        });
        Ok(Arc::new(Self {
            chunking,
            chunking_strategy,
            limits,
            state: RwLock::new(state),
            updates,
        }))
    }

    pub(crate) fn default_catalog() -> Arc<Self> {
        let state = Arc::new(CatalogState::default());
        let (updates, _) = watch::channel(ChunkCatalogSnapshot {
            state: Arc::clone(&state),
        });
        Arc::new(Self {
            chunking: ChunkingConfig::default(),
            chunking_strategy: WorkspaceChunkingStrategy::Lines,
            limits: ChunkCatalogLimits::default(),
            state: RwLock::new(state),
            updates,
        })
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<ChunkCatalogSnapshot> {
        self.updates.subscribe()
    }

    pub fn snapshot(&self) -> WorkspaceIndexResult<ChunkCatalogSnapshot> {
        self.state
            .read()
            .map(|state| ChunkCatalogSnapshot {
                state: Arc::clone(&state),
            })
            .map_err(|_| WorkspaceIndexError::LockPoisoned)
    }

    /// Replace one caller-admitted file and atomically publish a new revision.
    pub fn replace_file(
        &self,
        path: &WorkspacePath,
        language: Option<&str>,
        source_revision: u64,
        content: &str,
    ) -> WorkspaceIndexResult<ChunkCatalogSnapshot> {
        if path.is_root() || !safe_relative_path(path.as_str()) {
            return Err(WorkspaceIndexError::InvalidConfig(
                "catalog paths must be normalized workspace-relative files".to_owned(),
            ));
        }
        let file = LocalWorkspaceFile {
            path: path.as_str().to_owned(),
            size: content.len() as u64,
            modified_ms: None,
            language: language.map(str::to_owned),
            status: LocalWorkspaceFileStatus::Unknown,
            binary: false,
            generated: false,
        };
        let replacement = Arc::new(CatalogFile::build(
            file,
            source_revision,
            content,
            self.chunking,
            &self.chunking_strategy,
        )?);
        let mut state = self
            .state
            .write()
            .map_err(|_| WorkspaceIndexError::LockPoisoned)?;
        let mut files = state.files.clone();
        files.insert(path.as_str().to_owned(), replacement);
        let eligible_file_count = files.len();
        self.publish_locked(&mut state, source_revision, files, eligible_file_count, 0)
    }

    pub fn remove_file(
        &self,
        path: &WorkspacePath,
        source_revision: u64,
    ) -> WorkspaceIndexResult<ChunkCatalogSnapshot> {
        if path.is_root() || !safe_relative_path(path.as_str()) {
            return Err(WorkspaceIndexError::InvalidConfig(
                "catalog paths must be normalized workspace-relative files".to_owned(),
            ));
        }
        let mut state = self
            .state
            .write()
            .map_err(|_| WorkspaceIndexError::LockPoisoned)?;
        let mut files = state.files.clone();
        files.remove(path.as_str());
        let eligible_file_count = files.len();
        self.publish_locked(&mut state, source_revision, files, eligible_file_count, 0)
    }

    pub fn clear(&self, source_revision: u64) -> WorkspaceIndexResult<ChunkCatalogSnapshot> {
        let mut state = self
            .state
            .write()
            .map_err(|_| WorkspaceIndexError::LockPoisoned)?;
        self.publish_locked(&mut state, source_revision, BTreeMap::new(), 0, 0)
    }

    pub(crate) fn chunking(&self) -> ChunkingConfig {
        self.chunking
    }

    pub(crate) fn chunking_strategy(&self) -> WorkspaceChunkingStrategy {
        self.chunking_strategy.clone()
    }

    pub(crate) fn limits(&self) -> ChunkCatalogLimits {
        self.limits
    }

    pub(crate) fn publish_reconciliation(
        &self,
        expected_revision: u64,
        source_revision: u64,
        files: BTreeMap<String, Arc<CatalogFile>>,
        eligible_file_count: usize,
        failed_file_count: usize,
    ) -> WorkspaceIndexResult<ChunkCatalogSnapshot> {
        let mut state = self
            .state
            .write()
            .map_err(|_| WorkspaceIndexError::LockPoisoned)?;
        if state.revision != expected_revision {
            return Err(WorkspaceIndexError::ConcurrentUpdate {
                expected: expected_revision,
                actual: state.revision,
            });
        }
        self.publish_locked(
            &mut state,
            source_revision,
            files,
            eligible_file_count,
            failed_file_count,
        )
    }

    fn publish_locked(
        &self,
        state: &mut Arc<CatalogState>,
        source_revision: u64,
        files: BTreeMap<String, Arc<CatalogFile>>,
        eligible_file_count: usize,
        failed_file_count: usize,
    ) -> WorkspaceIndexResult<ChunkCatalogSnapshot> {
        if files.len().saturating_add(failed_file_count) > eligible_file_count {
            return Err(WorkspaceIndexError::InvalidConfig(
                "catalog coverage counts are inconsistent".to_owned(),
            ));
        }
        let usage = CatalogUsage::from_files(files.values(), self.limits)?;
        let chunks = files
            .values()
            .flat_map(|file| file.chunks.iter().cloned())
            .collect::<Vec<_>>();
        if source_revision < state.source_revision {
            return Err(WorkspaceIndexError::StaleRevision {
                requested: source_revision,
                current: state.source_revision,
            });
        }
        let next = Arc::new(CatalogState {
            revision: state.revision.saturating_add(1),
            source_revision,
            files,
            chunks: Arc::from(chunks),
            text_bytes: usage.text_bytes,
            estimated_index_bytes: usage.index_bytes,
            eligible_file_count,
            failed_file_count,
        });
        *state = Arc::clone(&next);
        let snapshot = ChunkCatalogSnapshot { state: next };
        self.updates.send_replace(snapshot.clone());
        Ok(snapshot)
    }
}

fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CatalogUsage {
    files: usize,
    chunks: usize,
    text_bytes: usize,
    index_bytes: usize,
}

impl CatalogUsage {
    pub(crate) fn from_files<'a>(
        files: impl IntoIterator<Item = &'a Arc<CatalogFile>>,
        limits: ChunkCatalogLimits,
    ) -> WorkspaceIndexResult<Self> {
        let mut usage = Self::default();
        for file in files {
            usage.try_add(file, limits)?;
        }
        Ok(usage)
    }

    pub(crate) fn try_add(
        &mut self,
        file: &CatalogFile,
        limits: ChunkCatalogLimits,
    ) -> WorkspaceIndexResult<()> {
        let requested_files = self.files.saturating_add(1);
        let requested_chunks = self.chunks.saturating_add(file.chunks.len());
        let requested_text = self.text_bytes.saturating_add(file.text_bytes);
        let requested_index = self.index_bytes.saturating_add(file.estimated_index_bytes);
        check_limit("file count", requested_files, limits.max_files)?;
        check_limit("chunk count", requested_chunks, limits.max_chunks)?;
        check_limit("text byte", requested_text, limits.max_text_bytes)?;
        check_limit(
            "index byte estimate",
            requested_index,
            limits.max_index_bytes,
        )?;
        self.files = requested_files;
        self.chunks = requested_chunks;
        self.text_bytes = requested_text;
        self.index_bytes = requested_index;
        Ok(())
    }
}

fn check_limit(resource: &'static str, requested: usize, limit: usize) -> WorkspaceIndexResult<()> {
    if requested > limit {
        return Err(WorkspaceIndexError::BudgetExceeded {
            resource,
            requested,
            limit,
        });
    }
    Ok(())
}

#[derive(Default)]
pub(crate) struct CatalogState {
    pub(crate) revision: u64,
    pub(crate) source_revision: u64,
    pub(crate) files: BTreeMap<String, Arc<CatalogFile>>,
    pub(crate) chunks: Arc<[Arc<WorkspaceChunk>]>,
    pub(crate) text_bytes: usize,
    pub(crate) estimated_index_bytes: usize,
    pub(crate) eligible_file_count: usize,
    pub(crate) failed_file_count: usize,
}

pub(crate) struct CatalogFile {
    pub(crate) manifest: LocalWorkspaceFile,
    pub(crate) content_digest: Arc<str>,
    pub(crate) chunks: Arc<[Arc<WorkspaceChunk>]>,
    pub(crate) lexical: Arc<LexicalPartition>,
    pub(crate) text_bytes: usize,
    pub(crate) estimated_index_bytes: usize,
}

impl CatalogFile {
    pub(crate) fn build(
        manifest: LocalWorkspaceFile,
        source_revision: u64,
        content: &str,
        chunking: ChunkingConfig,
        chunking_strategy: &WorkspaceChunkingStrategy,
    ) -> WorkspaceIndexResult<Self> {
        let chunked = chunk_file_with_strategy(
            ChunkFileRequest {
                path: &manifest.path,
                language: manifest.language.as_deref(),
                source_revision,
                content,
            },
            chunking,
            chunking_strategy,
        )?;
        let chunks: Arc<[Arc<WorkspaceChunk>]> = Arc::from(chunked.chunks);
        let lexical = Arc::new(LexicalPartition::build(Arc::clone(&chunks)));
        let estimated_index_bytes = lexical
            .estimated_bytes()
            .saturating_add(
                chunks
                    .len()
                    .saturating_mul(std::mem::size_of::<WorkspaceChunk>()),
            )
            .saturating_add(manifest.path.capacity())
            .saturating_add(chunked.content_digest.len());
        Ok(Self {
            manifest,
            content_digest: chunked.content_digest,
            chunks,
            lexical,
            text_bytes: chunked.text_bytes,
            estimated_index_bytes,
        })
    }

    pub(crate) fn matches_manifest(&self, candidate: &LocalWorkspaceFile) -> bool {
        self.manifest.path == candidate.path
            && self.manifest.size == candidate.size
            && self.manifest.modified_ms == candidate.modified_ms
            && self.manifest.language == candidate.language
            && self.manifest.binary == candidate.binary
            && self.manifest.generated == candidate.generated
    }

    pub(crate) fn with_manifest(&self, manifest: LocalWorkspaceFile) -> Self {
        Self {
            manifest,
            content_digest: Arc::clone(&self.content_digest),
            chunks: Arc::clone(&self.chunks),
            lexical: Arc::clone(&self.lexical),
            text_bytes: self.text_bytes,
            estimated_index_bytes: self.estimated_index_bytes,
        }
    }
}
