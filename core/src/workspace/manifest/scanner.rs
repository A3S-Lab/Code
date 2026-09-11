//! Workspace file discovery and file-type classification.

use super::file_kind::is_binary_file;
use super::{
    normalize_relative_path_lossy, system_time_ms, LocalWorkspaceFile, LocalWorkspaceFileStatus,
};
use crate::language::LanguageCatalog;
use crate::workspace::source_egress::is_personal_kb_path;
use ignore::WalkBuilder;
use notify::{Event, EventKind};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);

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
    let executable = crate::git::trusted_git_executable(root).ok()?;
    let mut command = Command::new(executable);
    crate::git::configure_git_environment(&mut command, root);
    command.args(args);
    let (status, stdout) = command_stdout_cancellable(command, is_cancelled, GIT_COMMAND_TIMEOUT)?;
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
    timeout: Duration,
) -> Option<(ExitStatus, Vec<u8>)> {
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    crate::tools::process::configure_std_process_group(&mut command);
    let mut child = crate::tools::process::spawn_std_with_native_gate(&mut command).ok()?;
    let mut process_group =
        crate::tools::process::ProcessGroupGuard::for_process_id(Some(child.id()));
    let mut stdout = child.stdout.take()?;
    let reader = thread::spawn(move || crate::git::read_git_output_bounded(&mut stdout));

    let deadline = Instant::now() + timeout;
    let status = loop {
        if is_cancelled() {
            process_group.kill();
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(PROCESS_POLL_INTERVAL),
            Ok(None) => {
                process_group.kill();
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return None;
            }
            Err(_) => {
                process_group.kill();
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return None;
            }
        }
    };
    // Manifest discovery does not own a background service. Terminate any
    // helper that survived the direct Git process before joining its pipes.
    process_group.kill();
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
    append_personal_kb_files(root, &mut files, is_cancelled)?;
    Some(files)
}

/// Workspace `.gitignore` often lists `.a3s/`, which would hide the personal
/// vault from ignore-aware walks. Re-scan `.a3s/kb/` without gitignore so agent
/// search/grep can discover shared notes without admitting other `.a3s` control
/// plane files.
fn append_personal_kb_files(
    root: &Path,
    files: &mut Vec<LocalWorkspaceFile>,
    is_cancelled: &impl Fn() -> bool,
) -> Option<()> {
    let kb_root = root.join(".a3s").join("kb");
    if !kb_root.is_dir() {
        return Some(());
    }
    let walker = WalkBuilder::new(&kb_root)
        .hidden(false)
        .parents(false)
        .ignore(false)
        .git_ignore(false)
        .git_exclude(false)
        .git_global(false)
        .build();
    for entry in walker {
        if is_cancelled() {
            return None;
        }
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if path == kb_root {
            continue;
        }
        let Some(relative) = path.strip_prefix(root).ok() else {
            continue;
        };
        if !is_personal_kb_path(relative) {
            continue;
        }
        if let Some(file) = workspace_file(root, relative, LocalWorkspaceFileStatus::Unknown) {
            files.push(file);
        }
    }
    Some(())
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
    if is_personal_kb_path(path) {
        return false;
    }
    path.components().any(|component| {
        let Component::Normal(name) = component else {
            return false;
        };
        matches!(
            name.to_string_lossy().as_ref(),
            // Control-plane and package/build trees must not invalidate the
            // workspace catalog. Persistent retrieval writes under `.a3s-code/`
            // inside the workspace; treating those as source changes republishes
            // mid-query and empties hybrid hits (revision-changed). The personal
            // vault (`.a3s/kb/`) is carved out above via `is_personal_kb_path`.
            ".git"
                | ".a3s"
                | ".a3s-code"
                | "node_modules"
                | "target"
                | ".next"
                | "dist"
                | ".DS_Store"
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
    #[cfg(unix)]
    use std::sync::Arc;

    #[test]
    fn control_plane_and_package_dirs_are_noise() {
        for path in [
            ".a3s-code/index/CURRENT",
            ".a3s/os-auth.json",
            "node_modules/pkg/index.js",
            "target/debug/build.rs",
            ".git/config",
        ] {
            assert!(
                path_has_noise_component(Path::new(path)),
                "{path} should be ignored as workspace noise"
            );
        }
        assert!(!path_has_noise_component(Path::new("src/lib.rs")));
        assert!(
            !path_has_noise_component(Path::new(".a3s/kb/sources/note.md")),
            "personal KB vault must stay discoverable for agent search"
        );
    }

    #[test]
    fn scan_includes_personal_kb_even_when_dot_a3s_is_gitignored() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join(".gitignore"), ".a3s/\n").unwrap();
        std::fs::create_dir_all(workspace.path().join(".a3s/kb/sources")).unwrap();
        std::fs::write(
            workspace.path().join(".a3s/kb/sources/seeded-kb-token.md"),
            "seeded_kb_token_for_scan\n",
        )
        .unwrap();
        std::fs::write(workspace.path().join(".a3s/config.acl"), "x = 1\n").unwrap();
        std::fs::write(workspace.path().join("lib.rs"), "fn main() {}\n").unwrap();

        let files = scan_workspace_files(workspace.path());
        let paths: Vec<_> = files.iter().map(|file| file.path.as_str()).collect();
        assert!(
            paths.iter().any(|path| path.contains("seeded-kb-token.md")),
            "expected personal KB source in catalog, got {paths:?}"
        );
        assert!(
            !paths.iter().any(|path| path.contains("config.acl")),
            "control-plane files under .a3s must stay out of catalog"
        );
    }

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
    fn cancellable_command_kills_a_blocked_process_group() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let trigger = Arc::clone(&cancelled);
        let cancel_task = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            trigger.store(true, Ordering::Release);
        });
        let mut command = Command::new("sh");
        // Keep a shell leader and a separate descendant alive so the test
        // fails if cancellation kills only the direct child.
        command.args(["-c", "sleep 30 & wait"]);
        let started = std::time::Instant::now();

        let output = command_stdout_cancellable(
            command,
            &|| cancelled.load(Ordering::Acquire),
            Duration::from_secs(5),
        );

        cancel_task.join().unwrap();
        assert!(output.is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn cancellable_command_has_an_independent_deadline() {
        let directory = tempfile::tempdir().unwrap();
        let leaked = directory.path().join("timeout-leak");
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            &format!("exec 1>&- 2>&-; sleep 0.30; touch '{}'", leaked.display()),
        ]);
        let started = Instant::now();

        let output = command_stdout_cancellable(command, &|| false, Duration::from_millis(50));

        assert!(output.is_none());
        assert!(started.elapsed() < Duration::from_secs(1));
        thread::sleep(Duration::from_millis(400));
        assert!(
            !leaked.exists(),
            "a timed-out manifest command must not leave delayed side effects"
        );
    }

    #[cfg(unix)]
    #[test]
    fn completed_command_cleans_up_surviving_descendants() {
        let directory = tempfile::tempdir().unwrap();
        let leaked = directory.path().join("descendant-leak");
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            &format!("(sleep 0.30; touch '{}') &", leaked.display()),
        ]);
        let started = Instant::now();

        let output = command_stdout_cancellable(command, &|| false, Duration::from_secs(1));

        assert!(output.is_some());
        assert!(started.elapsed() < Duration::from_millis(250));
        thread::sleep(Duration::from_millis(400));
        assert!(
            !leaked.exists(),
            "a completed manifest command must clean up helper descendants"
        );
    }
}
