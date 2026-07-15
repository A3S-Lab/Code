//! Workspace file discovery and file-type classification.

use super::{
    normalize_relative_path_lossy, system_time_ms, LocalWorkspaceFile, LocalWorkspaceFileStatus,
};
use crate::language::LanguageCatalog;
use ignore::WalkBuilder;
use notify::{Event, EventKind};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

pub fn scan_workspace_files(root: &Path) -> Vec<LocalWorkspaceFile> {
    let cancelled = AtomicBool::new(false);
    scan_workspace_files_cancellable(root, &cancelled).unwrap_or_default()
}

pub(super) fn scan_workspace_files_cancellable(
    root: &Path,
    cancelled: &AtomicBool,
) -> Option<Vec<LocalWorkspaceFile>> {
    scan_workspace_files_with(root, || cancelled.load(Ordering::Acquire))
}

fn scan_workspace_files_with(
    root: &Path,
    is_cancelled: impl Fn() -> bool,
) -> Option<Vec<LocalWorkspaceFile>> {
    if is_cancelled() {
        return None;
    }
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut files = scan_with_ignore(&root, &is_cancelled)?;
    if is_cancelled() {
        return None;
    }
    if let Some(paths) = git_workspace_paths(&root, &is_cancelled) {
        if is_cancelled() {
            return None;
        }
        apply_git_statuses(&root, &mut files, paths);
    }
    (!is_cancelled()).then(|| sorted_dedup(files))
}

fn git_workspace_paths(
    root: &Path,
    is_cancelled: &impl Fn() -> bool,
) -> Option<Vec<(PathBuf, LocalWorkspaceFileStatus)>> {
    let mut out = Vec::new();
    let tracked = git_ls_files(
        root,
        &["ls-files", "--cached", "--recurse-submodules", "-z"],
        is_cancelled,
    )
    .or_else(|| git_ls_files(root, &["ls-files", "--cached", "-z"], is_cancelled))?;
    out.extend(
        tracked
            .into_iter()
            .map(|path| (path, LocalWorkspaceFileStatus::Tracked)),
    );
    let untracked = git_ls_files(
        root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        is_cancelled,
    )
    .unwrap_or_default();
    out.extend(
        untracked
            .into_iter()
            .map(|path| (path, LocalWorkspaceFileStatus::Untracked)),
    );
    Some(out)
}

fn git_ls_files(
    root: &Path,
    args: &[&str],
    is_cancelled: &impl Fn() -> bool,
) -> Option<Vec<PathBuf>> {
    if is_cancelled() {
        return None;
    }
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(args);
    let (status, stdout) = command_stdout_cancellable(command, is_cancelled)?;
    if !status.success() {
        return None;
    }
    Some(
        stdout
            .split(|byte| *byte == 0)
            .filter(|raw| !raw.is_empty())
            .map(|raw| PathBuf::from(String::from_utf8_lossy(raw).into_owned()))
            .collect(),
    )
}

fn command_stdout_cancellable(
    mut command: Command,
    is_cancelled: &impl Fn() -> bool,
) -> Option<(ExitStatus, Vec<u8>)> {
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });

    let status = loop {
        if is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return None;
            }
        }
    };
    let stdout = reader.join().ok()?.ok()?;
    Some((status, stdout))
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

fn scan_with_ignore(
    root: &Path,
    is_cancelled: &impl Fn() -> bool,
) -> Option<Vec<LocalWorkspaceFile>> {
    let filter_root = root.to_path_buf();
    let walker = WalkBuilder::new(root)
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
        .build();
    let mut files = Vec::new();
    for entry in walker {
        if is_cancelled() {
            return None;
        }
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if path == root {
            continue;
        }
        let Some(relative) = path.strip_prefix(root).ok() else {
            continue;
        };
        if let Some(file) = workspace_file(root, relative, LocalWorkspaceFileStatus::Unknown) {
            files.push(file);
        }
    }
    Some(files)
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
        language: LanguageCatalog::id_for_path(Path::new(&relative)).map(str::to_string),
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
pub(super) fn path_has_noise_component(path: &Path) -> bool {
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
    if LanguageCatalog::id_for_path(path).is_some() {
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

pub(super) fn is_relevant_event(event: &Event, root: &Path) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    event.paths.iter().any(|path| {
        path.strip_prefix(root)
            .map(|relative| !path_has_noise_component(relative))
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::Arc;

    #[test]
    fn cancellable_scan_skips_a_pre_cancelled_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("lib.rs"), "fn main() {}\n").unwrap();
        let cancelled = AtomicBool::new(true);

        assert!(scan_workspace_files_cancellable(workspace.path(), &cancelled).is_none());
    }

    #[test]
    fn cancellable_scan_stops_during_traversal() {
        let workspace = tempfile::tempdir().unwrap();
        for index in 0..32 {
            std::fs::write(
                workspace.path().join(format!("file-{index}.rs")),
                "fn item() {}\n",
            )
            .unwrap();
        }
        let checks = Cell::new(0_usize);

        let result = scan_workspace_files_with(workspace.path(), || {
            checks.set(checks.get() + 1);
            checks.get() >= 5
        });

        assert!(result.is_none());
        assert_eq!(checks.get(), 5);
    }

    #[cfg(unix)]
    #[test]
    fn cancellable_command_kills_a_blocked_child() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let trigger = Arc::clone(&cancelled);
        let cancel_task = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            trigger.store(true, Ordering::Release);
        });
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30"]);
        let started = std::time::Instant::now();

        let output = command_stdout_cancellable(command, &|| cancelled.load(Ordering::Acquire));

        cancel_task.join().unwrap();
        assert!(output.is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
