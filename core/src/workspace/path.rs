//! Virtual workspace path normalization and display helpers.

use super::{WorkspacePath, WorkspacePathResolver};
use anyhow::{bail, Result};
use std::path::{Component, Path, PathBuf};

/// Lexical resolver suitable for virtual/browser/DFS workspaces.
#[derive(Debug, Default)]
pub struct VirtualPathResolver;

impl WorkspacePathResolver for VirtualPathResolver {
    fn normalize(&self, input: &str) -> Result<WorkspacePath> {
        normalize_virtual_path(input)
    }
}

fn normalize_virtual_path(input: &str) -> Result<WorkspacePath> {
    let input = default_path_input(input);
    if has_windows_path_prefix(input) {
        bail!("Absolute paths are not supported by this workspace backend");
    }

    let normalized_input = input.replace('\\', "/");
    let path = Path::new(&normalized_input);
    if path.is_absolute() {
        bail!("Absolute paths are not supported by this workspace backend");
    }

    let relative = normalize_relative_path(path)?;
    Ok(pathbuf_to_workspace_path(&relative))
}

pub(super) fn default_path_input(input: &str) -> &str {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        "."
    } else {
        trimmed
    }
}

pub(super) fn has_windows_path_prefix(input: &str) -> bool {
    let bytes = input.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return true;
    }

    input.starts_with("\\\\") || input.starts_with("//")
}

pub(crate) fn validate_relative_pattern(pattern: &str, label: &str) -> Result<()> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        bail!("{label} cannot be empty");
    }
    if has_windows_path_prefix(pattern) || Path::new(pattern).is_absolute() {
        bail!("{label} must be relative to the workspace");
    }

    let normalized = pattern.replace('\\', "/");
    if normalized.split('/').any(|component| component == "..") {
        bail!("{label} must not contain parent directory traversal");
    }

    Ok(())
}

pub(super) fn normalize_relative_path(path: &Path) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => out.push(part),
            Component::ParentDir => {
                if !out.pop() {
                    bail!("Workspace boundary violation: path escapes workspace");
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                bail!("Absolute paths are not supported by this workspace backend");
            }
        }
    }
    Ok(out)
}

pub(super) fn pathbuf_to_workspace_path(path: &Path) -> WorkspacePath {
    let display = path.to_string_lossy().replace('\\', "/");
    WorkspacePath::from_normalized(display)
}

pub(crate) fn escape_control_chars_for_display(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for ch in path.chars() {
        if ch.is_control() {
            escaped.extend(ch.escape_default());
        } else {
            escaped.push(ch);
        }
    }
    escaped
}
