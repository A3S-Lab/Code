//! Workspace file discovery and file-type classification.

use super::{
    normalize_relative_path_lossy, system_time_ms, LocalWorkspaceFile, LocalWorkspaceFileStatus,
};
use ignore::WalkBuilder;
use notify::{Event, EventKind};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

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
