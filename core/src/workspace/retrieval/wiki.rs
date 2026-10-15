//! Saved wiki pages, indexed by the in-process a3s-vec lexical engine.
//!
//! The catalog admits markdown pages and keeps chunk identity. a3s-vec indexes
//! only those admitted chunks. Publication runs off the caller: a search reads
//! the last published generation, or returns [`WikiSearchPhase::NotReady`]
//! when that generation is being replaced, and never waits for the build.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use notify::Watcher;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::catalog::WorkspaceChunkCatalog;
use super::lexical::LexicalSearchRequest;
use super::reconcile::WorkspaceCatalogReconciler;
use super::types::{
    WorkspaceChunk, WorkspaceIndexError, WorkspaceIndexResult, WorkspaceLexicalEngine,
};
use super::WorkspaceEligibilityPolicy;
use crate::workspace::{
    LocalWorkspaceBackend, LocalWorkspaceFile, LocalWorkspaceFileStatus,
    LocalWorkspaceManifestSnapshot, WorkspaceFileChange, WorkspaceFileChangeKind,
    WorkspaceFileSystem, WorkspacePath, WorkspacePersistentIndex,
};

const PASSAGE_CHARS: usize = 1_600;
const COALESCE: Duration = Duration::from_millis(50);
const POLL: Duration = Duration::from_millis(200);
const TITLE_CHARS: usize = 80;

/// How many Unicode scalars a retrieved passage may contain.
pub const WIKI_PASSAGE_CHARS: usize = PASSAGE_CHARS;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WikiSearchPhase {
    /// No published generation is readable right now. The body is absent.
    NotReady,
    /// Hits come from one published a3s-vec generation.
    Published,
}

#[derive(Clone, Debug)]
pub struct WikiSearchHit {
    pub name: String,
    pub path: String,
    pub passage: String,
    pub score: f64,
    pub source_revision: u64,
}

#[derive(Clone, Debug)]
pub struct WikiSearchResult {
    pub phase: WikiSearchPhase,
    pub lexical_engine: &'static str,
    pub source_revision: u64,
    pub catalog_revision: u64,
    pub hits: Vec<WikiSearchHit>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WikiReconcileReport {
    pub source_revision: u64,
    pub read_paths: Vec<String>,
    pub removed_paths: Vec<String>,
    pub indexed_paths: Vec<String>,
}

struct RecordingFs {
    inner: LocalWorkspaceBackend,
    reads: Mutex<Vec<String>>,
}

#[async_trait]
impl WorkspaceFileSystem for RecordingFs {
    async fn read_text(&self, path: &WorkspacePath) -> crate::workspace::WorkspaceResult<String> {
        self.reads
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(path.as_str().to_owned());
        self.inner.read_text(path).await
    }

    async fn write_text(
        &self,
        path: &WorkspacePath,
        content: &str,
    ) -> crate::workspace::WorkspaceResult<crate::workspace::WorkspaceWriteOutcome> {
        self.inner.write_text(path, content).await
    }

    async fn list_dir(
        &self,
        path: &WorkspacePath,
    ) -> crate::workspace::WorkspaceResult<Vec<crate::workspace::WorkspaceDirEntry>> {
        self.inner.list_dir(path).await
    }
}

struct WikiState {
    root: PathBuf,
    catalog: Arc<WorkspaceChunkCatalog>,
    file_system: Arc<RecordingFs>,
    index: Arc<WorkspacePersistentIndex>,
    /// Held for the whole native query. The publisher takes the write side
    /// only around generation replacement, so a new search fails fast instead
    /// of waiting for that build.
    gate: RwLock<()>,
    hold: AtomicBool,
    syncing: AtomicBool,
    builds: AtomicU64,
    stop: AtomicBool,
    version: AtomicU64,
    fingerprint: Mutex<Option<u64>>,
    indexed_paths: Mutex<Vec<String>>,
    lifetime: CancellationToken,
}

struct ReconcileCommand {
    changed: Option<Vec<String>>,
    reply: Option<oneshot::Sender<WorkspaceIndexResult<WikiReconcileReport>>>,
}

/// Incremental a3s-vec index of one saved-wiki directory.
pub struct WikiIndex {
    state: Arc<WikiState>,
    /// RAII keeper for the backing temp dir; never read on purpose.
    #[allow(dead_code)]
    storage: Option<tempfile::TempDir>,
    command_tx: Option<mpsc::UnboundedSender<ReconcileCommand>>,
    command_task: Option<tokio::task::JoinHandle<()>>,
    publisher: Option<std::thread::JoinHandle<()>>,
    watcher: Option<std::thread::JoinHandle<()>>,
}

impl WikiIndex {
    /// Start background admission and publication. Returns before the first
    /// generation is built. Later page changes schedule their own build.
    pub fn start(root: impl Into<PathBuf>) -> WorkspaceIndexResult<Self> {
        Self::boot(root, true)
    }

    /// Admit and publish only when [`Self::reconcile_now`] is called.
    pub fn open(root: impl Into<PathBuf>) -> WorkspaceIndexResult<Self> {
        Self::boot(root, false)
    }

    fn boot(root: impl Into<PathBuf>, auto: bool) -> WorkspaceIndexResult<Self> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
            WorkspaceIndexError::InvalidConfig(
                "wiki index scheduling requires a running async runtime".to_owned(),
            )
        })?;
        let root = root.into();
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        let storage = tempfile::tempdir().map_err(|error| WorkspaceIndexError::ReadFailed {
            path: root.display().to_string(),
            message: error.to_string(),
        })?;
        let index = WorkspacePersistentIndex::open(
            storage.path().join("wiki-index"),
            WorkspaceLexicalEngine::A3sVec,
        )?;
        let file_system = Arc::new(RecordingFs {
            inner: LocalWorkspaceBackend::new_with_source_egress_policy(root.clone()),
            reads: Mutex::new(Vec::new()),
        });
        let catalog = WorkspaceChunkCatalog::default_catalog_with_engines(
            WorkspaceLexicalEngine::A3sVec,
            WorkspaceLexicalEngine::Portable,
        );
        let state = Arc::new(WikiState {
            root: root.clone(),
            catalog,
            file_system,
            index,
            gate: RwLock::new(()),
            hold: AtomicBool::new(false),
            syncing: AtomicBool::new(false),
            builds: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            version: AtomicU64::new(0),
            fingerprint: Mutex::new(None),
            indexed_paths: Mutex::new(Vec::new()),
            lifetime: CancellationToken::new(),
        });
        let (publish_tx, publish_rx) = std::sync::mpsc::channel();
        let publisher_state = Arc::clone(&state);
        let publisher = std::thread::Builder::new()
            .name("a3s-wiki-publish".to_owned())
            .spawn(move || publish_loop(publisher_state, publish_rx))
            .map_err(|error| WorkspaceIndexError::ReadFailed {
                path: root.display().to_string(),
                message: error.to_string(),
            })?;
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let command_state = Arc::clone(&state);
        let command_tx_for_task = command_tx.clone();
        let command_task = runtime.spawn(async move {
            command_loop(
                command_state,
                command_rx,
                publish_tx,
                command_tx_for_task,
                auto,
            )
            .await;
        });
        let watcher = auto
            .then(|| spawn_watcher(root, command_tx.clone(), Arc::clone(&state)))
            .flatten();
        let index = Self {
            state,
            storage: Some(storage),
            command_tx: Some(command_tx),
            command_task: Some(command_task),
            publisher: Some(publisher),
            watcher,
        };
        if auto {
            index.enqueue(None, None);
        }
        Ok(index)
    }

    /// Reconcile the wiki directory and schedule one publication.
    ///
    /// The returned report is ready when admission finishes. Publication keeps
    /// running after this returns. `changed` forces those paths to be reread.
    pub async fn reconcile_now(
        &self,
        changed: Option<Vec<String>>,
    ) -> WorkspaceIndexResult<WikiReconcileReport> {
        let (tx, rx) = oneshot::channel();
        self.enqueue(changed, Some(tx));
        match rx.await {
            Ok(result) => result,
            Err(_) => Err(WorkspaceIndexError::InvalidConfig(
                "wiki index stopped during reconciliation".to_owned(),
            )),
        }
    }

    /// Search the published a3s-vec generation. A generation swap returns
    /// [`WikiSearchPhase::NotReady`] without waiting and without unpublished text.
    pub fn search(&self, query: impl Into<String>) -> WorkspaceIndexResult<WikiSearchResult> {
        let engine = WorkspaceLexicalEngine::A3sVec.stable_id();
        let idle = match self.state.gate.try_read() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::WouldBlock) => return Ok(not_ready(engine)),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(WorkspaceIndexError::LockPoisoned)
            }
        };
        if self.state.syncing.load(Ordering::Acquire) || !self.state.index.is_ready() {
            drop(idle);
            return Ok(not_ready(engine));
        }
        let result = self
            .state
            .index
            .search(&LexicalSearchRequest::new(query.into()))?;
        drop(idle);
        let mut hits = Vec::with_capacity(result.hits.len());
        for hit in result.hits {
            hits.push(WikiSearchHit {
                name: page_name(&hit.chunk),
                path: hit.chunk.path.to_string(),
                passage: bounded_passage(hit.chunk.text.as_ref(), &result.query_terms),
                score: hit.score,
                source_revision: hit.chunk.source_revision,
            });
        }
        Ok(WikiSearchResult {
            phase: WikiSearchPhase::Published,
            lexical_engine: result.lexical_engine.stable_id(),
            source_revision: result.source_revision,
            catalog_revision: result.catalog_revision,
            hits,
        })
    }

    pub fn root(&self) -> std::path::PathBuf {
        self.state.root.clone()
    }

    pub fn content_reads(&self) -> Vec<String> {
        self.state
            .file_system
            .reads
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    pub fn builds(&self) -> u64 {
        self.state.builds.load(Ordering::Acquire)
    }

    /// Keep the newest admitted snapshot queued and do not publish it yet.
    pub fn hold_publication(&self, hold: bool) {
        self.state.hold.store(hold, Ordering::Release);
    }

    pub async fn wait_published(&self, source_revision: u64) -> bool {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        loop {
            let status = self.state.index.status();
            if status.phase == super::persistent::WorkspacePersistentIndexPhase::Ready
                && status.source_revision >= source_revision
                && !self.state.syncing.load(Ordering::Acquire)
                && !self.state.hold.load(Ordering::Acquire)
                && self.state.builds.load(Ordering::Acquire) > 0
            {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }

    fn enqueue(
        &self,
        changed: Option<Vec<String>>,
        reply: Option<oneshot::Sender<WorkspaceIndexResult<WikiReconcileReport>>>,
    ) {
        let Some(tx) = &self.command_tx else {
            if let Some(reply) = reply {
                let _ = reply.send(Err(WorkspaceIndexError::InvalidConfig(
                    "wiki index is stopped".to_owned(),
                )));
            }
            return;
        };
        if tx.send(ReconcileCommand { changed, reply }).is_err() {
            // The command task has already stopped. The caller sees a closed reply.
        }
    }

    fn shutdown(&mut self) {
        self.state.stop.store(true, Ordering::Release);
        self.state.lifetime.cancel();
        self.command_tx.take();
        if let Some(task) = self.command_task.take() {
            // The handle belongs to the caller's runtime. Awaiting it on a
            // fresh runtime never observes cancellation and blocks Drop.
            task.abort();
        }
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
        if let Some(publisher) = self.publisher.take() {
            let _ = publisher.join();
        }
    }
}

impl Drop for WikiIndex {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn not_ready(engine: &'static str) -> WikiSearchResult {
    WikiSearchResult {
        phase: WikiSearchPhase::NotReady,
        lexical_engine: engine,
        source_revision: 0,
        catalog_revision: 0,
        hits: Vec::new(),
    }
}

async fn command_loop(
    state: Arc<WikiState>,
    mut commands: mpsc::UnboundedReceiver<ReconcileCommand>,
    publish_tx: std::sync::mpsc::Sender<super::catalog::ChunkCatalogSnapshot>,
    command_tx: mpsc::UnboundedSender<ReconcileCommand>,
    auto: bool,
) {
    let reconciler = WorkspaceCatalogReconciler::new(
        Arc::clone(&state.catalog),
        WorkspaceEligibilityPolicy::default(),
        Arc::clone(&state.file_system) as Arc<dyn WorkspaceFileSystem>,
    );
    let start = tokio::time::Instant::now() + POLL;
    let mut poll = tokio::time::interval_at(start, POLL);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = state.lifetime.cancelled() => break,
            command = commands.recv() => {
                let Some(command) = command else { break };
                let result = reconcile_once(&state, &reconciler, command.changed, &publish_tx).await;
                if let Some(reply) = command.reply {
                    let _ = reply.send(result);
                }
            }
            _ = poll.tick(), if auto && !state.stop.load(Ordering::Acquire) => {
                let _ = command_tx.send(ReconcileCommand { changed: None, reply: None });
            }
        }
    }
}

async fn reconcile_once(
    state: &WikiState,
    reconciler: &WorkspaceCatalogReconciler,
    changed: Option<Vec<String>>,
    publish_tx: &std::sync::mpsc::Sender<super::catalog::ChunkCatalogSnapshot>,
) -> WorkspaceIndexResult<WikiReconcileReport> {
    let files = scan_wiki(&state.root);
    let next_fingerprint = fingerprint(&files);
    let unchanged = state
        .fingerprint
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .is_some_and(|current| current == next_fingerprint);
    if changed.is_none() && unchanged && state.index.is_ready() {
        return Ok(WikiReconcileReport {
            source_revision: state.index.status().source_revision,
            read_paths: Vec::new(),
            removed_paths: Vec::new(),
            indexed_paths: state
                .indexed_paths
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone(),
        });
    }
    let version = state.version.fetch_add(1, Ordering::AcqRel) + 1;
    let snapshot = LocalWorkspaceManifestSnapshot {
        version,
        root: state.root.clone(),
        files,
        scanned_at_ms: system_millis(),
    };
    let admitted = if let Some(changed) = changed {
        let changes = changed
            .iter()
            .map(|path| WorkspaceFileChange {
                path: WorkspacePath::from_normalized(path),
                kind: WorkspaceFileChangeKind::Changed,
            })
            .collect::<Vec<_>>();
        reconciler.reconcile_changes(&snapshot, &changes).await?
    } else {
        reconciler.reconcile_snapshot(&snapshot).await?
    };
    let published = reconciler.catalog_snapshot()?;
    let indexed_paths = published.paths();
    *state
        .indexed_paths
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = indexed_paths.clone();
    *state
        .fingerprint
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = Some(next_fingerprint);
    if !admitted.read_paths.is_empty()
        || !admitted.removed_paths.is_empty()
        || !state.index.is_ready()
    {
        let _ = publish_tx.send(published);
    }
    Ok(WikiReconcileReport {
        source_revision: admitted.source_revision,
        read_paths: admitted.read_paths,
        removed_paths: admitted.removed_paths,
        indexed_paths,
    })
}

fn publish_loop(
    state: Arc<WikiState>,
    rx: std::sync::mpsc::Receiver<super::catalog::ChunkCatalogSnapshot>,
) {
    loop {
        if state.stop.load(Ordering::Acquire) {
            return;
        }
        let mut pending = match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(snapshot) => snapshot,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let settle = std::time::Instant::now() + COALESCE;
        loop {
            if state.stop.load(Ordering::Acquire) {
                return;
            }
            let rest = settle.saturating_duration_since(std::time::Instant::now());
            if rest.is_zero() && !state.hold.load(Ordering::Acquire) {
                break;
            }
            let wait = if state.hold.load(Ordering::Acquire) {
                Duration::from_millis(10)
            } else {
                rest
            };
            match rx.recv_timeout(wait) {
                Ok(snapshot) if is_newer(&snapshot, &pending) => pending = snapshot,
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        while let Ok(snapshot) = rx.try_recv() {
            if is_newer(&snapshot, &pending) {
                pending = snapshot;
            }
        }
        if state.stop.load(Ordering::Acquire) {
            return;
        }
        let guard = match state.gate.write() {
            Ok(guard) => guard,
            Err(poison) => poison.into_inner(),
        };
        state.syncing.store(true, Ordering::Release);
        let synced = state.index.sync_snapshot(&pending);
        state.syncing.store(false, Ordering::Release);
        drop(guard);
        match synced {
            Ok(()) => {
                state.builds.fetch_add(1, Ordering::AcqRel);
            }
            Err(error) => {
                tracing::warn!(%error, "wiki index publication failed");
            }
        }
    }
}

fn spawn_watcher(
    root: PathBuf,
    command_tx: mpsc::UnboundedSender<ReconcileCommand>,
    state: Arc<WikiState>,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("a3s-wiki-watch".to_owned())
        .spawn(move || {
            let (tx, rx) = std::sync::mpsc::channel();
            let mut watcher =
                notify::recommended_watcher(move |event: Result<notify::Event, notify::Error>| {
                    if let Ok(event) = event {
                        let _ = tx.send(event);
                    }
                });
            if let Ok(watcher) = watcher.as_mut() {
                let _ = watcher.watch(&root, notify::RecursiveMode::Recursive);
            }
            while !state.stop.load(Ordering::Acquire) {
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(_) => {
                        let _ = command_tx.send(ReconcileCommand {
                            changed: None,
                            reply: None,
                        });
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .ok()
}

fn is_newer(
    candidate: &super::catalog::ChunkCatalogSnapshot,
    current: &super::catalog::ChunkCatalogSnapshot,
) -> bool {
    (candidate.source_revision(), candidate.revision())
        > (current.source_revision(), current.revision())
}

fn scan_wiki(root: &Path) -> Vec<LocalWorkspaceFile> {
    let policy = WorkspaceEligibilityPolicy::default();
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            let Some(rel) = path.strip_prefix(root).ok().and_then(|path| path.to_str()) else {
                continue;
            };
            let rel = rel.replace('\\', "/");
            if rel.is_empty()
                || rel
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
            {
                continue;
            }
            if Path::new(&rel).extension().and_then(|ext| ext.to_str()) != Some("md") {
                continue;
            }
            let modified_ms = metadata.modified().ok().and_then(|modified| {
                modified
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
            });
            let file = LocalWorkspaceFile {
                path: rel,
                size: metadata.len(),
                modified_ms,
                language: Some("markdown".to_owned()),
                status: LocalWorkspaceFileStatus::Untracked,
                binary: false,
                generated: false,
            };
            if policy.admits(&file) {
                files.push(file);
            }
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files
}

fn fingerprint(files: &[LocalWorkspaceFile]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for file in files {
        file.path.hash(&mut hasher);
        file.size.hash(&mut hasher);
        file.modified_ms.hash(&mut hasher);
    }
    hasher.finish()
}

fn system_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn page_name(chunk: &WorkspaceChunk) -> String {
    wiki_title(chunk.text.as_ref()).unwrap_or_else(|| {
        Path::new(chunk.path.as_ref())
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.is_empty())
            .unwrap_or(chunk.path.as_ref())
            .to_owned()
    })
}

fn wiki_title(text: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return None;
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() != "title" {
            continue;
        }
        let value = unquote(value.trim());
        if value.is_empty() || value.chars().count() > TITLE_CHARS {
            return None;
        }
        return Some(value.to_owned());
    }
    None
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
        })
        .unwrap_or(value)
}

fn bounded_passage(text: &str, terms: &[String]) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= PASSAGE_CHARS {
        return chars.into_iter().collect();
    }
    let start = terms
        .iter()
        .filter_map(|term| find_chars(&chars, &term.chars().collect::<Vec<_>>()))
        .min()
        .unwrap_or(0);
    let start = start.min(chars.len().saturating_sub(PASSAGE_CHARS));
    let end = (start + PASSAGE_CHARS).min(chars.len());
    chars[start..end].iter().collect()
}

fn find_chars(haystack: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
