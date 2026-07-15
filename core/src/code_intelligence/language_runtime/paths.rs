use std::{
    path::{Component, Path},
    str::FromStr,
};

use lsp_types::Uri;
use url::Url;

use super::super::{
    language_profile::LanguageServerProfile, lsp::router::WorkspaceFolder,
    project_layout::ProjectLayout, CodePosition,
};
use super::LanguageRuntimeError;
use crate::workspace::WorkspacePath;

pub(super) async fn validate_canonical_root(root: &Path) -> Result<(), LanguageRuntimeError> {
    if !root.is_absolute() {
        return Err(LanguageRuntimeError::InvalidRoot {
            root: root.to_path_buf(),
            message: "the path is not absolute".to_owned(),
        });
    }
    let resolved =
        tokio::fs::canonicalize(root)
            .await
            .map_err(|error| LanguageRuntimeError::InvalidRoot {
                root: root.to_path_buf(),
                message: format!("the path cannot be resolved: {error}"),
            })?;
    if resolved != root {
        return Err(LanguageRuntimeError::InvalidRoot {
            root: root.to_path_buf(),
            message: format!("the canonical path is {resolved:?}"),
        });
    }
    let metadata =
        tokio::fs::metadata(root)
            .await
            .map_err(|error| LanguageRuntimeError::InvalidRoot {
                root: root.to_path_buf(),
                message: format!("the path metadata cannot be read: {error}"),
            })?;
    if !metadata.is_dir() {
        return Err(LanguageRuntimeError::InvalidRoot {
            root: root.to_path_buf(),
            message: "the path is not a directory".to_owned(),
        });
    }
    Ok(())
}

pub(super) fn directory_url(path: &Path) -> Result<Url, LanguageRuntimeError> {
    Url::from_directory_path(path).map_err(|()| LanguageRuntimeError::InvalidRoot {
        root: path.to_path_buf(),
        message: "the path cannot be represented as a file URI".to_owned(),
    })
}

pub(super) async fn workspace_folders(
    profile: &LanguageServerProfile,
    canonical_root: &Path,
    layout: &ProjectLayout,
) -> Result<Vec<WorkspaceFolder>, LanguageRuntimeError> {
    let mut folders = Vec::new();
    for root in profile.project_roots(layout) {
        validate_workspace_path_or_root(&root)?;
        let directory = if root.is_root() {
            canonical_root.to_path_buf()
        } else {
            canonical_root.join(root.as_str())
        };
        let resolved = tokio::fs::canonicalize(&directory).await.map_err(|error| {
            LanguageRuntimeError::InvalidPath {
                path: root.clone(),
                message: format!("the project root cannot be resolved: {error}"),
            }
        })?;
        if !resolved.starts_with(canonical_root) {
            return Err(LanguageRuntimeError::InvalidPath {
                path: root,
                message: "the project root resolves outside the workspace".to_owned(),
            });
        }
        if !tokio::fs::metadata(&resolved)
            .await
            .map_err(|error| LanguageRuntimeError::InvalidPath {
                path: root.clone(),
                message: format!("the project root metadata cannot be read: {error}"),
            })?
            .is_dir()
        {
            return Err(LanguageRuntimeError::InvalidPath {
                path: root,
                message: "the project root is not a directory".to_owned(),
            });
        }
        let name = resolved
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("workspace");
        folders.push(WorkspaceFolder::new(directory_url(&resolved)?, name));
    }
    Ok(folders)
}

pub(super) fn validate_workspace_path(path: &WorkspacePath) -> Result<(), LanguageRuntimeError> {
    if path.is_root() {
        return Err(LanguageRuntimeError::InvalidPath {
            path: path.clone(),
            message: "a source document path cannot be the workspace root".to_owned(),
        });
    }
    validate_workspace_path_or_root(path)
}

fn validate_workspace_path_or_root(path: &WorkspacePath) -> Result<(), LanguageRuntimeError> {
    let raw = Path::new(path.as_str());
    let valid = path.is_root()
        || (!raw.is_absolute()
            && !path.as_str().is_empty()
            && raw
                .components()
                .all(|component| matches!(component, Component::Normal(_))));
    if valid {
        Ok(())
    } else {
        Err(LanguageRuntimeError::InvalidPath {
            path: path.clone(),
            message: "the path is not a normalized workspace-relative path".to_owned(),
        })
    }
}

/// Construct a lexical URI for watcher events, including deleted paths.
pub(super) fn workspace_file_uri(
    canonical_root: &Path,
    path: &WorkspacePath,
) -> Result<Uri, LanguageRuntimeError> {
    validate_workspace_path(path)?;
    let file = canonical_root.join(path.as_str());
    let url = Url::from_file_path(&file).map_err(|()| LanguageRuntimeError::InvalidPath {
        path: path.clone(),
        message: "the path cannot be represented as a file URI".to_owned(),
    })?;
    Uri::from_str(url.as_str()).map_err(|error| LanguageRuntimeError::InvalidPath {
        path: path.clone(),
        message: format!("the generated file URI is invalid: {error}"),
    })
}

/// Validate an existing saved source before exposing its lexical URI.
pub(super) async fn existing_workspace_file_uri(
    canonical_root: &Path,
    path: &WorkspacePath,
) -> Result<Uri, LanguageRuntimeError> {
    validate_workspace_path(path)?;
    let lexical_path = canonical_root.join(path.as_str());
    let resolved = tokio::fs::canonicalize(&lexical_path)
        .await
        .map_err(|error| LanguageRuntimeError::InvalidPath {
            path: path.clone(),
            message: format!("the saved source path cannot be resolved: {error}"),
        })?;
    if !resolved.starts_with(canonical_root) {
        return Err(LanguageRuntimeError::InvalidPath {
            path: path.clone(),
            message: "the saved source path resolves outside the workspace".to_owned(),
        });
    }
    let metadata = tokio::fs::metadata(&resolved).await.map_err(|error| {
        LanguageRuntimeError::InvalidPath {
            path: path.clone(),
            message: format!("the saved source metadata cannot be read: {error}"),
        }
    })?;
    if !metadata.is_file() {
        return Err(LanguageRuntimeError::InvalidPath {
            path: path.clone(),
            message: "the saved source path is not a regular file".to_owned(),
        });
    }
    workspace_file_uri(canonical_root, path)
}

pub(super) fn valid_utf16_position(content: &str, position: CodePosition) -> bool {
    let Some(line) = text_line(content, position.line) else {
        return false;
    };
    let target = u64::from(position.character);
    let mut offset = 0_u64;
    if target == 0 {
        return true;
    }
    for character in line.chars() {
        offset += character.len_utf16() as u64;
        if offset == target {
            return true;
        }
        if offset > target {
            return false;
        }
    }
    false
}

fn text_line(content: &str, requested: u32) -> Option<&str> {
    let bytes = content.as_bytes();
    let mut line = 0_u32;
    let mut start = 0_usize;
    let mut index = 0_usize;
    while index < bytes.len() {
        if !matches!(bytes[index], b'\n' | b'\r') {
            index += 1;
            continue;
        }
        if line == requested {
            return Some(&content[start..index]);
        }
        if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
            index += 1;
        }
        index += 1;
        start = index;
        line = line.checked_add(1)?;
    }
    (line == requested).then_some(&content[start..])
}
