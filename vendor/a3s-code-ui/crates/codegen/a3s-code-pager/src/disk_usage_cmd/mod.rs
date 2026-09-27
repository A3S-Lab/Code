//! `a3s du`: what the user's a3s home uses on disk.
//! It creates no a3s home, registry file, or schema.
//! A read-only open of a WAL database still leaves `-shm` and `-wal` sidecars, so sizes are collected before the registry opens.
mod display;
use crate::fs_size::{
    BucketSize, Measure, Volume, WalkIssues, modified_at, physical_buckets, physical_dir_size,
    physical_file_size, volume_bytes,
};
use a3s_fast_worktree::{
    ListFilter, RegistryOpen, SqliteFailureKind, WORKTREE_POOL_DIR, WORKTREES_DIR, WorktreeDb,
    WorktreeKind, WorktreeRecord, WorktreeStatus, classify_sqlite_error, discover_worktrees,
    managed_worktree_roots, path_under_worktree_roots, resolve_grok_home,
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
/// Bump when a field changes meaning or is removed. Additions are free.
const SCHEMA_VERSION: u32 = 2;
#[derive(Clone, Debug, clap::Args)]
#[command(
    after_help = "Lists every top-level directory in the grok home, largest first, then every \
worktree under `worktrees/` and `worktree_pool/` with its size, age, and label. To reclaim space, preview a sweep with \
`grok worktree gc --max-age 7d --dry-run`: without `--max-age`, gc expires nothing, it \
visits only worktrees the registry tracks, and it keeps a worktree whose work \
it cannot find elsewhere. Inspect Grove artifacts with `grok worktree redirect list <mount>` \
and purge them with `grok worktree clean-artifacts <mount> --yes`."
)]
pub struct DiskUsageArgs {
    /// Emit machine-readable JSON output.
    #[arg(long)]
    pub json: bool,
    /// Purge live redirect contents and ownership-proven orphan jails.
    #[arg(long, requires = "yes", group = "clean_mode")]
    pub clean: bool,
    /// Delete only ownership-proven orphan redirect jails.
    #[arg(long, requires = "yes", group = "clean_mode")]
    pub clean_orphaned: bool,
    /// Confirm an irreversible redirect cleanup.
    #[arg(long, requires = "clean_mode")]
    pub yes: bool,
}
pub fn run(args: DiskUsageArgs) -> Result<()> {
    run_inner(args)
}
fn run_inner(args: DiskUsageArgs) -> Result<()> {
    if args.clean || args.clean_orphaned {
        anyhow::bail!("redirect cleanup is not available in this build");
    }
    let grok_home = resolve_grok_home()?;
    let mut out = std::io::stdout().lock();
    let present = grok_home
        .try_exists()
        .with_context(|| format!("cannot stat {}", grok_home.display()))?;
    if !present {
        let report = empty_report(&grok_home);
        if args.json {
            return write_report(&report, args.json, &mut out);
        }
        return Ok(crate::util::ignore_broken_pipe(
            display::print_missing_home_report(&report, &mut out),
        )?);
    }
    let grok_home = dunce::canonicalize(&grok_home).unwrap_or(grok_home);
    write_report(&collect_report(&grok_home)?, args.json, &mut out)
}
fn write_report(report: &DiskUsageReport, json: bool, out: &mut impl Write) -> Result<()> {
    let rendered = if json {
        Some(serde_json::to_string_pretty(report)?)
    } else {
        None
    };
    let written = match &rendered {
        Some(json) => writeln!(out, "{json}"),
        None => display::print_report(report, crate::util::unix_now(), out),
    };
    Ok(crate::util::ignore_broken_pipe(written)?)
}
fn empty_report(grok_home: &Path) -> DiskUsageReport {
    DiskUsageReport {
        grok_home: grok_home.to_string_lossy().into_owned(),
        ..DiskUsageReport::default()
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct SkipCounts {
    skipped_entries: u64,
    unreadable_dirs: u64,
    unstatable_entries: u64,
    /// Directories on another filesystem. No total and no row holds them.
    other_filesystem_dirs: u64,
}
impl From<WalkIssues> for SkipCounts {
    fn from(issues: WalkIssues) -> Self {
        Self {
            skipped_entries: issues.skipped(),
            unreadable_dirs: issues.unreadable_dirs,
            unstatable_entries: issues.unstatable_entries,
            other_filesystem_dirs: issues.other_filesystems,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RegistryState {
    Read,
    Absent,
    Busy,
    Unopenable,
    Corrupt,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct OrphanedRedirection {
    id: String,
    bytes: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct UnattributedRedirectDir {
    path: String,
    bytes: u64,
}
#[derive(Debug, Serialize)]
pub(crate) struct DiskUsageReport {
    schema_version: u32,
    grok_home: String,
    total_bytes: u64,
    /// Capacity less available is at least the used bytes (`f_bavail` withholds the root reserve).
    /// A larger `total_bytes` therefore proves blocks were counted more than once.
    volume_capacity_bytes: Option<u64>,
    volume_available_bytes: Option<u64>,
    /// Largest first.
    top_level_dirs: Vec<DirUsage>,
    root_files_bytes: u64,
    #[serde(default)]
    redirections_bytes: u64,
    #[serde(default)]
    orphaned_redirections: Vec<OrphanedRedirection>,
    #[serde(default)]
    unattributed_redirect_dirs: Vec<UnattributedRedirectDir>,
    #[serde(flatten)]
    skips: SkipCounts,
    /// Symlinked directories are not followed, so a symlinked `worktrees/` leaves the total short.
    unfollowed_dir_symlinks: u64,
    worktrees_outside_managed_roots: u64,
    registry: RegistryState,
    registry_path: String,
    /// Largest first.
    worktrees: Vec<WorktreeUsage>,
}
impl Default for DiskUsageReport {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            grok_home: String::new(),
            total_bytes: 0,
            volume_capacity_bytes: None,
            volume_available_bytes: None,
            top_level_dirs: Vec::new(),
            root_files_bytes: 0,
            redirections_bytes: 0,
            orphaned_redirections: Vec::new(),
            unattributed_redirect_dirs: Vec::new(),
            skips: SkipCounts::default(),
            unfollowed_dir_symlinks: 0,
            worktrees_outside_managed_roots: 0,
            registry: RegistryState::Absent,
            registry_path: String::new(),
            worktrees: Vec::new(),
        }
    }
}
impl DiskUsageReport {
    fn worktrees_dominate(&self) -> bool {
        let dir_bytes: u64 = self
            .top_level_dirs
            .iter()
            .filter(|e| e.name == WORKTREES_DIR || e.name == WORKTREE_POOL_DIR)
            .filter_map(|e| e.bytes)
            .sum();
        let row_bytes: u64 = self.worktrees.iter().filter_map(|w| w.bytes).sum();
        let worktree_bytes = dir_bytes.max(row_bytes);
        worktree_bytes > 0 && worktree_bytes.saturating_mul(2) >= self.total_bytes
    }
    fn total_exceeds_volume_used(&self) -> bool {
        let Some(capacity) = self.volume_capacity_bytes else {
            return false;
        };
        let Some(available) = self.volume_available_bytes else {
            return false;
        };
        self.total_bytes > capacity.saturating_sub(available)
    }
}
/// `bytes` is `None` for a directory on another filesystem: nothing sized it.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct DirUsage {
    name: String,
    bytes: Option<u64>,
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Registration {
    Untracked,
    Tracked(TrackedRow),
}
impl Registration {
    fn record(&self) -> Option<&TrackedRow> {
        match self {
            Self::Tracked(rec) => Some(rec),
            Self::Untracked => None,
        }
    }
}
/// `last_accessed_at` is stamped by session and agent activity, not by a shell parked in the tree.
/// `db rebuild` registers rows with no `git_ref`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TrackedRow {
    id: String,
    status: WorktreeStatus,
    created_at: i64,
    last_accessed_at: Option<i64>,
    label: Option<String>,
    repo_name: String,
    git_ref: Option<String>,
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct WorktreeUsage {
    bytes: Option<u64>,
    kind: WorktreeKind,
    registration: Registration,
    last_modified_at: Option<i64>,
    path: String,
}
/// Hand-written so the enum stays flat on the wire: twelve keys, fixed order.
impl Serialize for WorktreeUsage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let rec = self.registration.record();
        let mut row = serializer.serialize_struct("WorktreeUsage", 12)?;
        row.serialize_field("bytes", &self.bytes)?;
        row.serialize_field("kind", &self.kind)?;
        row.serialize_field("tracked", &rec.is_some())?;
        row.serialize_field("id", &rec.map(|r| &r.id))?;
        row.serialize_field("status", &rec.map(|r| r.status))?;
        row.serialize_field("created_at", &rec.map(|r| r.created_at))?;
        row.serialize_field("last_accessed_at", &rec.and_then(|r| r.last_accessed_at))?;
        row.serialize_field("last_modified_at", &self.last_modified_at)?;
        row.serialize_field("label", &rec.and_then(|r| r.label.as_deref()))?;
        row.serialize_field("repo_name", &rec.map(|r| &r.repo_name))?;
        row.serialize_field("git_ref", &rec.and_then(|r| r.git_ref.as_deref()))?;
        row.serialize_field("path", &self.path)?;
        row.end()
    }
}
impl WorktreeUsage {
    fn tracked(rec: WorktreeRecord, canonical_path: &Path, size: Measure) -> Self {
        let label = rec.label().map(String::from);
        Self {
            bytes: size.bytes(),
            kind: rec.kind,
            registration: Registration::Tracked(TrackedRow {
                id: rec.id,
                status: rec.status,
                created_at: rec.created_at,
                last_accessed_at: rec.last_accessed_at,
                label,
                repo_name: rec.repo_name,
                git_ref: rec.git_ref,
            }),
            last_modified_at: size.last_modified(),
            path: canonical_path.to_string_lossy().into_owned(),
        }
    }
    fn untracked(kind: WorktreeKind, canonical_path: &Path, size: Measure) -> Self {
        Self {
            bytes: size.bytes(),
            kind,
            registration: Registration::Untracked,
            last_modified_at: size.last_modified(),
            path: canonical_path.to_string_lossy().into_owned(),
        }
    }
    pub(crate) fn is_tracked(&self) -> bool {
        self.registration.record().is_some()
    }
    pub(crate) fn label(&self) -> &str {
        self.registration
            .record()
            .and_then(|r| r.label.as_deref())
            .unwrap_or("")
    }
    /// What gc's age pass measures: the newer of created and last accessed.
    pub(crate) fn age_stamp(&self) -> Option<i64> {
        match self.registration.record() {
            Some(rec) => Some(
                rec.last_accessed_at
                    .unwrap_or(rec.created_at)
                    .max(rec.created_at),
            ),
            None => self.last_modified_at,
        }
    }
}
fn collect_report(grok_home: &Path) -> Result<DiskUsageReport> {
    collect_report_in(grok_home, &[])
}
fn collect_report_in(grok_home: &Path, data_dirs: &[PathBuf]) -> Result<DiskUsageReport> {
    let _ = data_dirs;
    let mut top_level_dirs = Vec::new();
    let mut root_files_bytes = 0u64;
    let mut issues = WalkIssues::default();
    let mut unfollowed_dir_symlinks = 0u64;
    let volume = Volume::of(grok_home);
    let mut worktree_sizes: HashMap<PathBuf, Measure> = HashMap::new();
    let children = std::fs::read_dir(grok_home)
        .with_context(|| format!("cannot read {}", grok_home.display()))?;
    for child in children {
        let child = match child {
            Ok(child) => child,
            Err(e) => {
                tracing::debug!(path = %grok_home.display(), error = %e, "du: home entry could not be read");
                issues.unreadable_dirs += 1;
                continue;
            }
        };
        let file_type = match child.file_type() {
            Ok(file_type) => file_type,
            Err(e) => {
                tracing::debug!(path = %child.path().display(), error = %e, "du: entry type could not be read");
                issues.unstatable_entries += 1;
                continue;
            }
        };
        let name = child.file_name().to_string_lossy().into_owned();
        if !file_type.is_dir() {
            match child.metadata() {
                Ok(meta) => root_files_bytes += physical_file_size(&meta),
                Err(e) => {
                    tracing::debug!(path = %child.path().display(), error = %e, "du: entry could not be stat'd");
                    issues.unstatable_entries += 1;
                }
            }
            if file_type.is_symlink() && std::fs::metadata(child.path()).is_ok_and(|m| m.is_dir()) {
                tracing::debug!(path = %child.path().display(), "du: top-level symlink to a directory is not followed");
                unfollowed_dir_symlinks += 1;
            }
            continue;
        }
        let measure = if name == WORKTREES_DIR || name == WORKTREE_POOL_DIR {
            let sizes = physical_buckets(&child.path(), volume);
            issues.merge(sizes.issues);
            worktree_sizes.extend(sizes.buckets);
            sizes.total
        } else {
            let size = physical_dir_size(&child.path(), volume);
            issues.merge(size.issues);
            size.measure
        };
        top_level_dirs.push(DirUsage {
            name,
            bytes: measure.bytes(),
        });
    }
    top_level_dirs.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
    let total_bytes = top_level_dirs
        .iter()
        .filter_map(|e| e.bytes)
        .fold(root_files_bytes, u64::saturating_add);
    let (registry, records, registry_path) = load_registry(grok_home);
    let rows = collect_worktrees(grok_home, records, &worktree_sizes, volume);
    issues.merge(rows.issues);
    let (volume_capacity_bytes, volume_available_bytes) = volume_bytes(grok_home).unzip();
    Ok(DiskUsageReport {
        schema_version: SCHEMA_VERSION,
        grok_home: grok_home.to_string_lossy().into_owned(),
        total_bytes,
        volume_capacity_bytes,
        volume_available_bytes,
        top_level_dirs,
        root_files_bytes,
        redirections_bytes: 0,
        orphaned_redirections: Vec::new(),
        unattributed_redirect_dirs: Vec::new(),
        skips: issues.into(),
        unfollowed_dir_symlinks,
        worktrees_outside_managed_roots: rows.outside_managed_roots,
        registry,
        registry_path: registry_path.to_string_lossy().into_owned(),
        worktrees: rows.rows,
    })
}
fn load_registry(grok_home: &Path) -> (RegistryState, Vec<WorktreeRecord>, PathBuf) {
    classify(WorktreeDb::open_read_only(grok_home))
}
/// The arm an open failed on decides nothing: a busy writer, an IO blip, and file damage all fail the same call.
/// The error code alone decides `Corrupt`.
fn classify(open: RegistryOpen) -> (RegistryState, Vec<WorktreeRecord>, PathBuf) {
    match open {
        RegistryOpen::Opened { path, db } => {
            match db.list(&ListFilter {
                include_dead: true,
                ..Default::default()
            }) {
                Ok(records) => (RegistryState::Read, records, path),
                Err(e) => (from_sqlite(e), Vec::new(), path),
            }
        }
        RegistryOpen::Absent { path } => (RegistryState::Absent, Vec::new(), path),
        RegistryOpen::Busy { path, error } => {
            (failed(RegistryState::Busy, error), Vec::new(), path)
        }
        RegistryOpen::Failed { path, error } => (from_sqlite(error), Vec::new(), path),
    }
}
fn from_sqlite(e: anyhow::Error) -> RegistryState {
    failed(state_for(classify_sqlite_error(&e)), e)
}
fn state_for(kind: SqliteFailureKind) -> RegistryState {
    match kind {
        SqliteFailureKind::Busy => RegistryState::Busy,
        SqliteFailureKind::Corrupt => RegistryState::Corrupt,
        SqliteFailureKind::Other => RegistryState::Unopenable,
    }
}
fn failed(state: RegistryState, e: anyhow::Error) -> RegistryState {
    tracing::error!(
        error = format!("{e:#}"),
        state = ?state,
        "worktree registry unavailable; reporting rows as untracked"
    );
    state
}
#[derive(Default)]
struct WorktreeRows {
    rows: Vec<WorktreeUsage>,
    issues: WalkIssues,
    outside_managed_roots: u64,
}
fn collect_worktrees(
    grok_home: &Path,
    registered: Vec<WorktreeRecord>,
    sizes: &HashMap<PathBuf, Measure>,
    volume: Volume,
) -> WorktreeRows {
    let mut out = WorktreeRows::default();
    let mut known: HashSet<PathBuf> = HashSet::new();
    let roots = managed_worktree_roots(grok_home);
    for rec in registered {
        let path = match dunce::canonicalize(&rec.path) {
            Ok(path) => path,
            Err(_) => match rec.path.try_exists() {
                Ok(true) => rec.path.clone(),
                Ok(false) => continue,
                Err(e) => {
                    tracing::debug!(path = %rec.path.display(), error = %e, "du: record path could not be stat'd");
                    out.issues.unreadable_dirs += 1;
                    continue;
                }
            },
        };
        if !known.insert(path.clone()) {
            continue;
        }
        if path_under_worktree_roots(&path, &roots) {
            let size = row_size(&path, sizes, &mut out.issues, volume);
            out.rows.push(WorktreeUsage::tracked(rec, &path, size));
        } else {
            out.outside_managed_roots += 1;
        }
    }
    for found in discover_worktrees(grok_home).found {
        let path = dunce::canonicalize(&found.path).unwrap_or(found.path);
        if !known.insert(path.clone()) {
            continue;
        }
        if !path_under_worktree_roots(&path, &roots) {
            out.outside_managed_roots += 1;
            continue;
        }
        let size = row_size(&path, sizes, &mut out.issues, volume);
        out.rows
            .push(WorktreeUsage::untracked(found.kind, &path, size));
    }
    out.rows
        .sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
    out
}
fn row_size(
    path: &Path,
    sizes: &HashMap<PathBuf, Measure>,
    issues: &mut WalkIssues,
    volume: Volume,
) -> Measure {
    if let Some(size) = sizes.get(path) {
        return *size;
    }
    if !volume.holds(path) {
        tracing::debug!(path = %path.display(), "du: row is on another filesystem");
        issues.other_filesystems += 1;
        return Measure::Elsewhere;
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            let size = physical_dir_size(path, volume);
            issues.merge(size.issues);
            size.measure
        }
        Ok(meta) => Measure::Counted(BucketSize {
            bytes: physical_file_size(&meta),
            last_modified: modified_at(&meta),
        }),
        Err(e) => {
            tracing::debug!(path = %path.display(), error = %e, "du: row could not be stat'd");
            issues.unreadable_dirs += 1;
            Measure::Counted(BucketSize::default())
        }
    }
}
#[cfg(test)]
mod tests;
