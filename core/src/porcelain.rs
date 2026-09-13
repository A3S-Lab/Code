//! Workspace mutation paths taken from git effects, never from command text.
//!
//! Dirty-line porcelain catches uncommitted writes. A clean checkout can change
//! tracked files without leaving porcelain dirty, so a changed HEAD also
//! contributes `git diff --name-only` between the two revisions.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

const GIT_QUERY_TIMEOUT: Duration = Duration::from_millis(400);

/// Stat of a path already named by porcelain. A later write, mode change, or
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
}

#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    porcelain: Option<Vec<String>>,
    head: Option<String>,
    stamps: Vec<ContentStamp>,
    stamps_incomplete: bool,
}

impl Snapshot {
    pub(crate) fn observes_git(&self) -> bool {
        self.porcelain.is_some()
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
    pub(crate) async fn start(root: &Path) -> Self {
        let snapshot = snapshot(root).await;
        let files = if snapshot.observes_git() {
            None
        } else {
            Some(index_files(root))
        };
        Self { snapshot, files }
    }

    pub(crate) async fn finish(self, root: &Path) -> Vec<String> {
        let observed = delta(root, self.snapshot).await;
        let mut paths = observed.paths;
        if paths.is_empty() {
            if let Some(files) = self.files.as_ref() {
                let files = file_delta(root, files);
                // A truncated walk is not a complete empty mutation. Callers
                // that only publish paths must not invent one; the run gate
                // reads the incomplete flag from `RunWatch::file_changes`.
                if !files.incomplete {
                    paths = files.paths;
                }
            }
        }
        paths
    }
}

/// Paths the gate can bind, or an explicit failure to re-read the tree.
/// An incomplete read is not an empty mutation and is not a smaller digest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PathDelta {
    pub paths: Vec<String>,
    pub incomplete: bool,
}

pub(crate) async fn snapshot(root: &Path) -> Snapshot {
    let porcelain = lines(root).await;
    let (mut stamps, mut stamps_incomplete) = match &porcelain {
        Some(lines) => stamp_lines(root, lines),
        None => (Vec::new(), false),
    };
    if porcelain.is_some() {
        match hidden_index_paths(root).await {
            Some((paths, parse_incomplete)) => {
                stamps_incomplete |= parse_incomplete;
                let (extra, extra_incomplete) = stamp_paths(root, &paths);
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
    }
}

pub(crate) async fn lines(root: &Path) -> Option<Vec<String>> {
    let output = git(root, &["status", "--porcelain", "-z", "-u"]).await?;
    let (records, incomplete) = porcelain_records(&output);
    if incomplete {
        return None;
    }
    Some(records)
}

pub(crate) async fn delta(root: &Path, before: Snapshot) -> PathDelta {
    let after = snapshot(root).await;
    let names = match (&before.head, &after.head) {
        (Some(before_head), Some(after_head)) if before_head != after_head => {
            names_between(root, before_head, after_head).await
        }
        _ => Some(Vec::new()),
    };
    let mut observed = combine_delta(
        before.porcelain.as_deref(),
        after.porcelain.as_deref(),
        before.head.as_deref(),
        after.head.as_deref(),
        names,
    );
    observed.incomplete |= before.stamps_incomplete || after.stamps_incomplete;
    observed
        .paths
        .extend(content_changes(&before.stamps, &after.stamps));
    observed.paths.sort();
    observed.paths.dedup();
    observed
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

/// Non-git fallback. Git roots use [`delta`]; this only covers workspaces
/// porcelain cannot see. A truncated walk is incomplete, not an empty
/// mutation: directory order is unstable, so a prefix must not become paths.
/// Identity is path, length, mtime, mode, and kind — the same stat fields as
/// a git content stamp. A same-length overwrite that also preserves mtime,
/// mode, and kind is not hashed.
#[derive(Clone, Debug)]
struct FileStamp {
    path: String,
    len: u64,
    modified: u128,
    mode: u32,
    kind: u8,
}

#[derive(Clone, Debug)]
pub(crate) struct FileIndex {
    entries: Vec<FileStamp>,
    truncated: bool,
}

pub(crate) fn index_files(root: &Path) -> FileIndex {
    let mut entries = Vec::new();
    let mut truncated = false;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            if entries.len() >= FILE_WALK_CAP {
                truncated = true;
                break;
            }
            let name = entry.file_name();
            if name == ".git" || name == ".a3s" || name == ".a3s-code" {
                continue;
            }
            let path = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                if !metadata.file_type().is_symlink() {
                    stack.push(path);
                }
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|time| time.as_millis())
                .unwrap_or(0);
            entries.push(FileStamp {
                path: relative,
                len: metadata.len(),
                modified,
                mode: mode_bits(&metadata),
                kind: kind_bits(&metadata),
            });
        }
        if truncated {
            break;
        }
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    FileIndex { entries, truncated }
}

pub(crate) fn file_delta(root: &Path, before: &FileIndex) -> PathDelta {
    let after = index_files(root);
    if before.truncated || after.truncated {
        // Directory order is unstable past the cap. Comparing a prefix would
        // invent paths; returning a complete empty delta would hide a write.
        return PathDelta {
            paths: Vec::new(),
            incomplete: true,
        };
    }
    let before_map: HashMap<&str, (u64, u128, u32, u8)> = before
        .entries
        .iter()
        .map(|stamp| {
            (
                stamp.path.as_str(),
                (stamp.len, stamp.modified, stamp.mode, stamp.kind),
            )
        })
        .collect();
    let after_map: HashMap<&str, (u64, u128, u32, u8)> = after
        .entries
        .iter()
        .map(|stamp| {
            (
                stamp.path.as_str(),
                (stamp.len, stamp.modified, stamp.mode, stamp.kind),
            )
        })
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

fn stamp_lines(root: &Path, lines: &[String]) -> (Vec<ContentStamp>, bool) {
    let mut paths = Vec::new();
    let mut incomplete = false;
    for line in lines {
        match paths_of(line) {
            Some(found) => paths.extend(found),
            None => incomplete = true,
        }
    }
    let (stamps, stamp_incomplete) = stamp_paths(root, &paths);
    (stamps, incomplete || stamp_incomplete)
}

fn stamp_paths(root: &Path, paths: &[String]) -> (Vec<ContentStamp>, bool) {
    let mut stamps = Vec::new();
    let mut incomplete = false;
    let mut seen = HashSet::new();
    for path in paths {
        if !seen.insert(path.clone()) || is_harness_path(path) {
            continue;
        }
        match stamp_file(root, path) {
            Some(stamp) => stamps.push(stamp),
            None => incomplete = true,
        }
    }
    stamps.sort_by(|left, right| left.path.cmp(&right.path));
    (stamps, incomplete)
}

/// Paths `git status` hides. `S` is skip-worktree, `h`/`s` is assume-unchanged.
async fn hidden_index_paths(root: &Path) -> Option<(Vec<String>, bool)> {
    let output = git(root, &["ls-files", "-v", "-z"]).await?;
    Some(parse_hidden_index_paths(&output))
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
        let path = path.trim();
        if path.is_empty() || path.contains('\n') {
            incomplete = true;
            continue;
        }
        paths.push(path.to_string());
    }
    (paths, incomplete)
}

fn stamp_file(root: &Path, relative: &str) -> Option<ContentStamp> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return None;
    }
    match std::fs::symlink_metadata(root.join(relative_path)) {
        Ok(metadata) => Some(ContentStamp {
            path: relative.to_string(),
            present: true,
            len: metadata.len(),
            modified: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|time| time.as_nanos())
                .unwrap_or(0),
            mode: mode_bits(&metadata),
            kind: kind_bits(&metadata),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(ContentStamp {
            path: relative.to_string(),
            present: false,
            len: 0,
            modified: 0,
            mode: 0,
            kind: 0,
        }),
        Err(_) => None,
    }
}

fn mode_bits(metadata: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode()
    }
    #[cfg(not(unix))]
    {
        u32::from(metadata.permissions().readonly())
    }
}

fn kind_bits(metadata: &std::fs::Metadata) -> u8 {
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        2
    } else if file_type.is_dir() {
        1
    } else if file_type.is_file() {
        0
    } else {
        3
    }
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
    let path = path.trim_start_matches("./").replace('\\', "/");
    path == ".a3s"
        || path.starts_with(".a3s/")
        || path == ".a3s-code"
        || path.starts_with(".a3s-code/")
        || path == ".git"
        || path.starts_with(".git/")
        || path.contains("/.a3s/")
        || path.contains("/.a3s-code/")
        || path.contains("/.git/")
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
    let output = git(root, &["diff", "--name-only", "-z", before, after]).await?;
    let mut names = Vec::new();
    for name in output
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let name = std::str::from_utf8(name).ok()?;
        if name.is_empty() || name.contains('\n') {
            return None;
        }
        names.push(name.to_string());
    }
    Some(names)
}

async fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = tokio::time::timeout(
        GIT_QUERY_TIMEOUT,
        tokio::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(output.stdout)
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
        let Ok(text) = std::str::from_utf8(part) else {
            incomplete = true;
            continue;
        };
        if text.len() < 4 {
            incomplete = true;
            continue;
        }
        let status = &text[..2];
        let path = text[3..].trim();
        let renamed = status.contains('R') || status.contains('C');
        if renamed {
            let Some(dest) = parts.next() else {
                incomplete = true;
                if !path.is_empty() {
                    records.push(format!("{status} {path}"));
                }
                continue;
            };
            let Ok(dest) = std::str::from_utf8(dest) else {
                incomplete = true;
                continue;
            };
            let dest = dest.trim();
            if dest.is_empty() {
                incomplete = true;
            } else {
                records.push(format!("{status} {dest}"));
            }
            if !path.is_empty() {
                records.push(format!("D  {path}"));
            }
            continue;
        }
        if path.is_empty() {
            incomplete = true;
            continue;
        }
        records.push(format!("{status} {path}"));
    }
    (records, incomplete)
}

/// Paths named by one porcelain line. `None` means the line could not be
/// decoded, which is not the same as “no path”.
fn paths_of(line: &str) -> Option<Vec<String>> {
    if line.len() < 4 {
        return None;
    }
    let status = &line[..2];
    let body = line[3..].trim();
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
        if body[index..].starts_with(" -> ") {
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
        }),
    );
}

/// Reserve before the child can write. The gate observes this id, not command text.
pub(crate) fn reserve_workspace_child(task_id: &str) {
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
        }),
    );
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
}

pub(crate) fn snapshot_from_parts(
    porcelain: Option<Vec<String>>,
    head: Option<String>,
    stamps: Vec<ContentStamp>,
) -> Snapshot {
    Snapshot {
        porcelain,
        head,
        stamps,
        stamps_incomplete: false,
    }
}
