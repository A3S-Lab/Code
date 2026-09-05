//! Manifest-backed local workspace services.
//!
//! The manifest is an in-memory index of workspace files. It is built
//! asynchronously, refreshed from filesystem notifications, and used by the
//! local search backend (`glob`/`grep`) to avoid walking the filesystem for
//! every agent tool call. File I/O, command execution, and git operations still
//! delegate to [`LocalWorkspaceBackend`].

use super::retrieval::{
    ChunkCatalogLimits, ChunkingConfig, LocalWorkspaceCatalogRuntime, WorkspaceChunkCatalog,
    WorkspaceChunkingStrategy, WorkspaceIndexError, WorkspaceLexicalEngine,
    WorkspacePersistentIndex,
};
use super::{
    escape_control_chars_for_display, validate_relative_pattern, CommandOutput, CommandRequest,
    LocalWorkspaceAccessPolicy, LocalWorkspaceBackend, WorkspaceCommandRunner, WorkspaceDirEntry,
    WorkspaceFileSystem, WorkspaceGit, WorkspaceGitBranch, WorkspaceGitCheckoutOutput,
    WorkspaceGitCheckoutRequest, WorkspaceGitCommit, WorkspaceGitCreateBranchRequest,
    WorkspaceGitCreateWorktreeRequest, WorkspaceGitDiffRequest, WorkspaceGitRemote,
    WorkspaceGitRemoveWorktreeRequest, WorkspaceGitStash, WorkspaceGitStashProvider,
    WorkspaceGitStashRequest, WorkspaceGitStatus, WorkspaceGitWorktree,
    WorkspaceGitWorktreeMutation, WorkspaceGitWorktreeProvider, WorkspaceGlobRequest,
    WorkspaceGlobResult, WorkspaceGrepOutcome, WorkspaceGrepRequest, WorkspaceGrepResult,
    WorkspacePath, WorkspacePathResolver, WorkspaceResult, WorkspaceSearch, WorkspaceTextRange,
    WorkspaceTextReader, WorkspaceWriteOutcome,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock, RwLock,
};
#[cfg(test)]
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, watch};

mod file_kind;
mod scanner;
mod watcher;
use scanner::is_relevant_event;
pub use scanner::scan_workspace_files;
use watcher::run_manifest_task;

const SNAPSHOT_CHANNEL_CAPACITY: usize = 16;
const FILE_CHANGE_CHANNEL_CAPACITY: usize = 256;
const RECENT_FILE_LIMIT: usize = 128;
const RECENT_DECAY_HALF_LIFE_MS: f32 = 10.0 * 60.0 * 1000.0;
const RECENT_FREQUENCY_NORMALIZER: f32 = 16.0;
const RECENT_RECENCY_WEIGHT: f32 = 0.75;

/// Git/workspace status for a file in the manifest.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum LocalWorkspaceFileStatus {
    Tracked,
    Untracked,
    Unknown,
}

/// One manifest entry.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct LocalWorkspaceFile {
    pub path: String,
    pub size: u64,
    pub modified_ms: Option<u64>,
    pub language: Option<String>,
    pub status: LocalWorkspaceFileStatus,
    pub binary: bool,
    pub generated: bool,
}

/// Recency/usage score for a workspace file the user or agent touched.
///
/// Hosts should treat this as a ranking hint, not as an authoritative file
/// list. The manifest filters deleted files when exposing recent entries.
#[derive(Clone, Debug, PartialEq)]
pub struct RecentWorkspaceFile {
    pub path: String,
    pub score: f32,
    pub touched_at_ms: u64,
    pub touch_count: u32,
}

/// Immutable manifest snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalWorkspaceManifestSnapshot {
    pub version: u64,
    pub root: PathBuf,
    pub files: Vec<LocalWorkspaceFile>,
    pub scanned_at_ms: u64,
}

/// The normalized kind of a workspace file change.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum WorkspaceFileChangeKind {
    Created,
    Changed,
    Deleted,
}

/// A filesystem change for one normalized workspace-relative file path.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct WorkspaceFileChange {
    pub path: WorkspacePath,
    pub kind: WorkspaceFileChangeKind,
}

impl LocalWorkspaceManifestSnapshot {
    pub fn empty(root: PathBuf) -> Self {
        Self {
            version: 0,
            root,
            files: Vec::new(),
            scanned_at_ms: now_ms(),
        }
    }

    pub fn file_paths(&self) -> Vec<String> {
        self.files.iter().map(|file| file.path.clone()).collect()
    }
}

/// Shared in-memory workspace manifest.
pub struct LocalWorkspaceManifest {
    state: Arc<RwLock<ManifestState>>,
    recent: Arc<RwLock<RecentFiles>>,
    snapshots: broadcast::Sender<LocalWorkspaceManifestSnapshot>,
    changes: broadcast::Sender<WorkspaceFileChange>,
    activation: watch::Sender<bool>,
    scan_cancelled: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl LocalWorkspaceManifest {
    /// Start the manifest scanner/watcher for `root`.
    pub fn start(root: impl Into<PathBuf>) -> Arc<Self> {
        Self::start_with_activation(root, true)
    }

    /// Create an empty manifest whose scanner and watcher start only after
    /// [`Self::activate`] is called.
    ///
    /// Search-capable hosts can use this during a latency-sensitive bootstrap:
    /// manifest-backed operations retain their local filesystem fallback while
    /// the initial snapshot is empty, so deferring discovery does not make the
    /// workspace inaccessible.
    pub fn start_deferred(root: impl Into<PathBuf>) -> Arc<Self> {
        Self::start_with_activation(root, false)
    }

    fn start_with_activation(root: impl Into<PathBuf>, active: bool) -> Arc<Self> {
        let root = root.into();
        let root = root.canonicalize().unwrap_or_else(|_| root.clone());
        let initial = LocalWorkspaceManifestSnapshot::empty(root.clone());
        let state = Arc::new(RwLock::new(ManifestState {
            fingerprint: fingerprint_files(&initial.files),
            index: Arc::new(ManifestIndex::build(&initial.files)),
            snapshot: Arc::new(initial),
        }));
        let recent = Arc::new(RwLock::new(RecentFiles::default()));
        let (snapshots, _) = broadcast::channel(SNAPSHOT_CHANNEL_CAPACITY);
        let (changes, _) = broadcast::channel(FILE_CHANGE_CHANNEL_CAPACITY);
        let (activation, mut activation_rx) = watch::channel(active);
        let scan_cancelled = Arc::new(AtomicBool::new(false));
        let task_state = Arc::clone(&state);
        let task_snapshots = snapshots.clone();
        let task_changes = changes.clone();
        let task_scan_cancelled = Arc::clone(&scan_cancelled);
        let task = tokio::spawn(async move {
            if !*activation_rx.borrow() && activation_rx.wait_for(|active| *active).await.is_err() {
                return;
            }
            if task_scan_cancelled.load(Ordering::Acquire) {
                return;
            }
            run_manifest_task(
                root,
                task_state,
                task_snapshots,
                task_changes,
                task_scan_cancelled,
            )
            .await;
        });
        Arc::new(Self {
            state,
            recent,
            snapshots,
            changes,
            activation,
            scan_cancelled,
            task,
        })
    }

    /// Open a deferred manifest's one-way startup gate.
    ///
    /// Returns `true` only for the call that transitions the manifest from
    /// deferred to active. Calling this on an eagerly started or already active
    /// manifest is harmless and returns `false`.
    pub fn activate(&self) -> bool {
        self.activation.send_if_modified(|active| {
            if *active {
                false
            } else {
                *active = true;
                true
            }
        })
    }

    /// Whether discovery has been activated for this manifest.
    pub fn is_active(&self) -> bool {
        *self.activation.borrow()
    }

    pub fn snapshot(&self) -> LocalWorkspaceManifestSnapshot {
        self.state
            .read()
            .map(|state| (*state.snapshot).clone())
            .unwrap_or_else(|_| LocalWorkspaceManifestSnapshot::empty(PathBuf::new()))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<LocalWorkspaceManifestSnapshot> {
        self.snapshots.subscribe()
    }

    /// Subscribe to debounced, workspace-relative filesystem changes.
    pub fn subscribe_changes(&self) -> broadcast::Receiver<WorkspaceFileChange> {
        self.changes.subscribe()
    }

    /// Stop background discovery without waiting for an in-flight synchronous scan.
    ///
    /// Hosts with an explicit lifecycle should call this before shutting down
    /// their Tokio runtime. [`Drop`] is only a fallback because other background
    /// services may retain an `Arc` to the manifest until runtime teardown.
    pub fn shutdown(&self) {
        self.scan_cancelled.store(true, Ordering::Release);
        self.task.abort();
    }

    /// Record that a workspace-relative file was opened, read, or written.
    ///
    /// This intentionally does not require the initial manifest scan to have
    /// completed. The public recent-file views filter against the current
    /// manifest index, so early touches become visible after the file is indexed
    /// and deleted files disappear automatically.
    pub fn touch_file(&self, path: impl AsRef<str>) -> bool {
        let Some(path) = normalize_recent_file_path(path.as_ref()) else {
            return false;
        };
        let Ok(mut recent) = self.recent.write() else {
            return false;
        };
        recent.touch(path, now_ms());
        true
    }

    /// Return the hottest known files, newest/frequently used first.
    pub fn recent_file_entries(&self, limit: usize) -> Vec<RecentWorkspaceFile> {
        if limit == 0 {
            return Vec::new();
        }
        let Some(index) = self.state.read().ok().map(|state| Arc::clone(&state.index)) else {
            return Vec::new();
        };
        self.recent
            .read()
            .map(|recent| recent.entries(Some(&index), limit, now_ms()))
            .unwrap_or_default()
    }

    /// Return recent file paths only, preserving hot-file order.
    pub fn recent_file_paths(&self, limit: usize) -> Vec<String> {
        self.recent_file_entries(limit)
            .into_iter()
            .map(|entry| entry.path)
            .collect()
    }
}

impl Drop for LocalWorkspaceManifest {
    fn drop(&mut self) {
        // Aborting the async owner does not stop synchronous discovery that has
        // already begun. Signal the scanner first so a detached traversal stops
        // consuming filesystem resources after its host has gone away.
        self.shutdown();
    }
}

struct ManifestState {
    fingerprint: u64,
    index: Arc<ManifestIndex>,
    snapshot: Arc<LocalWorkspaceManifestSnapshot>,
}

#[derive(Debug, Default)]
struct RecentFiles {
    entries: HashMap<String, RecentFileState>,
    next_sequence: u64,
}

impl RecentFiles {
    fn touch(&mut self, path: String, now: u64) {
        self.next_sequence = self.next_sequence.saturating_add(1);
        let sequence = self.next_sequence;
        self.entries
            .entry(path.clone())
            .and_modify(|entry| {
                entry.touched_at_ms = now;
                entry.touch_count = entry.touch_count.saturating_add(1);
                entry.sequence = sequence;
            })
            .or_insert(RecentFileState {
                path,
                touched_at_ms: now,
                touch_count: 1,
                sequence,
            });
        self.prune(now);
    }

    fn entries(
        &self,
        index: Option<&ManifestIndex>,
        limit: usize,
        now: u64,
    ) -> Vec<RecentWorkspaceFile> {
        let mut entries = self
            .entries
            .values()
            .filter(|entry| {
                index
                    .map(|index| index.by_path.contains_key(&entry.path))
                    .unwrap_or(true)
            })
            .map(|entry| {
                let score = recent_score(entry, now);
                (
                    entry.sequence,
                    RecentWorkspaceFile {
                        path: entry.path.clone(),
                        score,
                        touched_at_ms: entry.touched_at_ms,
                        touch_count: entry.touch_count,
                    },
                )
            })
            .collect::<Vec<_>>();

        entries.sort_by(|(left_sequence, left), (right_sequence, right)| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| right.touched_at_ms.cmp(&left.touched_at_ms))
                .then_with(|| right_sequence.cmp(left_sequence))
                .then_with(|| left.path.cmp(&right.path))
        });
        entries
            .into_iter()
            .take(limit)
            .map(|(_, entry)| entry)
            .collect()
    }

    fn prune(&mut self, now: u64) {
        if self.entries.len() <= RECENT_FILE_LIMIT {
            return;
        }

        let keep = self
            .entries
            .values()
            .map(|entry| (entry.path.clone(), recent_score(entry, now), entry.sequence))
            .collect::<Vec<_>>();
        let mut keep = keep;
        keep.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| right.2.cmp(&left.2))
                .then_with(|| left.0.cmp(&right.0))
        });
        let keep = keep
            .into_iter()
            .take(RECENT_FILE_LIMIT)
            .map(|(path, _, _)| path)
            .collect::<HashSet<_>>();
        self.entries.retain(|path, _| keep.contains(path));
    }
}

#[derive(Debug)]
struct RecentFileState {
    path: String,
    touched_at_ms: u64,
    touch_count: u32,
    sequence: u64,
}

#[derive(Debug, Default)]
struct ManifestIndex {
    all: Vec<usize>,
    by_path: HashMap<String, usize>,
    by_basename: HashMap<String, Vec<usize>>,
    by_extension: HashMap<String, Vec<usize>>,
}

impl ManifestIndex {
    fn build(files: &[LocalWorkspaceFile]) -> Self {
        let mut index = Self {
            all: Vec::with_capacity(files.len()),
            by_path: HashMap::with_capacity(files.len()),
            by_basename: HashMap::new(),
            by_extension: HashMap::new(),
        };

        for (file_index, file) in files.iter().enumerate() {
            index.all.push(file_index);
            index.by_path.insert(file.path.clone(), file_index);
            if let Some(name) = Path::new(&file.path)
                .file_name()
                .and_then(|name| name.to_str())
            {
                index
                    .by_basename
                    .entry(name.to_string())
                    .or_default()
                    .push(file_index);
            }
            if let Some(extension) = Path::new(&file.path)
                .extension()
                .and_then(|extension| extension.to_str())
                .filter(|extension| !extension.is_empty())
            {
                index
                    .by_extension
                    .entry(extension.to_string())
                    .or_default()
                    .push(file_index);
            }
        }

        index
    }
}

struct ManifestSearchSnapshot {
    snapshot: Arc<LocalWorkspaceManifestSnapshot>,
    index: Arc<ManifestIndex>,
}

/// Local backend that uses an in-memory manifest for search.
pub struct ManifestWorkspaceBackend {
    local: Arc<LocalWorkspaceBackend>,
    catalog_local: Arc<LocalWorkspaceBackend>,
    manifest: Arc<LocalWorkspaceManifest>,
    catalog_runtime: OnceLock<Arc<LocalWorkspaceCatalogRuntime>>,
    persistent_index: OnceLock<Arc<WorkspacePersistentIndex>>,
}

impl ManifestWorkspaceBackend {
    pub fn new(root: impl Into<PathBuf>) -> Arc<Self> {
        Self::new_with_access_policy(root, LocalWorkspaceAccessPolicy::Unrestricted)
    }

    pub fn new_with_access_policy(
        root: impl Into<PathBuf>,
        access_policy: LocalWorkspaceAccessPolicy,
    ) -> Arc<Self> {
        Self::new_with_access_policy_and_activation(root, access_policy, true)
    }

    /// Create a manifest-backed local workspace whose initial discovery is
    /// explicitly activated through [`LocalWorkspaceManifest::activate`].
    pub fn new_deferred(root: impl Into<PathBuf>) -> Arc<Self> {
        Self::new_deferred_with_access_policy(root, LocalWorkspaceAccessPolicy::Unrestricted)
    }

    /// Create a policy-constrained manifest backend without starting its
    /// scanner or platform watcher until the manifest is activated.
    pub fn new_deferred_with_access_policy(
        root: impl Into<PathBuf>,
        access_policy: LocalWorkspaceAccessPolicy,
    ) -> Arc<Self> {
        Self::new_with_access_policy_and_activation(root, access_policy, false)
    }

    fn new_with_access_policy_and_activation(
        root: impl Into<PathBuf>,
        access_policy: LocalWorkspaceAccessPolicy,
        active: bool,
    ) -> Arc<Self> {
        let root = root.into();
        let local = Arc::new(LocalWorkspaceBackend::new_with_access_policy(
            root,
            access_policy,
        ));
        let catalog_local = Arc::new(LocalWorkspaceBackend::new_with_source_egress_policy(
            local.root.clone(),
        ));
        let manifest = if active {
            LocalWorkspaceManifest::start(local.root.clone())
        } else {
            LocalWorkspaceManifest::start_deferred(local.root.clone())
        };
        Arc::new(Self {
            local,
            catalog_local,
            manifest,
            catalog_runtime: OnceLock::new(),
            persistent_index: OnceLock::new(),
        })
    }

    pub fn from_manifest(
        local: Arc<LocalWorkspaceBackend>,
        manifest: Arc<LocalWorkspaceManifest>,
    ) -> Arc<Self> {
        let catalog_local = Arc::new(LocalWorkspaceBackend::new_with_source_egress_policy(
            local.root.clone(),
        ));
        Arc::new(Self {
            catalog_local,
            local,
            manifest,
            catalog_runtime: OnceLock::new(),
            persistent_index: OnceLock::new(),
        })
    }

    pub fn manifest(&self) -> Arc<LocalWorkspaceManifest> {
        Arc::clone(&self.manifest)
    }

    /// Enable and return a session-local catalog built from the shared manifest.
    ///
    /// Catalog reads always use the source-egress boundary independently from
    /// the access policy selected for ordinary workspace tools. Unless
    /// [`Self::configure_chunk_catalog`] ran first, this enables the compatible
    /// default catalog configuration.
    pub fn chunk_catalog(&self) -> Arc<WorkspaceChunkCatalog> {
        self.catalog_runtime
            .get_or_init(|| {
                let file_system: Arc<dyn WorkspaceFileSystem> = self.catalog_local.clone();
                let persistent = self.persistent_index.get().cloned();
                if let Some(persistent) = persistent {
                    LocalWorkspaceCatalogRuntime::start_with_catalog_and_persistent(
                        Arc::clone(&self.manifest),
                        file_system,
                        WorkspaceChunkCatalog::default_catalog(),
                        Some(persistent),
                    )
                } else {
                    LocalWorkspaceCatalogRuntime::start(Arc::clone(&self.manifest), file_system)
                }
            })
            .catalog()
    }

    /// Enable a workspace-owned persistent zvec FTS index before the catalog
    /// is initialized. The existing manifest watcher remains the sole source
    /// of updates; this only adds a durable projection of each catalog
    /// snapshot.
    pub fn configure_persistent_index(
        &self,
        root: impl Into<PathBuf>,
    ) -> Result<Arc<WorkspacePersistentIndex>, WorkspaceIndexError> {
        if self.catalog_runtime.get().is_some() {
            return Err(WorkspaceIndexError::InvalidConfig(
                "persistent workspace indexing must be configured before the chunk catalog"
                    .to_owned(),
            ));
        }
        let index = WorkspacePersistentIndex::open(root, WorkspaceLexicalEngine::ZvecRust)?;
        self.persistent_index.set(Arc::clone(&index)).map_err(|_| {
            WorkspaceIndexError::InvalidConfig(
                "persistent workspace index was already configured".to_owned(),
            )
        })?;
        Ok(index)
    }

    pub fn persistent_index(&self) -> Option<Arc<WorkspacePersistentIndex>> {
        self.persistent_index.get().cloned()
    }

    /// Configure and enable the catalog owned by this shared manifest backend.
    ///
    /// Hosts that supply [`super::WorkspaceServices`] to a session must call
    /// this before [`Self::chunk_catalog`] or a retrieval service constructor
    /// enables the default catalog. Configuration is one-shot so later
    /// sessions cannot silently replace the strategy or budgets owned by the
    /// host.
    pub fn configure_chunk_catalog(
        &self,
        strategy: WorkspaceChunkingStrategy,
        chunking: ChunkingConfig,
        limits: ChunkCatalogLimits,
    ) -> Result<Arc<WorkspaceChunkCatalog>, WorkspaceIndexError> {
        self.configure_chunk_catalog_with_engine(
            strategy,
            chunking,
            limits,
            WorkspaceLexicalEngine::default(),
        )
    }

    /// Configure the catalog with an explicit typed lexical engine.
    pub fn configure_chunk_catalog_with_engine(
        &self,
        strategy: WorkspaceChunkingStrategy,
        chunking: ChunkingConfig,
        limits: ChunkCatalogLimits,
        lexical_engine: WorkspaceLexicalEngine,
    ) -> Result<Arc<WorkspaceChunkCatalog>, WorkspaceIndexError> {
        let catalog = WorkspaceChunkCatalog::new_with_strategy_and_engine(
            strategy,
            chunking,
            limits,
            lexical_engine,
        )?;
        let file_system: Arc<dyn WorkspaceFileSystem> = self.catalog_local.clone();
        let runtime = LocalWorkspaceCatalogRuntime::start_with_catalog_and_persistent(
            Arc::clone(&self.manifest),
            file_system,
            Arc::clone(&catalog),
            self.persistent_index.get().cloned(),
        );
        if let Err(runtime) = self.catalog_runtime.set(runtime) {
            runtime.shutdown();
            return Err(WorkspaceIndexError::InvalidConfig(
                "workspace chunk catalog was already initialized".to_owned(),
            ));
        }
        Ok(catalog)
    }

    /// Stop the local manifest and any lazily enabled catalog projection.
    pub(crate) fn shutdown(&self) {
        if let Some(runtime) = self.catalog_runtime.get() {
            runtime.shutdown();
        }
        self.manifest.shutdown();
    }

    pub fn local_root(&self) -> &Path {
        &self.local.root
    }

    fn manifest_ready(&self) -> Option<ManifestSearchSnapshot> {
        let state = self.manifest.state.read().ok()?;
        (state.snapshot.version > 0).then(|| ManifestSearchSnapshot {
            snapshot: Arc::clone(&state.snapshot),
            index: Arc::clone(&state.index),
        })
    }

    fn fallback_search(&self) -> Arc<LocalWorkspaceBackend> {
        Arc::clone(&self.local)
    }

    fn recent_path_ranks(&self, index: &ManifestIndex) -> HashMap<String, usize> {
        self.manifest
            .recent
            .read()
            .map(|recent| {
                recent
                    .entries(Some(index), RECENT_FILE_LIMIT, now_ms())
                    .into_iter()
                    .enumerate()
                    .map(|(rank, entry)| (entry.path, rank))
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl WorkspacePathResolver for ManifestWorkspaceBackend {
    fn normalize(&self, input: &str) -> Result<WorkspacePath> {
        self.local.normalize(input)
    }
}

#[async_trait]
impl WorkspaceFileSystem for ManifestWorkspaceBackend {
    async fn read_text(&self, path: &WorkspacePath) -> WorkspaceResult<String> {
        let content = self.local.read_text(path).await?;
        self.manifest.touch_file(path.as_str());
        Ok(content)
    }

    async fn write_text(
        &self,
        path: &WorkspacePath,
        content: &str,
    ) -> WorkspaceResult<WorkspaceWriteOutcome> {
        let outcome = self.local.write_text(path, content).await?;
        self.manifest.touch_file(path.as_str());
        Ok(outcome)
    }

    async fn list_dir(&self, path: &WorkspacePath) -> WorkspaceResult<Vec<WorkspaceDirEntry>> {
        self.local.list_dir(path).await
    }
}

#[async_trait]
impl WorkspaceTextReader for ManifestWorkspaceBackend {
    async fn read_text_range(
        &self,
        path: &WorkspacePath,
        offset: usize,
        limit: usize,
    ) -> WorkspaceResult<WorkspaceTextRange> {
        let range = self.local.read_text_range(path, offset, limit).await?;
        self.manifest.touch_file(path.as_str());
        Ok(range)
    }
}

#[async_trait]
impl WorkspaceCommandRunner for ManifestWorkspaceBackend {
    async fn exec(&self, request: CommandRequest) -> Result<CommandOutput> {
        self.local.exec(request).await
    }
}

#[async_trait]
impl WorkspaceSearch for ManifestWorkspaceBackend {
    async fn glob(&self, request: WorkspaceGlobRequest) -> Result<WorkspaceGlobResult> {
        validate_relative_pattern(&request.pattern, "glob pattern")?;
        let Some(search_snapshot) = self.manifest_ready() else {
            return self.fallback_search().glob(request).await;
        };
        let pattern = glob::Pattern::new(&request.pattern)
            .map_err(|e| anyhow!("Invalid glob pattern '{}': {}", request.pattern, e))?;
        let candidates =
            candidate_indices_for_glob(&search_snapshot.index, &request.base, &request.pattern);
        let recent_ranks = self.recent_path_ranks(&search_snapshot.index);

        let mut matches = Vec::new();
        for file_index in
            recent_first_candidate_indices(&candidates, &search_snapshot.index, &recent_ranks)
        {
            let Some(file) = search_snapshot.snapshot.files.get(file_index) else {
                continue;
            };
            let Some(relative_to_base) = relative_to_base(&file.path, &request.base) else {
                continue;
            };
            if glob_matches(&pattern, relative_to_base) {
                matches.push(WorkspacePath::from_normalized(file.path.clone()));
            }
        }

        sort_paths_by_recent(&mut matches, &recent_ranks);
        Ok(WorkspaceGlobResult { matches })
    }

    async fn grep(&self, request: WorkspaceGrepRequest) -> Result<WorkspaceGrepResult> {
        Ok(self.grep_with_sources(request).await?.result)
    }

    async fn grep_with_sources(
        &self,
        request: WorkspaceGrepRequest,
    ) -> Result<WorkspaceGrepOutcome> {
        if let Some(ref glob) = request.glob {
            validate_relative_pattern(glob, "grep glob filter")?;
        }
        self.local.ensure_search_base_allowed(&request.base)?;
        let Some(search_snapshot) = self.manifest_ready() else {
            return self.fallback_search().grep_with_sources(request).await;
        };

        let regex_pattern = if request.case_insensitive {
            format!("(?i){}", request.pattern)
        } else {
            request.pattern.clone()
        };
        let regex = regex::Regex::new(&regex_pattern)
            .map_err(|e| anyhow!("Invalid regex pattern '{}': {}", request.pattern, e))?;
        let glob = request
            .glob
            .as_deref()
            .map(glob::Pattern::new)
            .transpose()
            .map_err(|e| anyhow!("Invalid grep glob filter: {e}"))?;

        let mut output = String::new();
        let mut match_count = 0;
        let mut file_count = 0;
        let mut total_size = 0;
        let mut matched_paths = Vec::new();
        let metadata_only = request.max_output_size == 0;

        let candidates = request
            .glob
            .as_deref()
            .map(|glob| candidate_indices_for_glob(&search_snapshot.index, &request.base, glob))
            .unwrap_or_else(|| CandidateIndices::Indexed(&search_snapshot.index.all));
        let recent_ranks = self.recent_path_ranks(&search_snapshot.index);

        for file_index in
            recent_first_candidate_indices(&candidates, &search_snapshot.index, &recent_ranks)
        {
            let Some(file) = search_snapshot.snapshot.files.get(file_index) else {
                continue;
            };
            if file.binary {
                continue;
            }
            let Some(relative_to_base) = relative_to_base(&file.path, &request.base) else {
                continue;
            };
            if let Some(glob) = &glob {
                if !glob_matches(glob, relative_to_base) {
                    continue;
                }
            }

            let workspace_path = WorkspacePath::from_normalized(file.path.clone());
            let Some(content) = self.local.read_search_file(&workspace_path) else {
                continue;
            };
            let lines: Vec<&str> = content.lines().collect();
            let file_matches = lines
                .iter()
                .enumerate()
                .filter_map(|(line_idx, line)| regex.is_match(line).then_some(line_idx))
                .collect::<Vec<_>>();

            if file_matches.is_empty() {
                continue;
            }

            file_count += 1;
            let display_path = escape_control_chars_for_display(&file.path);
            let mut path_recorded = false;
            for &match_idx in &file_matches {
                if !metadata_only && total_size > request.max_output_size {
                    return Ok(WorkspaceGrepOutcome {
                        result: WorkspaceGrepResult {
                            output,
                            match_count,
                            file_count,
                            truncated: true,
                        },
                        matched_paths: Some(matched_paths),
                    });
                }

                if !path_recorded {
                    matched_paths.push(workspace_path.clone());
                    path_recorded = true;
                }
                match_count += 1;
                if metadata_only {
                    continue;
                }
                let start = match_idx.saturating_sub(request.context_lines);
                let end = (match_idx + request.context_lines + 1).min(lines.len());

                for (i, line) in lines[start..end].iter().enumerate() {
                    let abs_i = start + i;
                    let prefix = if abs_i == match_idx { ">" } else { " " };
                    let line = format!("{}{}:{}: {}\n", prefix, display_path, abs_i + 1, line);
                    total_size += line.len();
                    output.push_str(&line);
                }

                if request.context_lines > 0 {
                    output.push_str("--\n");
                    total_size += 3;
                }
            }
        }

        Ok(WorkspaceGrepOutcome {
            result: WorkspaceGrepResult {
                output,
                match_count,
                file_count,
                truncated: false,
            },
            matched_paths: Some(matched_paths),
        })
    }
}

#[async_trait]
impl WorkspaceGit for ManifestWorkspaceBackend {
    async fn is_repository(&self) -> Result<bool> {
        self.local.is_repository().await
    }

    async fn status(&self) -> Result<WorkspaceGitStatus> {
        self.local.status().await
    }

    async fn log(&self, max_count: usize) -> Result<Vec<WorkspaceGitCommit>> {
        self.local.log(max_count).await
    }

    async fn list_branches(&self) -> Result<Vec<WorkspaceGitBranch>> {
        self.local.list_branches().await
    }

    async fn create_branch(&self, request: WorkspaceGitCreateBranchRequest) -> Result<()> {
        self.local.create_branch(request).await
    }

    async fn checkout(
        &self,
        request: WorkspaceGitCheckoutRequest,
    ) -> Result<WorkspaceGitCheckoutOutput> {
        self.local.checkout(request).await
    }

    async fn diff(&self, request: WorkspaceGitDiffRequest) -> Result<String> {
        self.local.diff(request).await
    }

    async fn list_remotes(&self) -> Result<Vec<WorkspaceGitRemote>> {
        self.local.list_remotes().await
    }
}

#[async_trait]
impl WorkspaceGitStashProvider for ManifestWorkspaceBackend {
    async fn list_stashes(&self) -> Result<Vec<WorkspaceGitStash>> {
        self.local.list_stashes().await
    }

    async fn stash(&self, request: WorkspaceGitStashRequest) -> Result<()> {
        self.local.stash(request).await
    }
}

#[async_trait]
impl WorkspaceGitWorktreeProvider for ManifestWorkspaceBackend {
    async fn list_worktrees(&self) -> Result<Vec<WorkspaceGitWorktree>> {
        self.local.list_worktrees().await
    }

    async fn create_worktree(
        &self,
        request: WorkspaceGitCreateWorktreeRequest,
    ) -> Result<WorkspaceGitWorktreeMutation> {
        self.local.create_worktree(request).await
    }

    async fn remove_worktree(
        &self,
        request: WorkspaceGitRemoveWorktreeRequest,
    ) -> Result<WorkspaceGitWorktreeMutation> {
        self.local.remove_worktree(request).await
    }
}

fn update_state(
    state: &Arc<RwLock<ManifestState>>,
    files: Vec<LocalWorkspaceFile>,
) -> Option<LocalWorkspaceManifestSnapshot> {
    let fingerprint = fingerprint_files(&files);
    let index = Arc::new(ManifestIndex::build(&files));
    let Ok(mut state) = state.write() else {
        return None;
    };
    if state.snapshot.version > 0 && state.fingerprint == fingerprint {
        return None;
    }
    state.fingerprint = fingerprint;
    state.index = index;
    state.snapshot = Arc::new(LocalWorkspaceManifestSnapshot {
        version: state.snapshot.version + 1,
        root: state.snapshot.root.clone(),
        files,
        scanned_at_ms: now_ms(),
    });
    Some((*state.snapshot).clone())
}

enum CandidateIndices<'a> {
    Indexed(&'a [usize]),
    Single(Option<usize>),
}

impl<'a> CandidateIndices<'a> {
    fn iter(&self) -> Box<dyn Iterator<Item = usize> + '_> {
        match self {
            Self::Indexed(indices) => Box::new(indices.iter().copied()),
            Self::Single(Some(index)) => Box::new(std::iter::once(*index)),
            Self::Single(None) => Box::new(std::iter::empty()),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Indexed(indices) => indices.len(),
            Self::Single(Some(_)) => 1,
            Self::Single(None) => 0,
        }
    }

    fn contains(&self, index: usize) -> bool {
        match self {
            Self::Indexed(indices) => indices.contains(&index),
            Self::Single(Some(candidate)) => *candidate == index,
            Self::Single(None) => false,
        }
    }
}

fn recent_first_candidate_indices(
    candidates: &CandidateIndices<'_>,
    index: &ManifestIndex,
    recent_ranks: &HashMap<String, usize>,
) -> Vec<usize> {
    if recent_ranks.is_empty() {
        return candidates.iter().collect();
    }

    let mut hot = recent_ranks
        .iter()
        .filter_map(|(path, rank)| {
            let file_index = *index.by_path.get(path)?;
            candidates
                .contains(file_index)
                .then_some((*rank, file_index))
        })
        .collect::<Vec<_>>();
    hot.sort_unstable_by_key(|(rank, _)| *rank);

    let mut out = Vec::with_capacity(candidates.len());
    let mut seen = HashSet::with_capacity(hot.len());
    for (_, file_index) in hot {
        if seen.insert(file_index) {
            out.push(file_index);
        }
    }
    out.extend(
        candidates
            .iter()
            .filter(|file_index| !seen.contains(file_index)),
    );
    out
}

fn sort_paths_by_recent(paths: &mut [WorkspacePath], recent_ranks: &HashMap<String, usize>) {
    paths.sort_by(|left, right| {
        recent_ranks
            .get(left.as_str())
            .copied()
            .unwrap_or(usize::MAX)
            .cmp(
                &recent_ranks
                    .get(right.as_str())
                    .copied()
                    .unwrap_or(usize::MAX),
            )
            .then_with(|| left.as_str().cmp(right.as_str()))
    });
}

fn candidate_indices_for_glob<'a>(
    index: &'a ManifestIndex,
    base: &WorkspacePath,
    pattern: &str,
) -> CandidateIndices<'a> {
    if !has_glob_meta(pattern) && pattern.contains('/') {
        return CandidateIndices::Single(
            literal_workspace_path(base, pattern)
                .and_then(|path| index.by_path.get(&path).copied()),
        );
    }

    if let Some(name) = literal_terminal_segment(pattern) {
        return index
            .by_basename
            .get(name)
            .map(|indices| CandidateIndices::Indexed(indices))
            .unwrap_or(CandidateIndices::Single(None));
    }

    if let Some(extension) = simple_extension_terminal(pattern) {
        return index
            .by_extension
            .get(extension)
            .map(|indices| CandidateIndices::Indexed(indices))
            .unwrap_or(CandidateIndices::Single(None));
    }

    CandidateIndices::Indexed(&index.all)
}

fn literal_workspace_path(base: &WorkspacePath, pattern: &str) -> Option<String> {
    let pattern = normalize_relative_path_lossy(Path::new(pattern))?;
    if pattern.is_empty() {
        return None;
    }
    if base.is_root() {
        Some(pattern)
    } else {
        Some(format!(
            "{}/{}",
            base.as_str().trim_end_matches('/'),
            pattern
        ))
    }
}

fn literal_terminal_segment(pattern: &str) -> Option<&str> {
    let terminal = pattern
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty())?;
    (!has_glob_meta(terminal)).then_some(terminal)
}

fn simple_extension_terminal(pattern: &str) -> Option<&str> {
    let terminal = pattern.trim_end_matches('/').rsplit('/').next()?;
    let extension = terminal.strip_prefix("*.")?;
    (!extension.is_empty() && !has_glob_meta(extension)).then_some(extension)
}

fn has_glob_meta(pattern: &str) -> bool {
    pattern
        .bytes()
        .any(|byte| matches!(byte, b'*' | b'?' | b'[' | b']' | b'{' | b'}'))
}

fn fingerprint_files(files: &[LocalWorkspaceFile]) -> u64 {
    let mut hasher = DefaultHasher::new();
    files.hash(&mut hasher);
    hasher.finish()
}

fn recent_score(entry: &RecentFileState, now: u64) -> f32 {
    let age_ms = now.saturating_sub(entry.touched_at_ms) as f32;
    let recency = (-age_ms / RECENT_DECAY_HALF_LIFE_MS).exp();
    let frequency =
        ((entry.touch_count as f32) + 1.0).ln() / (RECENT_FREQUENCY_NORMALIZER + 1.0).ln();
    RECENT_RECENCY_WEIGHT * recency + (1.0 - RECENT_RECENCY_WEIGHT) * frequency.min(1.0)
}

fn normalize_recent_file_path(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    let normalized = normalize_relative_path_lossy(Path::new(path))?;
    (!normalized.is_empty()).then_some(normalized)
}

fn normalize_relative_path_lossy(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

fn relative_to_base<'a>(path: &'a str, base: &WorkspacePath) -> Option<&'a str> {
    if base.is_root() {
        return Some(path);
    }
    let base = base.as_str().trim_end_matches('/');
    if path == base {
        Some("")
    } else {
        path.strip_prefix(base)
            .and_then(|tail| tail.strip_prefix('/'))
            .filter(|tail| !tail.is_empty())
    }
}

fn glob_matches(pattern: &glob::Pattern, path: &str) -> bool {
    let path = Path::new(path);
    pattern.matches_path(path)
        || path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| pattern.matches(name))
}

fn now_ms() -> u64 {
    system_time_ms(SystemTime::now())
}

fn system_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "manifest/tests.rs"]
mod tests;
