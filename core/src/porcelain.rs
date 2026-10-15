//! Workspace mutation paths taken from git effects, never from command text.
//!
//! Dirty-line porcelain catches uncommitted writes. A clean checkout can change
//! tracked files without leaving porcelain dirty, so a changed HEAD also
//! contributes `git diff --name-only` between the two revisions when this
//! command ran a HEAD-moving git subcommand. A parallel commit in the same
//! worktree moves HEAD while this command only reads git; those paths belong
//! to that commit.

use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(any(unix, test))]
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
#[cfg(any(unix, test))]
use std::io::Read;
use std::path::Path;
use std::time::Duration;

/// `git status -u` on a large worktree takes longer than a few hundred
/// milliseconds. A shorter budget made porcelain return nothing, and the
/// file walk then hit its cap and marked a real git tree incomplete.
/// Five seconds covers that status. A read that still misses inside a
/// worktree stays incomplete and does not walk the tree.
const GIT_QUERY_TIMEOUT: Duration = Duration::from_secs(5);
/// Bound subprocess output before allocating authority baselines. Overflow is
/// an unreadable query, never a successful prefix of the repository state.
const GIT_OUTPUT_BYTE_CAP: u64 = 4 * 1024 * 1024;

/// Identity of a path already named by porcelain. A later write, mode change, or
/// type change that keeps the same status line is still a mutation. This is
/// not a second effect identity and not a full-tree hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ContentStamp {
    pub path: String,
    pub present: bool,
    pub len: u64,
    pub modified: u128,
    #[serde(default)]
    pub mode: u32,
    #[serde(default)]
    pub kind: u8,
    #[serde(default)]
    pub content_digest: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Snapshot {
    porcelain: Option<Vec<String>>,
    head: Option<String>,
    stamps: Vec<ContentStamp>,
    stamps_incomplete: bool,
    /// This directory is a git worktree, including when `status` itself
    /// failed. A failed git read must not fall through to the file walk.
    git_worktree: bool,
}

impl Snapshot {
    pub(crate) fn observes_git(&self) -> bool {
        self.git_worktree || self.porcelain.is_some()
    }
}

/// One before/after observation of a workspace. Git uses porcelain; other
/// roots use a bounded file index. Callers attach the result as
/// `changed_paths` and must not parse command text.
pub(crate) struct Watch {
    snapshot: Snapshot,
    files: Option<FileIndex>,
}

impl Watch {
    /// The bounded authority baseline, without the model's display truncation.
    /// The durable fact writer applies its own serialized-size limit before
    /// execution is authorized.
    pub(crate) fn receipt_baseline(&self) -> Value {
        serde_json::json!({
            "git": self.snapshot,
            "nongit": self.files.as_ref().map(FileIndex::summary),
        })
    }

    pub(crate) async fn start(root: &Path) -> Self {
        let snapshot = snapshot(root).await;
        let files = if snapshot.observes_git() {
            None
        } else {
            Some(index_files(root).with_content(root))
        };
        Self { snapshot, files }
    }

    pub(crate) async fn finish(self, root: &Path) -> Vec<String> {
        self.finish_with_revision(root, true).await
    }

    /// `attribute_revision` is false when this command did not run a
    /// HEAD-moving git subcommand. A parallel commit's clean paths are then
    /// left off the command delta.
    pub(crate) async fn finish_with_revision(
        self,
        root: &Path,
        attribute_revision: bool,
    ) -> Vec<String> {
        self.finish_observed_with_revision(root, attribute_revision)
            .await
            .paths
    }

    /// Preserve read failures and bounded-walk truncation for durable receipts.
    /// Known paths remain useful even when the observation is incomplete.
    pub(crate) async fn finish_observed_with_revision(
        self,
        root: &Path,
        attribute_revision: bool,
    ) -> PathDelta {
        let mut observed = command_delta(root, self.snapshot, attribute_revision).await;
        if let Some(files) = self.files.as_ref() {
            let files = file_delta(root, files);
            observed.paths.extend(files.paths);
            observed.incomplete |= files.incomplete;
        }
        observed.paths.sort();
        observed.paths.dedup();
        observed
    }
}

/// Paths the gate can bind, or an explicit failure to re-read the tree.
/// An incomplete read is not an empty mutation and is not a smaller digest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PathDelta {
    pub paths: Vec<String>,
    pub incomplete: bool,
}

/// The one typed byte observer used by foreground and background receipts.
/// An unsafe or failed read stays Unknown, never an invented Missing identity.
pub(crate) fn observe_path_effects(
    root: &Path,
    delta: PathDelta,
) -> a3s_effect::ToolEffectObservationV1 {
    observe_path_effects_within(root, delta, CHANGED_PATH_BYTE_CAP)
}

fn observe_path_effects_within(
    root: &Path,
    mut delta: PathDelta,
    mut budget: u64,
) -> a3s_effect::ToolEffectObservationV1 {
    use a3s_effect::{
        ToolEffectObservationV1, ToolFileIdentityV1, ToolFileKind, ToolObservationCompleteness,
        ToolPathChangeV1,
    };
    let mut invalid_scope = false;
    delta.paths.retain(|path| {
        if path.len() > OBSERVATION_PATH_BYTE_CAP
            || path.contains('\0')
            || Path::new(path).components().any(|component| {
                !matches!(
                    component,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            })
        {
            // A harness-looking escape must not become a complete empty
            // observation merely because metadata paths are normally hidden.
            invalid_scope = true;
            false
        } else {
            !is_harness_path(path)
        }
    });
    delta.incomplete |= invalid_scope;
    delta.paths.sort();
    delta.paths.dedup();
    let mut effects = ToolEffectObservationV1::complete(Vec::new());
    if delta.paths.len() > a3s_effect::MAX_TOOL_EFFECT_PATHS {
        delta.incomplete = true;
        delta.paths.truncate(a3s_effect::MAX_TOOL_EFFECT_PATHS);
    }
    for path in delta.paths {
        if path.is_empty() || path.len() > OBSERVATION_PATH_BYTE_CAP {
            delta.incomplete = true;
            continue;
        }
        let after = match observe_file_identity(root, &path, &mut budget) {
            Some(stamp) => ToolFileIdentityV1 {
                kind: if !stamp.present {
                    ToolFileKind::Missing
                } else {
                    match stamp.kind {
                        0 => ToolFileKind::File,
                        1 => ToolFileKind::Directory,
                        2 => ToolFileKind::Symlink,
                        _ => ToolFileKind::Unknown,
                    }
                },
                content_digest: stamp
                    .content_digest
                    .map(|digest| format!("sha256:{digest}")),
                mode: stamp.present.then_some(stamp.mode),
            },
            None => {
                delta.incomplete = true;
                ToolFileIdentityV1 {
                    kind: ToolFileKind::Unknown,
                    content_digest: None,
                    mode: None,
                }
            }
        };
        effects.changes.push(ToolPathChangeV1 {
            path,
            before: None,
            after,
        });
    }
    if delta.incomplete {
        effects.completeness = ToolObservationCompleteness::Incomplete;
        effects.reason = Some(
            "tool_effect_observation_incomplete: workspace read or observation budget failed"
                .into(),
        );
    }
    if effects.validate().is_err() {
        return ToolEffectObservationV1::incomplete(
            "tool_effect_observation_incomplete: typed receipt contract or canonical size bound failed",
        );
    }
    effects
}

pub(crate) async fn snapshot(root: &Path) -> Snapshot {
    let porcelain = lines(root).await;
    let git_worktree = porcelain.is_some() || inside_work_tree(root).await;
    let mut byte_budget = OBSERVATION_BYTE_CAP;
    let mut path_budget = FILE_WALK_CAP;
    let (mut stamps, mut stamps_incomplete) = match &porcelain {
        Some(lines) => stamp_lines(root, lines, &mut byte_budget, &mut path_budget),
        // Status missed inside a worktree. That is not an empty non-git tree.
        None => (Vec::new(), git_worktree),
    };
    if porcelain.is_some() {
        match hidden_index_paths(root).await {
            Some((paths, parse_incomplete)) => {
                stamps_incomplete |= parse_incomplete;
                let (extra, extra_incomplete) =
                    stamp_paths(root, &paths, &mut byte_budget, &mut path_budget);
                stamps_incomplete |= extra_incomplete;
                stamps.extend(extra);
                stamps.sort_by(|left, right| left.path.cmp(&right.path));
                stamps.dedup_by(|left, right| left.path == right.path);
            }
            None => stamps_incomplete = true,
        }
    }
    Snapshot {
        porcelain,
        head: revision(root).await,
        stamps,
        stamps_incomplete,
        git_worktree,
    }
}

async fn inside_work_tree(root: &Path) -> bool {
    let Some(output) = git(root, &["rev-parse", "--is-inside-work-tree"]).await else {
        return false;
    };
    String::from_utf8_lossy(&output).trim() == "true"
}

pub(crate) async fn lines(root: &Path) -> Option<Vec<String>> {
    let prefix = workspace_git_prefix(root).await?;
    let output = git(root, &["status", "--porcelain", "-z", "-u", "--", "."]).await?;
    let (records, incomplete) = porcelain_records(&output);
    if incomplete {
        return None;
    }
    scope_porcelain_records(records, &prefix)
}

/// Porcelain always names paths relative to the repository top, even when
/// invoked from a nested workspace. Keep the exact prefix from this cwd;
/// never join a repository-relative path onto the smaller workspace root.
async fn workspace_git_prefix(root: &Path) -> Option<String> {
    let output = git(root, &["rev-parse", "--show-prefix"]).await?;
    let prefix = std::str::from_utf8(&output).ok()?.strip_suffix('\n')?;
    if prefix.len() > OBSERVATION_PATH_BYTE_CAP
        || prefix.contains('\n')
        || (!prefix.is_empty() && !prefix.ends_with('/'))
    {
        return None;
    }
    Some(prefix.to_string())
}

fn bind_git_path<'a>(path: &'a str, prefix: &str) -> Result<Option<&'a str>, ()> {
    let Some(relative) = path.strip_prefix(prefix) else {
        return Ok(None);
    };
    if relative.is_empty()
        || relative.len() > OBSERVATION_PATH_BYTE_CAP
        || Path::new(relative)
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(());
    }
    Ok(Some(relative))
}

fn scope_porcelain_records(records: Vec<String>, prefix: &str) -> Option<Vec<String>> {
    let mut scoped = Vec::new();
    for record in records {
        let status = record.get(..2)?;
        for path in paths_of(&record)? {
            if let Some(relative) = bind_git_path(&path, prefix).ok()? {
                scoped.push(format!("{status} {}", quote_git_path(relative)));
            }
        }
    }
    Some(scoped)
}

pub(crate) async fn delta(root: &Path, before: Snapshot) -> PathDelta {
    command_delta(root, before, true).await
}

async fn command_delta(root: &Path, before: Snapshot, attribute_revision: bool) -> PathDelta {
    let after = snapshot(root).await;
    let names = match (&before.head, &after.head) {
        (Some(before_head), Some(after_head)) if before_head != after_head => {
            names_between(root, before_head, after_head).await
        }
        _ => Some(Vec::new()),
    };
    // A failed revision read stays incomplete. An unread move is not "no paths".
    let names_for_combine = if attribute_revision || names.is_none() {
        names.clone()
    } else {
        Some(Vec::new())
    };
    let mut observed = combine_delta(
        before.porcelain.as_deref(),
        after.porcelain.as_deref(),
        before.head.as_deref(),
        after.head.as_deref(),
        names_for_combine,
    );
    observed.incomplete |= before.stamps_incomplete || after.stamps_incomplete;
    observed
        .paths
        .extend(content_changes(&before.stamps, &after.stamps));
    if !attribute_revision && !observed.incomplete {
        if let (Some(names), Some(lines)) = (names, after.porcelain.as_deref()) {
            if let Some(still_dirty) = porcelain_paths(lines) {
                drop_foreign_revision(&mut observed.paths, &names, &still_dirty);
            }
        }
    }
    observed.paths.sort();
    observed.paths.dedup();
    observed
}

/// Paths a parallel commit made clean are that commit's effect. A path this
/// command left dirty stays, even if the same name is in the revision diff.
fn drop_foreign_revision(
    paths: &mut Vec<String>,
    revision_paths: &[String],
    still_dirty: &[String],
) {
    if revision_paths.is_empty() {
        return;
    }
    paths.retain(|path| {
        let from_revision = revision_paths.iter().any(|name| name == path);
        !from_revision || still_dirty.iter().any(|dirty| dirty == path)
    });
}

fn porcelain_paths(lines: &[String]) -> Option<Vec<String>> {
    let mut paths = Vec::new();
    for line in lines {
        paths.extend(paths_of(line)?);
    }
    paths.sort();
    paths.dedup();
    Some(paths)
}

/// Join a before/after git read. A missing re-read is incomplete. Every path
/// git already returned is kept; dropping a suffix would let a waiver of the
/// prefix close the gate.
fn combine_delta(
    before_porcelain: Option<&[String]>,
    after_porcelain: Option<&[String]>,
    before_head: Option<&str>,
    after_head: Option<&str>,
    names_between: Option<Vec<String>>,
) -> PathDelta {
    let mut incomplete = false;
    let mut paths = match (before_porcelain, after_porcelain) {
        (Some(before_lines), Some(after_lines)) => {
            let parsed = diff_porcelain(before_lines, after_lines);
            incomplete |= parsed.incomplete;
            parsed.paths
        }
        (Some(_), None) => {
            incomplete = true;
            Vec::new()
        }
        _ => Vec::new(),
    };
    match (before_head, after_head) {
        (Some(before_head), Some(after_head)) if before_head != after_head => match names_between {
            Some(names) => paths.extend(names),
            None => incomplete = true,
        },
        (Some(_), None) => incomplete = true,
        _ => {}
    }
    paths.sort();
    paths.dedup();
    PathDelta { paths, incomplete }
}

#[allow(dead_code)] // bash unit tests compare porcelain lines through this helper
pub(crate) fn changed_paths(before: &[String], after: &[String]) -> Vec<String> {
    diff_porcelain(before, after).paths
}

fn diff_porcelain(before: &[String], after: &[String]) -> PathDelta {
    let before_set: HashSet<&str> = before.iter().map(String::as_str).collect();
    let after_set: HashSet<&str> = after.iter().map(String::as_str).collect();
    let mut paths = Vec::new();
    let mut incomplete = false;
    for line in after
        .iter()
        .filter(|line| !before_set.contains(line.as_str()))
        .chain(
            before
                .iter()
                .filter(|line| !after_set.contains(line.as_str())),
        )
    {
        match paths_of(line) {
            Some(found) => paths.extend(found),
            None => incomplete = true,
        }
    }
    paths.sort();
    paths.dedup();
    PathDelta { paths, incomplete }
}

const FILE_WALK_CAP: usize = 4_096;
/// Entries a non-git walk may stat. Stat-only, so a dependency tree fits.
const NONGIT_WALK_CAP: usize = 1 << 18;
const OBSERVATION_PATH_BYTE_CAP: usize = 4_096;
/// Bytes a baseline may hash. Baselines run before every mutating call.
pub(crate) const OBSERVATION_BYTE_CAP: u64 = 64 * 1024 * 1024;
/// Bytes one receipt may hash for its changed paths. A changed file without
/// byte identity leaves an obligation that nothing can discharge, so this
/// covers the data files a task writes; beyond it the receipt is incomplete.
const CHANGED_PATH_BYTE_CAP: u64 = 1 << 30;

/// Non-git fallback. Git roots use [`delta`]; this only covers workspaces
/// porcelain cannot see. A truncated walk is incomplete, not an empty
/// mutation: directory order is unstable, so a prefix must not become paths.
/// Like git's index, a node's stat identity is the change detector: ctime
/// moves on every content, mode or link write and no write can set it back,
/// so no bytes are read here. Changed paths get their content identity from
/// [`observe_path_effects`]. Directory errors and unsupported node kinds
/// remain incomplete.
pub(crate) struct FileIndex {
    entries: Vec<NodeStamp>,
    incomplete: bool,
    /// Before-content of a small tree, kept as receipt recovery evidence.
    content: Option<Vec<ContentStamp>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct NodeStamp {
    pub path: String,
    pub kind: u8,
    pub dev: u64,
    pub ino: u64,
    pub mode: u32,
    pub len: u64,
    pub modified: (i64, i64),
    pub changed: (i64, i64),
}

/// The prepared fact is bounded at 1 MiB; leave room for the rest of it.
const CONTENT_BASELINE_BYTE_CAP: usize = 512 * 1024;

impl FileIndex {
    /// Hash a small tree's bytes once, before the call runs. A larger tree,
    /// or one the byte budget cannot cover, keeps only its stat digest.
    pub(crate) fn with_content(mut self, root: &Path) -> Self {
        if self.entries.len() > FILE_WALK_CAP {
            return self;
        }
        let mut budget = OBSERVATION_BYTE_CAP;
        self.content = self
            .entries
            .iter()
            .map(|entry| observe_file_identity(root, &entry.path, &mut budget))
            .collect();
        self
    }

    fn summary(&self) -> Value {
        if let Some(content) = &self.content {
            let full = serde_json::json!({"entries": content, "incomplete": self.incomplete});
            if serde_json::to_vec(&full).is_ok_and(|bytes| bytes.len() <= CONTENT_BASELINE_BYTE_CAP)
            {
                return full;
            }
        }
        let mut digest = Sha256::new();
        for entry in &self.entries {
            digest.update(serde_json::to_vec(entry).unwrap_or_default());
            digest.update(b"\n");
        }
        serde_json::json!({
            "entry_count": self.entries.len(),
            "stat_digest": format!("sha256:{:x}", digest.finalize()),
            "content_omitted": true,
            "incomplete": self.incomplete,
        })
    }
}

pub(crate) fn index_files(root: &Path) -> FileIndex {
    index_files_within(root, NONGIT_WALK_CAP)
}

fn index_files_within(root: &Path, cap: usize) -> FileIndex {
    let mut entries = Vec::new();
    let mut incomplete = false;
    let mut visited = 0;
    let mut stack = vec![std::path::PathBuf::new()];
    while let Some(dir) = stack.pop() {
        let Some((found, directory_incomplete)) = directory_entries(root, &dir, cap - visited)
        else {
            incomplete = true;
            continue;
        };
        incomplete |= directory_incomplete;
        for (name, stat) in found {
            visited += 1;
            if name == ".git" || name == ".a3s" || name == ".a3s-code" {
                continue;
            }
            let path = dir.join(name);
            let Some(stamp) = path
                .to_str()
                .and_then(|relative| node_stamp(relative, &stat))
            else {
                incomplete = true;
                continue;
            };
            if stamp.kind == 1 {
                stack.push(path);
            }
            entries.push(stamp);
        }
        if visited >= cap && !stack.is_empty() {
            incomplete = true;
            break;
        }
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    FileIndex {
        entries,
        incomplete,
        content: None,
    }
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast, clippy::useless_conversion)] // libc stat field widths vary by target.
fn node_stamp(relative: &str, stat: &libc::stat) -> Option<NodeStamp> {
    if relative.is_empty() || relative.len() > OBSERVATION_PATH_BYTE_CAP {
        return None;
    }
    let kind = match stat.st_mode & libc::S_IFMT {
        libc::S_IFREG => 0,
        libc::S_IFDIR => 1,
        libc::S_IFLNK => 2,
        _ => return None,
    };
    let (modified_nsec, changed_nsec) = stat_subseconds(stat);
    Some(NodeStamp {
        path: relative.to_string(),
        kind,
        dev: stat.st_dev as u64,
        ino: stat.st_ino as u64,
        mode: stat.st_mode as u32,
        len: u64::try_from(stat.st_size).ok()?,
        modified: (i64::from(stat.st_mtime), i64::try_from(modified_nsec).ok()?),
        changed: (i64::from(stat.st_ctime), i64::try_from(changed_nsec).ok()?),
    })
}

#[cfg(not(unix))]
fn node_stamp(_relative: &str, _stat: &()) -> Option<NodeStamp> {
    None
}

pub(crate) fn file_delta(root: &Path, before: &FileIndex) -> PathDelta {
    let after = index_files(root);
    if before.incomplete || after.incomplete {
        // Directory order is unstable past the cap. Comparing a prefix would
        // invent paths; returning a complete empty delta would hide a write.
        return PathDelta {
            paths: Vec::new(),
            incomplete: true,
        };
    }
    let before_map: HashMap<&str, &NodeStamp> = before
        .entries
        .iter()
        .map(|stamp| (stamp.path.as_str(), stamp))
        .collect();
    let after_map: HashMap<&str, &NodeStamp> = after
        .entries
        .iter()
        .map(|stamp| (stamp.path.as_str(), stamp))
        .collect();
    let mut paths = Vec::new();
    for (path, stamp) in &after_map {
        if before_map.get(path) != Some(stamp) {
            paths.push((*path).to_string());
        }
    }
    for path in before_map.keys() {
        if !after_map.contains_key(path) {
            paths.push((*path).to_string());
        }
    }
    paths.sort();
    paths.dedup();
    PathDelta {
        paths,
        incomplete: false,
    }
}

/// Baseline taken at run start. Queried again at the gate so a writer that
/// never published a path still opens the same ledger. `.a3s` and `.git` are
/// harness metadata, not source mutations.
pub(crate) struct RunWatch {
    snapshot: Snapshot,
    files: Option<FileIndex>,
}

impl RunWatch {
    pub(crate) async fn start(root: &Path) -> Self {
        let snapshot = snapshot(root).await;
        let files = if snapshot.observes_git() {
            None
        } else {
            Some(index_files(root))
        };
        Self { snapshot, files }
    }

    pub(crate) fn porcelain(&self) -> Option<Vec<String>> {
        self.snapshot.porcelain.clone()
    }

    pub(crate) fn stamps_incomplete(&self) -> bool {
        self.snapshot.stamps_incomplete
    }

    pub(crate) fn head(&self) -> Option<String> {
        self.snapshot.head.clone()
    }

    pub(crate) fn stamps(&self) -> Vec<ContentStamp> {
        self.snapshot.stamps.clone()
    }

    pub(crate) fn file_changes(&self, root: &Path) -> PathDelta {
        let Some(files) = self.files.as_ref() else {
            return PathDelta::default();
        };
        let mut observed = file_delta(root, files);
        observed.paths = source_paths(observed.paths);
        observed
    }
}

pub(crate) async fn baseline_delta(
    root: &Path,
    porcelain: Option<Vec<String>>,
    head: Option<String>,
    stamps: Vec<ContentStamp>,
) -> PathDelta {
    let mut observed = delta(root, snapshot_from_parts(porcelain, head, stamps)).await;
    observed.paths = source_paths(observed.paths);
    observed
}

fn stamp_lines(
    root: &Path,
    lines: &[String],
    byte_budget: &mut u64,
    path_budget: &mut usize,
) -> (Vec<ContentStamp>, bool) {
    let mut paths = Vec::new();
    let mut incomplete = false;
    for line in lines {
        match paths_of(line) {
            Some(found) if paths.len() + found.len() <= FILE_WALK_CAP => paths.extend(found),
            Some(_) => {
                incomplete = true;
                break;
            }
            None => incomplete = true,
        }
    }
    let (stamps, stamp_incomplete) = stamp_paths(root, &paths, byte_budget, path_budget);
    (stamps, incomplete || stamp_incomplete)
}

fn stamp_paths(
    root: &Path,
    paths: &[String],
    byte_budget: &mut u64,
    path_budget: &mut usize,
) -> (Vec<ContentStamp>, bool) {
    let mut stamps = Vec::new();
    let mut incomplete = false;
    let mut seen = HashSet::new();
    for path in paths {
        if path.len() > OBSERVATION_PATH_BYTE_CAP {
            incomplete = true;
            continue;
        }
        if !seen.insert(path.clone()) || is_harness_path(path) {
            continue;
        }
        if *path_budget == 0 {
            incomplete = true;
            break;
        }
        *path_budget -= 1;
        match observe_file_identity(root, path, byte_budget) {
            Some(stamp) => stamps.push(stamp),
            None => incomplete = true,
        }
    }
    stamps.sort_by(|left, right| left.path.cmp(&right.path));
    (stamps, incomplete)
}

/// Paths `git status` hides. `S` is skip-worktree, `h`/`s` is assume-unchanged.
async fn hidden_index_paths(root: &Path) -> Option<(Vec<String>, bool)> {
    let prefix = workspace_git_prefix(root).await?;
    let output = git(root, &["ls-files", "--full-name", "-v", "-z", "--", "."]).await?;
    let (paths, mut incomplete) = parse_hidden_index_paths(&output);
    let mut scoped = Vec::new();
    for path in paths {
        match bind_git_path(&path, &prefix) {
            Ok(Some(relative)) => scoped.push(relative.to_string()),
            Ok(None) => {}
            Err(()) => incomplete = true,
        }
    }
    Some((scoped, incomplete))
}

fn parse_hidden_index_paths(output: &[u8]) -> (Vec<String>, bool) {
    let mut paths = Vec::new();
    let mut incomplete = false;
    for part in output
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
    {
        if part.len() < 3 || part[1] != b' ' {
            incomplete = true;
            continue;
        }
        if !matches!(part[0], b'S' | b's' | b'h') {
            continue;
        }
        let Ok(path) = std::str::from_utf8(&part[2..]) else {
            incomplete = true;
            continue;
        };
        if path.is_empty() || path.contains('\n') || path.len() > OBSERVATION_PATH_BYTE_CAP {
            incomplete = true;
            continue;
        }
        if paths.len() == FILE_WALK_CAP {
            incomplete = true;
            break;
        }
        paths.push(path.to_string());
    }
    (paths, incomplete)
}

/// Read one identity within the workspace. `budget` is remaining readable bytes
/// shared across the observation, not a per-path allowance. A missing path has
/// a complete absent identity; any unsafe or unprovable read returns `None`.
pub(crate) fn observe_file_identity(
    root: &Path,
    relative: &str,
    budget: &mut u64,
) -> Option<ContentStamp> {
    if relative.len() > OBSERVATION_PATH_BYTE_CAP {
        return None;
    }
    #[cfg(unix)]
    {
        observe_identity_unix(root, relative, budget)
    }
    #[cfg(not(unix))]
    {
        // Never substitute a following open on platforms where this observer
        // has no descriptor-anchored implementation. Receipts stay incomplete.
        let _ = (root, relative, budget);
        None
    }
}

#[cfg(unix)]
fn missing_identity(relative: &str) -> ContentStamp {
    ContentStamp {
        path: relative.to_string(),
        present: false,
        len: 0,
        modified: 0,
        mode: 0,
        kind: 0,
        content_digest: None,
    }
}

#[cfg(any(unix, test))]
fn read_content_digest(reader: &mut impl Read, len: u64, budget: &mut u64) -> Option<String> {
    if len > *budget {
        return None;
    }
    let mut reader = reader.take(len);
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut read = 0_u64;
    loop {
        let count = match reader.read(&mut buffer) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        if count == 0 {
            break;
        }
        *budget -= count as u64;
        read += count as u64;
        digest.update(&buffer[..count]);
    }
    (read == len).then(|| format!("{:x}", digest.finalize()))
}

#[cfg(unix)]
fn relative_components(relative: &Path) -> Option<Vec<&std::ffi::OsStr>> {
    let mut parts = Vec::new();
    for part in relative.components() {
        match part {
            std::path::Component::Normal(name) => parts.push(name),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    Some(parts)
}

#[cfg(unix)]
fn open_root_directory(root: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let metadata = std::fs::symlink_metadata(root)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(std::io::Error::other(
            "observation root is not a real directory",
        ));
    }
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(root)
}

#[cfg(unix)]
fn open_at(
    parent: &std::fs::File,
    name: &std::ffi::CStr,
    flags: i32,
) -> std::io::Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    // SAFETY: the parent descriptor and NUL-terminated name live throughout
    // openat; no mode argument is required because O_CREAT is never used.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: openat returned a new descriptor owned by this File alone.
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

#[cfg(unix)]
fn open_parent_directory(
    root: std::fs::File,
    parts: &[&std::ffi::OsStr],
) -> std::io::Result<std::fs::File> {
    use std::os::unix::ffi::OsStrExt;
    let mut directory = root;
    for part in parts {
        let name = std::ffi::CString::new(part.as_bytes())
            .map_err(|_| std::io::Error::other("NUL in observation path"))?;
        directory = open_at(
            &directory,
            &name,
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )?;
    }
    Ok(directory)
}

#[cfg(unix)]
fn stat_at(parent: &std::fs::File, name: &std::ffi::CStr) -> std::io::Result<libc::stat> {
    use std::os::fd::AsRawFd;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: stat points to writable storage; both inputs remain valid. The
    // no-follow flag observes the link itself and cannot read its target.
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful fstatat initialized the entire stat value.
    Ok(unsafe { stat.assume_init() })
}

#[cfg(unix)]
fn stat_file(file: &std::fs::File) -> Option<libc::stat> {
    use std::os::fd::AsRawFd;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: file owns a live descriptor and stat is writable storage.
    if unsafe { libc::fstat(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: successful fstat initialized the entire stat value.
    Some(unsafe { stat.assume_init() })
}

#[cfg(unix)]
fn same_node(before: &libc::stat, after: &libc::stat) -> bool {
    before.st_dev == after.st_dev
        && before.st_ino == after.st_ino
        && before.st_mode == after.st_mode
        && before.st_size == after.st_size
        && before.st_mtime == after.st_mtime
        && before.st_ctime == after.st_ctime
        && stat_subseconds(before) == stat_subseconds(after)
}

#[cfg(unix)]
fn stat_subseconds(stat: &libc::stat) -> (i128, i128) {
    #[cfg(target_os = "netbsd")]
    {
        (stat.st_mtimensec as i128, stat.st_ctimensec as i128)
    }
    #[cfg(not(target_os = "netbsd"))]
    {
        (stat.st_mtime_nsec as i128, stat.st_ctime_nsec as i128)
    }
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // libc::mode_t is u16 on macOS and u32 on Linux.
fn identity_from_stat(
    relative: &str,
    stat: &libc::stat,
    kind: u8,
    digest: Option<String>,
) -> Option<ContentStamp> {
    let modified = u128::try_from(stat.st_mtime)
        .ok()?
        .checked_mul(1_000_000_000)?
        .checked_add(u128::try_from(stat_subseconds(stat).0).ok()?)?;
    Some(ContentStamp {
        path: relative.to_string(),
        present: true,
        len: u64::try_from(stat.st_size).ok()?,
        modified,
        mode: stat.st_mode as u32,
        kind,
        content_digest: digest,
    })
}

#[cfg(unix)]
fn observe_identity_unix(root: &Path, relative: &str, budget: &mut u64) -> Option<ContentStamp> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    let parts = relative_components(Path::new(relative))?;
    let (leaf, ancestors) = parts.split_last()?;
    let root = open_root_directory(root).ok()?;
    let parent = match open_parent_directory(root, ancestors) {
        Ok(parent) => parent,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Some(missing_identity(relative))
        }
        Err(_) => return None,
    };
    let name = std::ffi::CString::new(leaf.as_bytes()).ok()?;
    let before = match stat_at(&parent, &name) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Some(missing_identity(relative))
        }
        Err(_) => return None,
    };
    let len = u64::try_from(before.st_size).ok()?;
    match before.st_mode & libc::S_IFMT {
        libc::S_IFREG => {
            // Gate kind and declared size BEFORE open: a FIFO may block open
            // itself. O_NONBLOCK also closes a regular-file-to-FIFO race.
            if len > *budget {
                return None;
            }
            let mut file = open_at(
                &parent,
                &name,
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
            .ok()?;
            if !same_node(&before, &stat_file(&file)?) {
                return None;
            }
            let digest = read_content_digest(&mut file, len, budget)?;
            if !same_node(&before, &stat_file(&file)?)
                || !same_node(&before, &stat_at(&parent, &name).ok()?)
            {
                return None;
            }
            identity_from_stat(relative, &before, 0, Some(digest))
        }
        libc::S_IFDIR => identity_from_stat(relative, &before, 1, None),
        libc::S_IFLNK => {
            // Hash link spelling only. Never follow it, even to an internal
            // path. A retargeted link is a different observed identity.
            if len == 0 || len > 16 * 1024 || len > *budget {
                return None;
            }
            let mut target = vec![0_u8; len as usize];
            // SAFETY: the parent/name are live and target is writable for len
            // bytes. readlinkat never opens or reads the linked destination.
            let count = unsafe {
                libc::readlinkat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    target.as_mut_ptr().cast(),
                    target.len(),
                )
            };
            let count = usize::try_from(count).ok()?;
            *budget = budget.saturating_sub(count as u64);
            if count != target.len() || !same_node(&before, &stat_at(&parent, &name).ok()?) {
                return None;
            }
            identity_from_stat(
                relative,
                &before,
                2,
                Some(format!("{:x}", Sha256::digest(&target))),
            )
        }
        _ => None,
    }
}

/// Names and their no-follow stats, read through the one anchored directory
/// descriptor so a deep tree costs one `fstatat` per entry.
#[cfg(unix)]
fn directory_entries(
    root: &Path,
    relative: &Path,
    limit: usize,
) -> Option<(Vec<(std::ffi::OsString, libc::stat)>, bool)> {
    use std::os::fd::IntoRawFd;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let errno = directory_errno()?;
    if errno.is_null() {
        return None;
    }
    let parts = relative_components(relative)?;
    let file = open_parent_directory(open_root_directory(root).ok()?, &parts).ok()?;
    let before = stat_file(&file)?;
    let anchor = file.try_clone().ok()?;
    let fd = file.into_raw_fd();
    // SAFETY: ownership of the directory descriptor transfers to fdopendir
    // on success. On failure it is still ours and is closed below.
    let directory = unsafe { libc::fdopendir(fd) };
    if directory.is_null() {
        // SAFETY: fdopendir failed and fd is still an owned live descriptor.
        unsafe {
            libc::close(fd);
        }
        return None;
    }
    let mut names = Vec::new();
    let mut incomplete = false;
    loop {
        // SAFETY: errno points to this thread's live errno cell; clearing it
        // distinguishes readdir EOF from failure. This stream is not shared.
        let found = unsafe {
            *errno = 0;
            libc::readdir(directory)
        };
        if found.is_null() {
            // SAFETY: the thread-local errno pointer remains valid here.
            incomplete |= unsafe { *errno } != 0;
            break;
        }
        // Some filesystems return a d_name longer than libc's fixed array.
        // Read only the record supplied by readdir, never copy a whole dirent
        // into a fixed readdir_r buffer. The entry lives until the next call.
        let offset = std::mem::offset_of!(libc::dirent, d_name);
        // SAFETY: successful readdir returns a valid initialized record.
        let record_len = unsafe { (*found).d_reclen as usize };
        let Some(name_capacity) = record_len.checked_sub(offset) else {
            incomplete = true;
            break;
        };
        if name_capacity == 0 || name_capacity > OBSERVATION_PATH_BYTE_CAP + 1 {
            incomplete = true;
            break;
        }
        let mut name_bytes = Vec::new();
        let mut terminated = false;
        for index in 0..name_capacity {
            // SAFETY: d_reclen bounds the record and readdir initializes the
            // name through its terminating NUL. Stop before any tail padding.
            let byte = unsafe { *found.cast::<u8>().add(offset + index) };
            if byte == 0 {
                terminated = true;
                break;
            }
            name_bytes.push(byte);
        }
        if !terminated {
            incomplete = true;
            break;
        }
        let name = std::ffi::OsString::from_vec(name_bytes);
        if name == "." || name == ".." {
            continue;
        }
        if names.len() == limit {
            incomplete = true;
            break;
        }
        let Ok(c_name) = std::ffi::CString::new(name.as_bytes()) else {
            incomplete = true;
            continue;
        };
        match stat_at(&anchor, &c_name) {
            Ok(stat) => names.push((name, stat)),
            // Vanished or unreadable mid-walk: never a complete baseline.
            Err(_) => incomplete = true,
        }
    }
    // SAFETY: fdopendir succeeded and this is the only close of its stream.
    if unsafe { libc::closedir(directory) } != 0 {
        incomplete = true;
    }
    let current = open_root_directory(root)
        .and_then(|root| open_parent_directory(root, &parts))
        .ok()
        .and_then(|file| stat_file(&file));
    if !current.is_some_and(|current| same_node(&before, &current)) {
        // A directory changed/replaced during enumeration. A transient prefix
        // cannot serve as a complete before/after authority baseline.
        incomplete = true;
    }
    Some((names, incomplete))
}

#[cfg(all(unix, any(target_vendor = "apple", target_os = "freebsd")))]
fn directory_errno() -> Option<*mut libc::c_int> {
    // SAFETY: this libc accessor returns the current thread's errno cell.
    Some(unsafe { libc::__error() })
}

#[cfg(all(unix, any(target_os = "linux", target_os = "dragonfly")))]
fn directory_errno() -> Option<*mut libc::c_int> {
    // SAFETY: this libc accessor returns the current thread's errno cell.
    Some(unsafe { libc::__errno_location() })
}

#[cfg(all(
    unix,
    any(target_os = "android", target_os = "netbsd", target_os = "openbsd")
))]
fn directory_errno() -> Option<*mut libc::c_int> {
    // SAFETY: this libc accessor returns the current thread's errno cell.
    Some(unsafe { libc::__errno() })
}

#[cfg(all(unix, any(target_os = "solaris", target_os = "illumos")))]
fn directory_errno() -> Option<*mut libc::c_int> {
    // SAFETY: this libc accessor returns the current thread's errno cell.
    Some(unsafe { libc::___errno() })
}

#[cfg(all(unix, target_os = "aix"))]
fn directory_errno() -> Option<*mut libc::c_int> {
    // SAFETY: this libc accessor returns the current thread's errno cell.
    Some(unsafe { libc::_Errno() })
}

#[cfg(all(
    unix,
    not(any(
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "android",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "solaris",
        target_os = "illumos",
        target_os = "aix"
    ))
))]
fn directory_errno() -> Option<*mut libc::c_int> {
    None
}

#[cfg(not(unix))]
fn directory_entries(
    _root: &Path,
    _relative: &Path,
    _limit: usize,
) -> Option<(Vec<(std::ffi::OsString, ())>, bool)> {
    None
}

/// Paths present on both sides whose stat changed. A stable status line is
/// not proof the file is unchanged.
fn content_changes(before: &[ContentStamp], after: &[ContentStamp]) -> Vec<String> {
    let before_map: HashMap<&str, &ContentStamp> = before
        .iter()
        .map(|stamp| (stamp.path.as_str(), stamp))
        .collect();
    let mut paths = Vec::new();
    for stamp in after {
        let Some(previous) = before_map.get(stamp.path.as_str()) else {
            continue;
        };
        if *previous != stamp {
            paths.push(stamp.path.clone());
        }
    }
    paths
}

fn source_paths(mut paths: Vec<String>) -> Vec<String> {
    paths.retain(|path| !is_harness_path(path));
    paths
}

pub(crate) fn is_harness_path(path: &str) -> bool {
    // Separators belong to the host filesystem. A Unix backslash is a literal
    // byte in the name, so `.a3s\\file` is not the `.a3s` directory.
    Path::new(path).components().any(|component| {
        matches!(
            component,
            std::path::Component::Normal(name)
                if name == ".a3s" || name == ".a3s-code" || name == ".git"
        )
    })
}

const MODEL_CHANGED_PATH_CAP: usize = 32;

pub(crate) fn attach(metadata: &mut Option<Value>, paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    let truncated = paths.len() > MODEL_CHANGED_PATH_CAP;
    let shown = if truncated {
        &paths[..MODEL_CHANGED_PATH_CAP]
    } else {
        paths
    };
    let mut value = metadata.take().unwrap_or_else(|| serde_json::json!({}));
    if let Some(object) = value.as_object_mut() {
        object.insert("changed_paths".to_string(), serde_json::json!(shown));
        if truncated {
            object.insert("changed_paths_truncated".to_string(), Value::Bool(true));
        }
    }
    *metadata = Some(value);
}

async fn revision(root: &Path) -> Option<String> {
    let output = git(root, &["rev-parse", "HEAD"]).await?;
    let revision = String::from_utf8_lossy(&output).trim().to_string();
    if revision.is_empty() {
        None
    } else {
        Some(revision)
    }
}

async fn names_between(root: &Path, before: &str, after: &str) -> Option<Vec<String>> {
    let prefix = workspace_git_prefix(root).await?;
    let output = git(
        root,
        &[
            "diff",
            "--name-only",
            "--no-relative",
            "-z",
            before,
            after,
            "--",
            ".",
        ],
    )
    .await?;
    let mut names = Vec::new();
    for name in output
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let name = std::str::from_utf8(name).ok()?;
        if name.is_empty()
            || name.contains('\n')
            || name.len() > OBSERVATION_PATH_BYTE_CAP
            || names.len() == FILE_WALK_CAP
        {
            return None;
        }
        if let Some(relative) = bind_git_path(name, &prefix).ok()? {
            names.push(relative.to_string());
        }
    }
    Some(names)
}

async fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    // Dropping the timeout future must kill git. A live `status` otherwise
    // keeps `index.lock` and the next read waits inside its own budget.
    tokio::time::timeout(GIT_QUERY_TIMEOUT, async {
        let mut child = tokio::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .ok()?;
        let output = read_git_output(child.stdout.take()?).await?;
        child.wait().await.ok()?.success().then_some(output)
    })
    .await
    .ok()?
}

async fn read_git_output(reader: impl tokio::io::AsyncRead + Unpin) -> Option<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut output = Vec::new();
    reader
        .take(GIT_OUTPUT_BYTE_CAP + 1)
        .read_to_end(&mut output)
        .await
        .ok()?;
    (output.len() as u64 <= GIT_OUTPUT_BYTE_CAP).then_some(output)
}

/// Status lines from `git status --porcelain -z`. A rename is two NUL fields.
/// An unreadable record is incomplete, not an absent mutation.
fn porcelain_records(output: &[u8]) -> (Vec<String>, bool) {
    let mut records = Vec::new();
    let mut incomplete = false;
    let mut parts = output
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty());
    while let Some(part) = parts.next() {
        if records.len() == FILE_WALK_CAP {
            incomplete = true;
            break;
        }
        if part.len() > OBSERVATION_PATH_BYTE_CAP + 3 {
            incomplete = true;
            continue;
        }
        let Ok(text) = std::str::from_utf8(part) else {
            incomplete = true;
            continue;
        };
        if text.len() < 4 || part[2] != b' ' {
            incomplete = true;
            continue;
        }
        let Some(status) = text.get(..2) else {
            incomplete = true;
            continue;
        };
        let Some(path) = text.get(3..) else {
            incomplete = true;
            continue;
        };
        let renamed = status.contains('R') || status.contains('C');
        if renamed {
            if records.len() + 2 > FILE_WALK_CAP {
                incomplete = true;
                break;
            }
            let Some(dest) = parts.next() else {
                incomplete = true;
                if !path.is_empty() {
                    records.push(format!("{status} {}", quote_git_path(path)));
                }
                continue;
            };
            let Ok(dest) = std::str::from_utf8(dest) else {
                incomplete = true;
                continue;
            };
            if dest.is_empty() || dest.len() > OBSERVATION_PATH_BYTE_CAP {
                incomplete = true;
            } else {
                records.push(format!("{status} {}", quote_git_path(dest)));
            }
            if !path.is_empty() {
                records.push(format!("D  {}", quote_git_path(path)));
            }
            continue;
        }
        if path.is_empty() {
            incomplete = true;
            continue;
        }
        records.push(format!("{status} {}", quote_git_path(path)));
    }
    (records, incomplete)
}

/// NUL porcelain fields are exact names, never C-quoted. Encode them for the
/// existing line representation so quotes, spaces and arrows stay literal.
fn quote_git_path(path: &str) -> String {
    let mut quoted = String::with_capacity(path.len() + 2);
    quoted.push('"');
    for character in path.chars() {
        match character {
            '\\' | '"' => {
                quoted.push('\\');
                quoted.push(character);
            }
            '\0'..='\u{1f}' | '\u{7f}' => {
                quoted.push_str(&format!("\\{:03o}", character as u32));
            }
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

/// Paths named by one porcelain line. `None` means the line could not be
/// decoded, which is not the same as “no path”.
fn paths_of(line: &str) -> Option<Vec<String>> {
    if line.len() < 4 {
        return None;
    }
    let status = line.get(..2)?;
    let body = line.get(3..)?.trim();
    if body.is_empty() {
        return None;
    }
    let pieces = if status.contains('R') || status.contains('C') {
        match split_arrow(body) {
            Some((from, to)) => vec![from, to],
            None => vec![body],
        }
    } else {
        vec![body]
    };
    let mut paths = Vec::with_capacity(pieces.len());
    for piece in pieces {
        let path = decode_git_path(piece)?;
        if path.is_empty() || path.contains('\n') {
            return None;
        }
        paths.push(path);
    }
    Some(paths)
}

fn split_arrow(body: &str) -> Option<(&str, &str)> {
    let bytes = body.as_bytes();
    let mut index = 0;
    let mut quoted = false;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
            index += 1;
            continue;
        }
        if byte == b'"' {
            quoted = true;
            index += 1;
            continue;
        }
        if bytes.get(index..index + 4) == Some(b" -> ") {
            let from = body[..index].trim();
            let to = body[index + 4..].trim();
            if from.is_empty() || to.is_empty() {
                return None;
            }
            return Some((from, to));
        }
        index += 1;
    }
    None
}

fn decode_git_path(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if !raw.starts_with('"') {
        return Some(raw.to_string());
    }
    if raw.len() < 2 || !raw.ends_with('"') {
        return None;
    }
    let inner = &raw[1..raw.len() - 1];
    let mut bytes = Vec::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            let mut encoded = [0; 4];
            bytes.extend(ch.encode_utf8(&mut encoded).as_bytes());
            continue;
        }
        let next = chars.next()?;
        match next {
            '\\' | '"' => bytes.push(next as u8),
            'n' => bytes.push(b'\n'),
            't' => bytes.push(b'\t'),
            'r' => bytes.push(b'\r'),
            'a' => bytes.push(0x07),
            'b' => bytes.push(0x08),
            'f' => bytes.push(0x0c),
            'v' => bytes.push(0x0b),
            digit if digit.is_digit(8) => {
                let mut value = digit as u8 - b'0';
                for _ in 0..2 {
                    let Some(candidate) = chars.clone().next() else {
                        break;
                    };
                    if !candidate.is_digit(8) {
                        break;
                    }
                    chars.next();
                    value = value
                        .saturating_mul(8)
                        .saturating_add(candidate as u8 - b'0');
                }
                bytes.push(value);
            }
            _ => return None,
        }
    }
    String::from_utf8(bytes).ok()
}

/// A background child that shares the parent workspace. The completion gate
/// waits for this slot; it is not a second effect identity.
struct ChildSlot {
    watch: std::sync::Mutex<Option<Watch>>,
    porcelain: std::sync::Mutex<Option<Vec<String>>>,
    head: std::sync::Mutex<Option<String>>,
    nongit: std::sync::atomic::AtomicBool,
    settled: std::sync::Mutex<Option<Vec<String>>>,
    abandoned: std::sync::atomic::AtomicBool,
    notify: tokio::sync::Notify,
    settlement_requested: Option<tokio_util::sync::CancellationToken>,
}

fn child_slots() -> &'static std::sync::Mutex<HashMap<String, std::sync::Arc<ChildSlot>>> {
    static SLOTS: std::sync::OnceLock<
        std::sync::Mutex<HashMap<String, std::sync::Arc<ChildSlot>>>,
    > = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn lock_slots() -> std::sync::MutexGuard<'static, HashMap<String, std::sync::Arc<ChildSlot>>> {
    child_slots()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) struct ChildMarker {
    pub task_id: String,
    pub porcelain: Option<Vec<String>>,
    pub head: Option<String>,
    pub nongit: bool,
}

/// Attach a watch that already started, before the writer is spawned.
pub(crate) fn install_workspace_child(task_id: &str, watch: Watch) {
    let porcelain = watch.snapshot.porcelain.clone();
    let head = watch.snapshot.head.clone();
    let nongit = !watch.snapshot.observes_git();
    lock_slots().insert(
        task_id.to_string(),
        std::sync::Arc::new(ChildSlot {
            watch: std::sync::Mutex::new(Some(watch)),
            porcelain: std::sync::Mutex::new(porcelain),
            head: std::sync::Mutex::new(head),
            nongit: std::sync::atomic::AtomicBool::new(nongit),
            settled: std::sync::Mutex::new(None),
            abandoned: std::sync::atomic::AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
            settlement_requested: None,
        }),
    );
}

/// Reserve before the child can write. The gate observes this id, not command text.
pub(crate) fn reserve_workspace_child(task_id: &str) {
    reserve_workspace_child_inner(task_id, None);
}

pub(crate) fn reserve_workspace_child_with_settlement(
    task_id: &str,
    settlement_requested: tokio_util::sync::CancellationToken,
) {
    reserve_workspace_child_inner(task_id, Some(settlement_requested));
}

fn reserve_workspace_child_inner(
    task_id: &str,
    settlement_requested: Option<tokio_util::sync::CancellationToken>,
) {
    lock_slots().insert(
        task_id.to_string(),
        std::sync::Arc::new(ChildSlot {
            watch: std::sync::Mutex::new(None),
            porcelain: std::sync::Mutex::new(None),
            head: std::sync::Mutex::new(None),
            nongit: std::sync::atomic::AtomicBool::new(false),
            settled: std::sync::Mutex::new(None),
            abandoned: std::sync::atomic::AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
            settlement_requested,
        }),
    );
}

/// Ask an explicitly background command to stop before the completion gate
/// waits for its final observation. This live signal is never recovery proof.
pub(crate) fn request_workspace_child_settlement(task_id: &str) {
    if let Some(slot) = lock_slots().get(task_id) {
        if let Some(requested) = &slot.settlement_requested {
            requested.cancel();
        }
    }
}

pub(crate) async fn begin_workspace_child(task_id: &str, root: &Path) {
    let watch = Watch::start(root).await;
    let Some(slot) = lock_slots().get(task_id).cloned() else {
        return;
    };
    let porcelain = watch.snapshot.porcelain.clone();
    let head = watch.snapshot.head.clone();
    let nongit = !watch.snapshot.observes_git();
    *slot
        .porcelain
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = porcelain;
    *slot
        .head
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = head;
    slot.nongit
        .store(nongit, std::sync::atomic::Ordering::SeqCst);
    *slot
        .watch
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(watch);
}

pub(crate) fn child_marker(task_id: &str) -> Option<ChildMarker> {
    let slot = lock_slots().get(task_id).cloned()?;
    if slot
        .settled
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_some()
    {
        return None;
    }
    let porcelain = slot
        .porcelain
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let head = slot
        .head
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let nongit = slot.nongit.load(std::sync::atomic::Ordering::SeqCst);
    Some(ChildMarker {
        task_id: task_id.to_string(),
        porcelain,
        head,
        nongit,
    })
}

pub(crate) fn peek_settled_workspace_child(task_id: &str) -> Option<Vec<String>> {
    lock_slots()
        .get(task_id)?
        .settled
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

pub(crate) fn take_settled_workspace_child(task_id: &str) -> Option<Vec<String>> {
    let mut slots = lock_slots();
    let slot = slots.get(task_id).cloned()?;
    let paths = slot
        .settled
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()?;
    slots.remove(task_id);
    Some(paths)
}

pub(crate) fn workspace_child_pending(task_id: &str) -> bool {
    lock_slots().contains_key(task_id)
}

pub(crate) struct WorkspaceChildGuard {
    task_id: String,
    settled: bool,
}

impl WorkspaceChildGuard {
    pub(crate) fn new(task_id: &str) -> Self {
        Self {
            task_id: task_id.to_string(),
            settled: false,
        }
    }

    pub(crate) fn settle_paths(&mut self, paths: Vec<String>) {
        settle_workspace_child_paths(&self.task_id, paths);
        self.settled = true;
    }
}

impl Drop for WorkspaceChildGuard {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        let Some(slot) = lock_slots().get(&self.task_id).cloned() else {
            return;
        };
        slot.abandoned
            .store(true, std::sync::atomic::Ordering::SeqCst);
        slot.notify.notify_waiters();
    }
}

pub(crate) async fn settle_workspace_child_guard(guard: &mut WorkspaceChildGuard, root: &Path) {
    settle_workspace_child(&guard.task_id, root).await;
    guard.settled = true;
}

pub(crate) async fn settle_workspace_child(task_id: &str, root: &Path) {
    let Some(slot) = lock_slots().get(task_id).cloned() else {
        return;
    };
    let watch = slot
        .watch
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    let paths = if let Some(watch) = watch {
        watch.finish(root).await
    } else {
        Vec::new()
    };
    *slot
        .settled
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(paths);
    slot.notify.notify_waiters();
}

/// Settle using the command's existing observation rather than taking a second
/// workspace snapshot. The reserved child remains pending until this call.
pub(crate) fn settle_workspace_child_paths(task_id: &str, paths: Vec<String>) {
    let Some(slot) = lock_slots().get(task_id).cloned() else {
        return;
    };
    slot.watch
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    *slot
        .settled
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(paths);
    slot.notify.notify_waiters();
}

pub(crate) async fn await_workspace_child(
    task_id: &str,
    root: &Path,
    cancel: &tokio_util::sync::CancellationToken,
) -> Option<Vec<String>> {
    loop {
        if let Some(paths) = take_settled_workspace_child(task_id) {
            return Some(paths);
        }
        let Some(slot) = lock_slots().get(task_id).cloned() else {
            return Some(Vec::new());
        };
        if slot.abandoned.load(std::sync::atomic::Ordering::SeqCst) {
            return Some(finish_abandoned_workspace_child(task_id, root).await);
        }
        if cancel.is_cancelled() {
            return None;
        }
        let notified = slot.notify.notified();
        tokio::pin!(notified);
        if let Some(paths) = take_settled_workspace_child(task_id) {
            return Some(paths);
        }
        if slot.abandoned.load(std::sync::atomic::Ordering::SeqCst) {
            return Some(finish_abandoned_workspace_child(task_id, root).await);
        }
        if cancel.is_cancelled() {
            return None;
        }
        tokio::select! {
            _ = &mut notified => {}
            _ = cancel.cancelled() => return None,
        }
    }
}

async fn finish_abandoned_workspace_child(task_id: &str, root: &Path) -> Vec<String> {
    let watch = {
        let slots = lock_slots();
        let Some(slot) = slots.get(task_id).cloned() else {
            return Vec::new();
        };
        let watch = slot
            .watch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        watch
    };
    let paths = if let Some(watch) = watch {
        watch.finish(root).await
    } else {
        Vec::new()
    };
    lock_slots().remove(task_id);
    paths
}

pub(crate) fn snapshot_from_parts(
    porcelain: Option<Vec<String>>,
    head: Option<String>,
    stamps: Vec<ContentStamp>,
) -> Snapshot {
    // A stored head without porcelain means status missed. That is not an
    // empty tree, and it must not be reread as a non-git walk.
    let git_worktree = porcelain.is_some() || head.is_some();
    let stamps_incomplete = porcelain.is_none() && head.is_some();
    Snapshot {
        porcelain,
        head,
        stamps,
        stamps_incomplete,
        git_worktree,
    }
}

#[cfg(test)]
mod receipt_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_reread_is_not_an_empty_mutation() {
        let before = vec!["?? guest.txt".to_string()];
        let observed = combine_delta(Some(&before), None, Some("abc"), None, None);
        assert!(observed.incomplete);
        assert!(observed.paths.is_empty());
    }

    #[test]
    fn a_quoted_porcelain_path_is_a_mutation() {
        let after = vec!["?? \"my file.txt\"".to_string()];
        let parsed = diff_porcelain(&[], &after);
        assert!(!parsed.incomplete);
        assert_eq!(parsed.paths, vec!["my file.txt".to_string()]);
        let octal = vec!["?? \"\\350\\257\\264\\346\\230\\216.md\"".to_string()];
        let decoded = diff_porcelain(&[], &octal);
        assert!(!decoded.incomplete);
        assert_eq!(decoded.paths, vec!["说明.md".to_string()]);
    }

    #[test]
    fn an_undecodable_quoted_path_is_not_an_empty_mutation() {
        let after = vec!["?? \"unterminated".to_string()];
        let parsed = diff_porcelain(&[], &after);
        assert!(parsed.incomplete);
        assert!(parsed.paths.is_empty());
    }

    #[test]
    fn a_renamed_quoted_pair_records_both_paths() {
        let after = vec!["R  \"old name.txt\" -> \"new name.txt\"".to_string()];
        let parsed = diff_porcelain(&[], &after);
        assert!(!parsed.incomplete);
        assert_eq!(
            parsed.paths,
            vec!["new name.txt".to_string(), "old name.txt".to_string()]
        );
    }

    #[test]
    fn hidden_index_tags_are_paths_and_ordinary_tags_are_not() {
        let output = b"H tracked.txt\0S skipped.txt\0h assumed.txt\0s both.txt\0";
        let (paths, incomplete) = parse_hidden_index_paths(output);
        assert!(!incomplete);
        assert_eq!(
            paths,
            vec![
                "skipped.txt".to_string(),
                "assumed.txt".to_string(),
                "both.txt".to_string()
            ]
        );
    }

    #[test]
    fn nul_status_keeps_a_path_with_a_space() {
        let output = b"?? my file.txt\0?? \xe8\xaf\xb4\xe6\x98\x8e.md\0";
        let (records, incomplete) = porcelain_records(output);
        assert!(!incomplete);
        let parsed = diff_porcelain(&[], &records);
        assert!(parsed.paths.iter().any(|path| path == "my file.txt"));
        assert!(parsed.paths.iter().any(|path| path == "说明.md"));
    }

    #[test]
    fn porcelain_keeps_the_suffix_past_thirty_two_paths() {
        let after: Vec<String> = (0..33).map(|index| format!("?? f{index:02}.txt")).collect();
        let paths = changed_paths(&[], &after);
        assert_eq!(paths.len(), 33);
        assert!(paths.iter().any(|path| path == "f32.txt"));
    }

    #[test]
    fn a_clean_revision_path_is_not_kept_when_git_was_not_invoked() {
        let mut paths = vec!["guest.txt".to_string(), "local.txt".to_string()];
        drop_foreign_revision(
            &mut paths,
            &["guest.txt".to_string()],
            &["local.txt".to_string()],
        );
        assert_eq!(paths, vec!["local.txt".to_string()]);
    }

    #[test]
    fn a_revision_path_left_dirty_stays_on_the_command() {
        let mut paths = vec!["guest.txt".to_string()];
        drop_foreign_revision(
            &mut paths,
            &["guest.txt".to_string()],
            &["guest.txt".to_string()],
        );
        assert_eq!(paths, vec!["guest.txt".to_string()]);
    }

    #[test]
    fn a_missed_status_with_a_known_head_is_not_an_empty_tree() {
        let before = snapshot_from_parts(None, Some("abc".to_string()), Vec::new());
        assert!(before.observes_git());
        assert!(before.stamps_incomplete);
    }

    #[test]
    fn a_missed_status_outside_git_is_not_a_git_observation() {
        let before = snapshot_from_parts(None, None, Vec::new());
        assert!(!before.observes_git());
        assert!(!before.stamps_incomplete);
    }

    #[tokio::test]
    async fn an_unchanged_git_worktree_is_a_complete_empty_delta() {
        let root = tempfile::tempdir().unwrap();
        git_repo(root.path());
        std::fs::write(root.path().join("README.md"), "base\n").unwrap();
        git_at(root.path(), &["add", "README.md"]);
        git_at(root.path(), &["commit", "-m", "base"]);

        let watch = Watch::start(root.path()).await;
        assert!(watch.snapshot.observes_git());
        assert!(!watch.snapshot.stamps_incomplete);
        assert!(watch.files.is_none());
        let paths = watch.finish(root.path()).await;
        assert!(
            paths.is_empty(),
            "an unchanged worktree was reported as a mutation: {paths:?}"
        );
    }

    #[tokio::test]
    async fn a_foreign_commit_is_not_a_command_mutation_unless_this_command_invoked_git() {
        let root = tempfile::tempdir().unwrap();
        git_repo(root.path());
        std::fs::write(root.path().join("README.md"), "base\n").unwrap();
        git_at(root.path(), &["add", "README.md"]);
        git_at(root.path(), &["commit", "-m", "base"]);

        let watch = Watch::start(root.path()).await;
        std::fs::write(root.path().join("guest.txt"), "foreign\n").unwrap();
        git_at(root.path(), &["add", "guest.txt"]);
        git_at(root.path(), &["commit", "-m", "foreign"]);
        let foreign = watch.finish_with_revision(root.path(), false).await;
        assert!(
            !foreign.iter().any(|path| path == "guest.txt"),
            "parallel commit entered the command delta: {foreign:?}"
        );

        let watch = Watch::start(root.path()).await;
        std::fs::write(root.path().join("owned.txt"), "owned\n").unwrap();
        git_at(root.path(), &["add", "owned.txt"]);
        git_at(root.path(), &["commit", "-m", "owned"]);
        let owned = watch.finish_with_revision(root.path(), true).await;
        assert!(
            owned.iter().any(|path| path == "owned.txt"),
            "this command's commit was dropped: {owned:?}"
        );

        let watch = Watch::start(root.path()).await;
        std::fs::write(root.path().join("local.txt"), "local\n").unwrap();
        let local = watch.finish_with_revision(root.path(), false).await;
        assert!(
            local.iter().any(|path| path == "local.txt"),
            "an uncommitted write was dropped: {local:?}"
        );
    }

    fn git_repo(root: &Path) {
        git_at(root, &["init"]);
        git_at(root, &["config", "user.email", "test@example.com"]);
        git_at(root, &["config", "user.name", "test"]);
        git_at(root, &["config", "commit.gpgsign", "false"]);
    }

    fn git_at(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap_or_else(|error| panic!("git {args:?} failed to spawn: {error}"));
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
