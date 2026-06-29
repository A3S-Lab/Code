//! Manifest-backed local workspace services.
//!
//! The manifest is an in-memory index of workspace files. It is built
//! asynchronously, refreshed from filesystem notifications, and used by the
//! local search backend (`glob`/`grep`) to avoid walking the filesystem for
//! every agent tool call. File I/O, command execution, and git operations still
//! delegate to [`LocalWorkspaceBackend`].

use super::{
    validate_relative_pattern, CommandOutput, CommandRequest, LocalWorkspaceBackend,
    WorkspaceCommandRunner, WorkspaceDirEntry, WorkspaceFileSystem, WorkspaceGit,
    WorkspaceGitBranch, WorkspaceGitCheckoutOutput, WorkspaceGitCheckoutRequest,
    WorkspaceGitCommit, WorkspaceGitCreateBranchRequest, WorkspaceGitCreateWorktreeRequest,
    WorkspaceGitDiffRequest, WorkspaceGitRemote, WorkspaceGitRemoveWorktreeRequest,
    WorkspaceGitStash, WorkspaceGitStashProvider, WorkspaceGitStashRequest, WorkspaceGitStatus,
    WorkspaceGitWorktree, WorkspaceGitWorktreeMutation, WorkspaceGitWorktreeProvider,
    WorkspaceGlobRequest, WorkspaceGlobResult, WorkspaceGrepRequest, WorkspaceGrepResult,
    WorkspacePath, WorkspacePathResolver, WorkspaceResult, WorkspaceSearch, WorkspaceWriteOutcome,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use ignore::WalkBuilder;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc};

const WATCH_DEBOUNCE: Duration = Duration::from_millis(150);
const SNAPSHOT_CHANNEL_CAPACITY: usize = 16;
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
    task: tokio::task::JoinHandle<()>,
}

impl LocalWorkspaceManifest {
    /// Start the manifest scanner/watcher for `root`.
    pub fn start(root: impl Into<PathBuf>) -> Arc<Self> {
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
        let task_state = Arc::clone(&state);
        let task_snapshots = snapshots.clone();
        let task = tokio::spawn(async move {
            run_manifest_task(root, task_state, task_snapshots).await;
        });
        Arc::new(Self {
            state,
            recent,
            snapshots,
            task,
        })
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
        self.task.abort();
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
    manifest: Arc<LocalWorkspaceManifest>,
}

impl ManifestWorkspaceBackend {
    pub fn new(root: impl Into<PathBuf>) -> Arc<Self> {
        let local = Arc::new(LocalWorkspaceBackend::new(root.into()));
        let manifest = LocalWorkspaceManifest::start(local.root.clone());
        Self::from_manifest(local, manifest)
    }

    pub fn from_manifest(
        local: Arc<LocalWorkspaceBackend>,
        manifest: Arc<LocalWorkspaceManifest>,
    ) -> Arc<Self> {
        Arc::new(Self { local, manifest })
    }

    pub fn manifest(&self) -> Arc<LocalWorkspaceManifest> {
        Arc::clone(&self.manifest)
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
        if let Some(ref glob) = request.glob {
            validate_relative_pattern(glob, "grep glob filter")?;
        }
        let Some(search_snapshot) = self.manifest_ready() else {
            return self.fallback_search().grep(request).await;
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

            let full_path = search_snapshot.snapshot.root.join(&file.path);
            let content = match std::fs::read_to_string(&full_path) {
                Ok(content) => content,
                Err(_) => continue,
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
            for &match_idx in &file_matches {
                if total_size > request.max_output_size {
                    return Ok(WorkspaceGrepResult {
                        output,
                        match_count,
                        file_count,
                        truncated: true,
                    });
                }

                match_count += 1;
                let start = match_idx.saturating_sub(request.context_lines);
                let end = (match_idx + request.context_lines + 1).min(lines.len());

                for (i, line) in lines[start..end].iter().enumerate() {
                    let abs_i = start + i;
                    let prefix = if abs_i == match_idx { ">" } else { " " };
                    let line = format!("{}{}:{}: {}\n", prefix, file.path, abs_i + 1, line);
                    total_size += line.len();
                    output.push_str(&line);
                }

                if request.context_lines > 0 {
                    output.push_str("--\n");
                    total_size += 3;
                }
            }
        }

        Ok(WorkspaceGrepResult {
            output,
            match_count,
            file_count,
            truncated: false,
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

async fn run_manifest_task(
    root: PathBuf,
    state: Arc<RwLock<ManifestState>>,
    snapshots: broadcast::Sender<LocalWorkspaceManifestSnapshot>,
) {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let watcher = RecommendedWatcher::new(
        move |event| {
            let _ = event_tx.send(event);
        },
        Config::default(),
    )
    .and_then(|mut watcher| {
        watcher.watch(&root, RecursiveMode::Recursive)?;
        Ok(watcher)
    });

    publish_scan(&root, &state, &snapshots).await;

    let Ok(_watcher) = watcher else {
        return;
    };

    while let Some(event) = event_rx.recv().await {
        let Ok(event) = event else {
            continue;
        };
        if !is_relevant_event(&event, &root) {
            continue;
        }
        tokio::time::sleep(WATCH_DEBOUNCE).await;
        while let Ok(event) = event_rx.try_recv() {
            if let Ok(event) = event {
                if !is_relevant_event(&event, &root) {
                    continue;
                }
            }
        }
        publish_scan(&root, &state, &snapshots).await;
    }
}

async fn publish_scan(
    root: &Path,
    state: &Arc<RwLock<ManifestState>>,
    snapshots: &broadcast::Sender<LocalWorkspaceManifestSnapshot>,
) {
    let root = root.to_path_buf();
    let Ok(files) = tokio::task::spawn_blocking(move || scan_workspace_files(&root)).await else {
        return;
    };
    let Some(snapshot) = update_state(state, files) else {
        return;
    };
    let _ = snapshots.send(snapshot);
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

pub fn scan_workspace_files(root: &Path) -> Vec<LocalWorkspaceFile> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut files = scan_with_ignore(&root);
    if let Some(paths) = git_workspace_paths(&root) {
        apply_git_statuses(&root, &mut files, paths);
    }
    sorted_dedup(files)
}

fn git_workspace_paths(root: &Path) -> Option<Vec<(PathBuf, LocalWorkspaceFileStatus)>> {
    let mut out = Vec::new();
    let tracked = git_ls_files(
        root,
        &["ls-files", "--cached", "--recurse-submodules", "-z"],
    )
    .or_else(|| git_ls_files(root, &["ls-files", "--cached", "-z"]))?;
    out.extend(
        tracked
            .into_iter()
            .map(|path| (path, LocalWorkspaceFileStatus::Tracked)),
    );
    let untracked = git_ls_files(root, &["ls-files", "--others", "--exclude-standard", "-z"])
        .unwrap_or_default();
    out.extend(
        untracked
            .into_iter()
            .map(|path| (path, LocalWorkspaceFileStatus::Untracked)),
    );
    Some(out)
}

fn git_ls_files(root: &Path, args: &[&str]) -> Option<Vec<PathBuf>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|raw| !raw.is_empty())
            .map(|raw| PathBuf::from(String::from_utf8_lossy(raw).into_owned()))
            .collect(),
    )
}

fn apply_git_statuses(
    root: &Path,
    files: &mut Vec<LocalWorkspaceFile>,
    paths: Vec<(PathBuf, LocalWorkspaceFileStatus)>,
) {
    let mut statuses = HashMap::<String, LocalWorkspaceFileStatus>::new();
    for (relative, status) in paths {
        if path_has_noise_component(&relative) {
            continue;
        }
        let Some(relative) = normalize_relative_path_lossy(&relative) else {
            continue;
        };
        statuses
            .entry(relative)
            .and_modify(|existing| *existing = preferred_status(*existing, status))
            .or_insert(status);
    }

    for file in files.iter_mut() {
        if let Some(status) = statuses.remove(&file.path) {
            file.status = status;
        }
    }

    for (relative, status) in statuses {
        if let Some(file) = workspace_file(root, Path::new(&relative), status) {
            files.push(file);
        }
    }
}

fn preferred_status(
    existing: LocalWorkspaceFileStatus,
    incoming: LocalWorkspaceFileStatus,
) -> LocalWorkspaceFileStatus {
    match (existing, incoming) {
        (LocalWorkspaceFileStatus::Tracked, _) | (_, LocalWorkspaceFileStatus::Tracked) => {
            LocalWorkspaceFileStatus::Tracked
        }
        (LocalWorkspaceFileStatus::Untracked, _) | (_, LocalWorkspaceFileStatus::Untracked) => {
            LocalWorkspaceFileStatus::Untracked
        }
        _ => LocalWorkspaceFileStatus::Unknown,
    }
}

fn scan_with_ignore(root: &Path) -> Vec<LocalWorkspaceFile> {
    let filter_root = root.to_path_buf();
    WalkBuilder::new(root)
        .hidden(false)
        .parents(true)
        .ignore(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .filter_entry(move |entry| {
            entry
                .path()
                .strip_prefix(&filter_root)
                .map(|relative| !path_has_noise_component(relative))
                .unwrap_or(true)
        })
        .build()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path == root {
                return None;
            }
            let relative = path.strip_prefix(root).ok()?;
            workspace_file(root, relative, LocalWorkspaceFileStatus::Unknown)
        })
        .collect()
}

fn workspace_file(
    root: &Path,
    relative: &Path,
    status: LocalWorkspaceFileStatus,
) -> Option<LocalWorkspaceFile> {
    let relative = normalize_relative_path_lossy(relative)?;
    if relative.is_empty() {
        return None;
    }
    let full_path = root.join(&relative);
    let metadata = std::fs::metadata(&full_path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    Some(LocalWorkspaceFile {
        language: language_for_path(Path::new(&relative)).map(str::to_string),
        binary: is_binary_file(&full_path, metadata.len()),
        generated: is_generated_path(Path::new(&relative)),
        modified_ms: metadata.modified().ok().map(system_time_ms),
        size: metadata.len(),
        path: relative,
        status,
    })
}

fn sorted_dedup(files: Vec<LocalWorkspaceFile>) -> Vec<LocalWorkspaceFile> {
    let mut by_path = HashMap::<String, LocalWorkspaceFile>::new();
    for file in files {
        by_path
            .entry(file.path.clone())
            .and_modify(|existing| {
                if existing.status == LocalWorkspaceFileStatus::Unknown
                    && file.status != LocalWorkspaceFileStatus::Unknown
                {
                    *existing = file.clone();
                }
            })
            .or_insert(file);
    }
    let mut files = by_path.into_values().collect::<Vec<_>>();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
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

fn path_has_noise_component(path: &Path) -> bool {
    path.components().any(|component| {
        let Component::Normal(name) = component else {
            return false;
        };
        matches!(
            name.to_string_lossy().as_ref(),
            ".git" | "node_modules" | "target" | ".next" | "dist" | ".DS_Store"
        )
    })
}

fn is_generated_path(path: &Path) -> bool {
    path.components().any(|component| {
        let Component::Normal(name) = component else {
            return false;
        };
        matches!(
            name.to_string_lossy().as_ref(),
            "target" | "node_modules" | ".next" | "dist" | "build" | "coverage"
        )
    })
}

fn language_for_path(path: &Path) -> Option<&'static str> {
    match path.extension().and_then(|ext| ext.to_str())? {
        "rs" => Some("rust"),
        "toml" => Some("toml"),
        "hcl" => Some("hcl"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "jsx" => Some("javascript-react"),
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("typescript-react"),
        "json" => Some("json"),
        "md" | "mdx" => Some("markdown"),
        "py" => Some("python"),
        "go" => Some("go"),
        "java" => Some("java"),
        "kt" | "kts" => Some("kotlin"),
        "swift" => Some("swift"),
        "c" | "h" => Some("c"),
        "cc" | "cpp" | "cxx" | "hpp" => Some("cpp"),
        "cs" => Some("csharp"),
        "rb" => Some("ruby"),
        "php" => Some("php"),
        "sh" | "bash" | "zsh" => Some("shell"),
        "yml" | "yaml" => Some("yaml"),
        "html" | "htm" => Some("html"),
        "css" => Some("css"),
        "scss" | "sass" => Some("scss"),
        "sql" => Some("sql"),
        "xml" => Some("xml"),
        _ => None,
    }
}

fn is_binary_file(path: &Path, size: u64) -> bool {
    if matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "ico"
            | "pdf"
            | "zip"
            | "gz"
            | "tgz"
            | "xz"
            | "wasm"
            | "dylib"
            | "so"
            | "a"
            | "o"
            | "rlib"
            | "class"
            | "jar"
    ) {
        return true;
    }
    if is_known_text_path(path) {
        return false;
    }
    if size == 0 {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 2048];
    use std::io::Read;
    match file.read(&mut buf) {
        Ok(n) => buf[..n].contains(&0),
        Err(_) => false,
    }
}

fn is_known_text_path(path: &Path) -> bool {
    if language_for_path(path).is_some() {
        return true;
    }
    if matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "txt" | "lock" | "gitignore" | "dockerignore" | "env" | "example" | "sample"
    ) {
        return true;
    }
    matches!(
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "dockerfile" | "makefile" | "justfile" | "license" | "notice"
    )
}

fn is_relevant_event(event: &Event, root: &Path) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    event.paths.iter().any(|path| {
        path.strip_prefix(root)
            .map(|relative| !path_has_noise_component(relative))
            .unwrap_or(false)
    })
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
mod tests {
    use super::*;

    fn write(path: &Path, body: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    fn git_available() -> bool {
        Command::new("git").arg("--version").output().is_ok()
    }

    fn run_git(root: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "git command failed: git -C {root:?} {args:?}"
        );
    }

    fn test_file(path: &str) -> LocalWorkspaceFile {
        LocalWorkspaceFile {
            path: path.to_string(),
            size: 0,
            modified_ms: None,
            language: None,
            status: LocalWorkspaceFileStatus::Unknown,
            binary: false,
            generated: false,
        }
    }

    #[test]
    fn manifest_index_reduces_glob_candidates() {
        let files = vec![
            test_file("src/main.rs"),
            test_file("crates/demo/src/lib.rs"),
            test_file("README.md"),
            test_file("docs/README.md"),
        ];
        let index = ManifestIndex::build(&files);

        let exact = candidate_indices_for_glob(&index, &WorkspacePath::root(), "src/main.rs")
            .iter()
            .collect::<Vec<_>>();
        assert_eq!(exact, vec![0]);

        let basename = candidate_indices_for_glob(&index, &WorkspacePath::root(), "**/README.md")
            .iter()
            .collect::<Vec<_>>();
        assert_eq!(basename, vec![2, 3]);

        let extension = candidate_indices_for_glob(&index, &WorkspacePath::root(), "**/*.rs")
            .iter()
            .collect::<Vec<_>>();
        assert_eq!(extension, vec![0, 1]);
    }

    #[test]
    fn recent_files_are_bounded_and_ranked_by_heat() {
        let mut recent = RecentFiles::default();
        for index in 0..(RECENT_FILE_LIMIT + 5) {
            recent.touch(format!("src/file_{index:03}.rs"), index as u64);
        }
        recent.touch("src/file_000.rs".to_string(), 10_000);
        recent.touch("src/file_000.rs".to_string(), 10_001);

        let entries = recent.entries(None, usize::MAX, 10_001);

        assert_eq!(entries.len(), RECENT_FILE_LIMIT);
        assert_eq!(entries[0].path, "src/file_000.rs");
        assert_eq!(entries[0].touch_count, 2);
    }

    #[tokio::test]
    async fn manifest_search_matches_glob_and_grep() {
        let temp = tempfile::tempdir().unwrap();
        write(
            &temp.path().join("src/main.rs"),
            b"fn main() {\n    println!(\"hello\");\n}\n",
        );
        write(&temp.path().join("README.md"), b"hello from docs\n");

        let backend = ManifestWorkspaceBackend::new(temp.path());
        let mut rx = backend.manifest().subscribe();
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();

        let glob = backend
            .glob(WorkspaceGlobRequest {
                base: backend.normalize("src").unwrap(),
                pattern: "*.rs".to_string(),
            })
            .await
            .unwrap();
        assert_eq!(glob.matches[0].as_str(), "src/main.rs");

        let grep = backend
            .grep(WorkspaceGrepRequest {
                base: WorkspacePath::root(),
                pattern: "hello".to_string(),
                glob: Some("*.rs".to_string()),
                context_lines: 0,
                case_insensitive: false,
                max_output_size: 1024,
            })
            .await
            .unwrap();
        assert_eq!(grep.match_count, 1);
        assert_eq!(grep.file_count, 1);
        assert!(grep.output.contains("src/main.rs:2"));
    }

    #[tokio::test]
    async fn manifest_backend_read_write_touch_recent_files() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("src/main.rs"), b"fn main() {}\n");

        let backend = ManifestWorkspaceBackend::new(temp.path());
        let mut rx = backend.manifest().subscribe();
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();

        let path = backend.normalize("src/main.rs").unwrap();
        backend.read_text(&path).await.unwrap();
        assert_eq!(
            backend.manifest().recent_file_paths(4),
            vec!["src/main.rs".to_string()]
        );

        backend
            .write_text(&path, "fn main() { println!(\"hi\"); }\n")
            .await
            .unwrap();
        let entries = backend.manifest().recent_file_entries(4);
        assert_eq!(entries[0].path, "src/main.rs");
        assert_eq!(entries[0].touch_count, 2);
    }

    #[tokio::test]
    async fn manifest_glob_prioritizes_recent_matching_files() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("src/a.rs"), b"pub fn a() {}\n");
        write(&temp.path().join("src/z.rs"), b"pub fn z() {}\n");

        let backend = ManifestWorkspaceBackend::new(temp.path());
        let mut rx = backend.manifest().subscribe();
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();

        backend.manifest().touch_file("src/z.rs");
        let glob = backend
            .glob(WorkspaceGlobRequest {
                base: WorkspacePath::root(),
                pattern: "**/*.rs".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(glob.matches[0].as_str(), "src/z.rs");
        assert_eq!(glob.matches[1].as_str(), "src/a.rs");
    }

    #[tokio::test]
    async fn manifest_grep_prioritizes_recent_matches_when_truncated() {
        let temp = tempfile::tempdir().unwrap();
        write(
            &temp.path().join("src/a.rs"),
            b"pub const HIT: &str = \"a\";\n",
        );
        write(
            &temp.path().join("src/z.rs"),
            b"pub const HIT: &str = \"z\";\n",
        );

        let backend = ManifestWorkspaceBackend::new(temp.path());
        let mut rx = backend.manifest().subscribe();
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();

        backend.manifest().touch_file("src/z.rs");
        let grep = backend
            .grep(WorkspaceGrepRequest {
                base: WorkspacePath::root(),
                pattern: "HIT".to_string(),
                glob: Some("**/*.rs".to_string()),
                context_lines: 0,
                case_insensitive: false,
                max_output_size: 1,
            })
            .await
            .unwrap();

        assert!(grep.truncated);
        assert!(grep.output.starts_with(">src/z.rs:"), "{}", grep.output);
    }

    #[tokio::test]
    async fn manifest_refreshes_after_file_event() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("README.md"), b"# hello\n");
        let manifest = LocalWorkspaceManifest::start(temp.path());
        let mut rx = manifest.subscribe();
        let initial = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(initial.files.iter().any(|file| file.path == "README.md"));

        write(&temp.path().join("src/lib.rs"), b"pub fn lib() {}\n");
        let updated = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let snapshot = rx.recv().await.unwrap();
                if snapshot.files.iter().any(|file| file.path == "src/lib.rs") {
                    break snapshot;
                }
            }
        })
        .await
        .unwrap();

        assert!(updated.version > initial.version);
    }

    #[tokio::test]
    async fn manifest_search_falls_back_before_initial_scan() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("src/main.rs"), b"fn main() {}\n");
        let backend = ManifestWorkspaceBackend::new(temp.path());
        let glob = backend
            .glob(WorkspaceGlobRequest {
                base: backend.normalize("src").unwrap(),
                pattern: "*.rs".to_string(),
            })
            .await
            .unwrap();
        assert!(glob
            .matches
            .iter()
            .any(|path| path.as_str() == "src/main.rs"));
    }

    #[test]
    fn scan_includes_files_inside_nested_git_workspaces() {
        if !git_available() {
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        run_git(temp.path(), &["init"]);
        write(&temp.path().join("README.md"), b"# root\n");

        let nested = temp.path().join("vendor/child");
        std::fs::create_dir_all(&nested).unwrap();
        run_git(&nested, &["init"]);
        write(&nested.join("src/lib.rs"), b"pub fn child() {}\n");

        let files = scan_workspace_files(temp.path());
        assert!(files.iter().any(|file| file.path == "README.md"));
        assert!(files
            .iter()
            .any(|file| file.path == "vendor/child/src/lib.rs"));
    }
}
