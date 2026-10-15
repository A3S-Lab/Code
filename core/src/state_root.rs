//! Where per-workspace harness state lives.
//!
//! By default session folders, effect logs, and the bash git witness sit
//! under `<workspace>/.a3s`. A host whose workspace tree is itself the
//! graded output (an unattended evaluation) sets one process-wide root
//! before it opens any session, so the harness leaves no state in that
//! tree. The override is set at most once. Interactive hosts never set it,
//! and their paths stay exactly `<workspace>/.a3s`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};

static OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Route harness state for every workspace in this process to `root`.
///
/// `root` must be an absolute, existing, non-symlink directory. Setting the
/// same root again is a no-op; a different root is refused.
pub fn set_state_root_override(root: &Path) -> Result<()> {
    if !root.is_absolute() {
        bail!(
            "state root override must be an absolute path: {}",
            root.display()
        );
    }
    let metadata = std::fs::symlink_metadata(root)
        .with_context(|| format!("inspect state root override {}", root.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "state root override must be a real directory: {}",
            root.display()
        );
    }
    let root = root.to_path_buf();
    if OVERRIDE.set(root.clone()).is_ok() || OVERRIDE.get() == Some(&root) {
        return Ok(());
    }
    bail!(
        "state root override is already set to {}",
        OVERRIDE
            .get()
            .map(|set| set.display().to_string())
            .unwrap_or_default()
    )
}

/// The process-wide override, when a host set one.
pub fn state_root_override() -> Option<&'static Path> {
    OVERRIDE.get().map(PathBuf::as_path)
}

/// The directory that holds harness state for `workspace`.
pub fn workspace_state_dir(workspace: &Path) -> PathBuf {
    state_dir_for(workspace, state_root_override())
}

fn state_dir_for(workspace: &Path, root: Option<&Path>) -> PathBuf {
    match root {
        Some(root) => root.to_path_buf(),
        None => workspace.join(".a3s"),
    }
}

/// The directory that holds derived workspace indexes: the `a3s-vec` FTS
/// projection and the grep trigram stamp. Without an override it is
/// `<workspace>/.a3s-code`.
pub fn workspace_index_dir(workspace: &Path) -> PathBuf {
    index_dir_for(workspace, state_root_override())
}

fn index_dir_for(workspace: &Path, root: Option<&Path>) -> PathBuf {
    use sha2::{Digest, Sha256};
    match root {
        // One process can serve several workspaces; each gets its own index.
        Some(root) => {
            let digest = Sha256::digest(workspace.to_string_lossy().as_bytes());
            let key: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
            root.join("indexes").join(key)
        }
        None => workspace.join(".a3s-code"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_an_override_state_stays_in_the_workspace() {
        let workspace = Path::new("/work/project");
        assert_eq!(
            state_dir_for(workspace, None),
            PathBuf::from("/work/project/.a3s")
        );
    }

    #[test]
    fn an_override_replaces_the_workspace_state_dir() {
        let workspace = Path::new("/work/project");
        assert_eq!(
            state_dir_for(workspace, Some(Path::new("/var/state"))),
            PathBuf::from("/var/state")
        );
    }

    #[test]
    fn indexes_stay_in_the_workspace_without_an_override() {
        assert_eq!(
            index_dir_for(Path::new("/work/project"), None),
            PathBuf::from("/work/project/.a3s-code")
        );
    }

    #[test]
    fn an_override_gives_each_workspace_its_own_index_dir() {
        let root = Path::new("/var/state");
        let one = index_dir_for(Path::new("/work/one"), Some(root));
        let two = index_dir_for(Path::new("/work/two"), Some(root));
        assert!(one.starts_with("/var/state/indexes"));
        assert_ne!(one, two);
        assert_eq!(one, index_dir_for(Path::new("/work/one"), Some(root)));
    }
}
