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
use crate::workspace::source_egress;
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
    source_egress: bool,
    name: &'static str,
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
            .filter_map(|path| {
                let metadata = std::fs::metadata(&path).ok()?;
                metadata
                    .is_file()
                    .then(|| FileIdentity::from_path(&path, &metadata))?
            })
            .collect();

        let denied_hardlink_paths = workspace_hardlink_paths(workspace)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|path| path.strip_prefix(workspace).ok().map(Path::to_path_buf))
            .collect();

        Self {
            sensitive_file_ids,
            denied_hardlink_paths,
            source_egress: false,
            name: "credential",
        }
    }

    /// Construct an O(1) boundary for source sent to an external embedding
    /// provider. Unlike the interactive credential boundary, it does not scan
    /// the workspace at construction time. It rejects every multi-link source
    /// file at the actual read instead.
    pub(crate) fn for_source_egress() -> Self {
        Self {
            sensitive_file_ids: HashSet::new(),
            denied_hardlink_paths: HashSet::new(),
            source_egress: true,
            name: "source egress",
        }
    }

    pub(crate) fn ensure_access(
        &self,
        workspace: &Path,
        logical_path: &Path,
        resolved_path: Option<&Path>,
        metadata: Option<&Metadata>,
        opened_hard_link_count: Option<u64>,
        operation: &'static str,
    ) -> Result<()> {
        if self.path_is_denied(logical_path) {
            return self.denied(operation);
        }
        if self.denied_hardlink_paths.contains(logical_path) {
            return self.denied(operation);
        }

        let resolved_relative = match resolved_path {
            Some(path) => match path.strip_prefix(workspace) {
                Ok(relative) => Some(relative),
                Err(_) => return self.denied(operation),
            },
            None => None,
        };
        if resolved_relative.is_some_and(|path| self.path_is_denied(path)) {
            return self.denied(operation);
        }
        if resolved_relative.is_some_and(|path| self.denied_hardlink_paths.contains(path)) {
            return self.denied(operation);
        }

        let Some(metadata) = metadata.filter(|metadata| metadata.is_file()) else {
            return Ok(());
        };
        let checked_path = resolved_path
            .map(Path::to_path_buf)
            .unwrap_or_else(|| workspace.join(logical_path));
        let link_count =
            opened_hard_link_count.unwrap_or_else(|| hard_link_count(&checked_path, metadata));
        if link_count <= 1 {
            return Ok(());
        }
        if self.source_egress {
            return self.denied(operation);
        }

        let relative = resolved_relative.unwrap_or(logical_path);
        let Some(identity) = FileIdentity::from_path(&checked_path, metadata) else {
            return self.denied(operation);
        };
        let aliases_known_sensitive = self.sensitive_file_ids.contains(&identity);
        let inside_package_or_build_tree = is_skipped_workspace_tree(relative);

        // Source-tree multi-link files are denied conservatively because one
        // alias may live outside the workspace. Package/build stores commonly
        // use legitimate hard links, so they remain readable unless a
        // discovered credential identity proves the inode is sensitive.
        if aliases_known_sensitive || !inside_package_or_build_tree {
            return self.denied(operation);
        }

        Ok(())
    }

    fn path_is_denied(&self, path: &Path) -> bool {
        if self.source_egress {
            source_egress::path_is_denied(path)
        } else {
            is_sensitive_workspace_path(path)
        }
    }

    fn denied(&self, operation: &'static str) -> Result<()> {
        bail!(
            "local workspace {} boundary denied {operation} access",
            self.name
        )
    }
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
    fn from_path(_path: &Path, metadata: &Metadata) -> Option<Self> {
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
    fn from_path(path: &Path, _metadata: &Metadata) -> Option<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };

        let file = std::fs::File::open(path).ok()?;
        let mut information = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
        // SAFETY: `file` owns a valid handle for this call and `information`
        // points to writable storage of the required type.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
            return None;
        }
        Some(Self {
            volume: information.dwVolumeSerialNumber,
            index: (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow),
        })
    }
}

#[cfg(not(any(unix, windows)))]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct FileIdentity;

#[cfg(not(any(unix, windows)))]
impl FileIdentity {
    fn from_path(_path: &Path, _metadata: &Metadata) -> Option<Self> {
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
