//! Local workspace credential boundary shared by built-in file operations.
//!
//! The process sandbox protects shell commands, but local built-in tools do
//! not execute inside that process. Hosts that select
//! [`LocalWorkspaceAccessPolicy::CredentialBoundary`] therefore need the same
//! credential and hard-link rules at the workspace backend itself.

use crate::sandbox::srt::{
    hard_link_count, sensitive_paths, should_skip_workspace_scan_directory,
    workspace_hardlink_paths, workspace_sensitive_paths,
};
use anyhow::{bail, Result};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::Metadata;
use std::path::{Component, Path, PathBuf};

/// Optional access policy for the local workspace backend.
///
/// `Unrestricted` preserves the embeddable Core API's historical behavior.
/// Interactive hosts can opt into `CredentialBoundary` to keep direct file
/// tools inside the same credential boundary as sandboxed local commands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LocalWorkspaceAccessPolicy {
    #[default]
    Unrestricted,
    CredentialBoundary,
}

#[derive(Debug)]
pub(crate) struct LocalWorkspaceAccessBoundary {
    sensitive_file_ids: HashSet<FileIdentity>,
    denied_hardlink_paths: HashSet<PathBuf>,
}

impl LocalWorkspaceAccessBoundary {
    pub(crate) fn for_policy(policy: LocalWorkspaceAccessPolicy, workspace: &Path) -> Option<Self> {
        match policy {
            LocalWorkspaceAccessPolicy::Unrestricted => None,
            LocalWorkspaceAccessPolicy::CredentialBoundary => Some(Self::discover(workspace)),
        }
    }

    fn discover(workspace: &Path) -> Self {
        let mut paths = sensitive_paths();
        if let Ok(workspace_paths) = workspace_sensitive_paths(workspace) {
            paths.extend(workspace_paths);
        }
        paths.sort();
        paths.dedup();

        let sensitive_file_ids = paths
            .into_iter()
            .filter_map(|path| std::fs::metadata(path).ok())
            .filter(|metadata| metadata.is_file())
            .filter_map(|metadata| FileIdentity::from_metadata(&metadata))
            .collect();

        let denied_hardlink_paths = workspace_hardlink_paths(workspace)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|path| path.strip_prefix(workspace).ok().map(Path::to_path_buf))
            .collect();

        Self {
            sensitive_file_ids,
            denied_hardlink_paths,
        }
    }

    pub(crate) fn ensure_access(
        &self,
        workspace: &Path,
        logical_path: &Path,
        resolved_path: Option<&Path>,
        metadata: Option<&Metadata>,
        operation: &'static str,
    ) -> Result<()> {
        if is_sensitive_workspace_path(logical_path) {
            return denied(operation);
        }
        if self.denied_hardlink_paths.contains(logical_path) {
            return denied(operation);
        }

        let resolved_relative = match resolved_path {
            Some(path) => match path.strip_prefix(workspace) {
                Ok(relative) => Some(relative),
                Err(_) => return denied(operation),
            },
            None => None,
        };
        if resolved_relative.is_some_and(is_sensitive_workspace_path) {
            return denied(operation);
        }
        if resolved_relative.is_some_and(|path| self.denied_hardlink_paths.contains(path)) {
            return denied(operation);
        }

        let Some(metadata) = metadata.filter(|metadata| metadata.is_file()) else {
            return Ok(());
        };
        if hard_link_count(metadata) <= 1 {
            return Ok(());
        }

        let relative = resolved_relative.unwrap_or(logical_path);
        let identity = FileIdentity::from_metadata(metadata);
        let aliases_known_sensitive = identity
            .as_ref()
            .is_some_and(|identity| self.sensitive_file_ids.contains(identity));
        let inside_package_or_build_tree = is_skipped_workspace_tree(relative);

        // Source-tree multi-link files are denied conservatively because one
        // alias may live outside the workspace. Package/build stores commonly
        // use legitimate hard links, so they remain readable unless a
        // discovered credential identity proves the inode is sensitive.
        if aliases_known_sensitive || !inside_package_or_build_tree {
            return denied(operation);
        }

        Ok(())
    }
}

fn denied(operation: &'static str) -> Result<()> {
    bail!("local workspace credential boundary denied {operation} access")
}

fn is_sensitive_workspace_path(path: &Path) -> bool {
    let Some(components) = normalized_components(path) else {
        return true;
    };
    if components.is_empty() {
        return false;
    }

    const EXACT_PATHS: &[&[&str]] = &[
        &[".netrc"],
        &[".npmrc"],
        &[".pypirc"],
        &[".git-credentials"],
        &[".a3s", "os-auth.json"],
        &[".codex", "auth.json"],
        &[".claude", ".credentials.json"],
        &[".claude.json"],
    ];
    if EXACT_PATHS.iter().any(|expected| {
        expected.len() == components.len()
            && expected
                .iter()
                .zip(&components)
                .all(|(expected, actual)| expected.eq_ignore_ascii_case(actual))
    }) {
        return true;
    }

    for component in components {
        if component
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(".env"))
        {
            return true;
        }
        if should_skip_workspace_scan_directory(OsStr::new(component)) {
            return false;
        }
    }
    false
}

fn is_skipped_workspace_tree(path: &Path) -> bool {
    normalized_components(path).is_some_and(|components| {
        components
            .into_iter()
            .any(|component| should_skip_workspace_scan_directory(OsStr::new(component)))
    })
}

fn normalized_components(path: &Path) -> Option<Vec<&str>> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(component) => components.push(component.to_str()?),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(components)
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[cfg(unix)]
impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        Some(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct FileIdentity {
    volume: u32,
    index: u64,
}

#[cfg(windows)]
impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Option<Self> {
        use std::os::windows::fs::MetadataExt;
        Some(Self {
            volume: metadata.volume_serial_number()?,
            index: metadata.file_index()?,
        })
    }
}

#[cfg(not(any(unix, windows)))]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct FileIdentity;

#[cfg(not(any(unix, windows)))]
impl FileIdentity {
    fn from_metadata(_metadata: &Metadata) -> Option<Self> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_workspace_paths_match_nested_env_and_fixed_credentials() {
        for path in [
            ".env",
            ".env.local",
            "apps/api/.ENV.production",
            ".env-secrets/value",
            ".npmrc",
            ".a3s/os-auth.json",
            ".codex/auth.json",
        ] {
            assert!(
                is_sensitive_workspace_path(Path::new(path)),
                "{path} should be sensitive"
            );
        }
        for path in [
            "src/env.rs",
            "node_modules/pkg/.env",
            "target/debug/.env",
            ".git/objects/.env",
            ".codex/config.acl",
        ] {
            assert!(
                !is_sensitive_workspace_path(Path::new(path)),
                "{path} should remain readable"
            );
        }
    }
}
