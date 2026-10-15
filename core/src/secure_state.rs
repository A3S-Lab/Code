//! Permission enforcement for sensitive local state.
//!
//! Conversation transcripts, session snapshots, effect logs, and durable
//! memory carry everything the agent has seen and said; a world-class
//! harness refuses to leave them group- or other-readable. The pattern is
//! the Moli cache's `validate_cache_directory`: on every open, a state
//! directory that leaks group/other bits is tightened to 0700, and state
//! files are created 0600. Symlinks are never followed or tightened — a
//! linked state directory is a different tool's surface.

use std::path::Path;

/// Tighten a state directory to 0700 when it leaks group/other bits.
/// Creating the parent chain stays the caller's job (`create_dir_all` first,
/// then this). A symlink path is left untouched.
pub(crate) fn restrict_state_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Ok(());
        }
        let mut permissions = metadata.permissions();
        if permissions.mode() & 0o077 != 0 {
            permissions.set_mode(0o700);
            std::fs::set_permissions(path, permissions)?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Restrict a state file to 0600. Apply right after creation (before the
/// content lands, when practical) or after a completed write.
pub(crate) fn restrict_state_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Ok(());
        }
        let mut permissions = metadata.permissions();
        if permissions.mode() & 0o777 != 0o600 {
            permissions.set_mode(0o600);
            std::fs::set_permissions(path, permissions)?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn tightens_leaking_directories_and_files_and_skips_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("state");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        restrict_state_dir(&dir).unwrap();
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let file = dir.join("notes.json");
        std::fs::write(&file, b"x").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        restrict_state_file(&file).unwrap();
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );

        // Already-tight state is left alone (idempotent), and a symlink is
        // never followed or tightened.
        restrict_state_dir(&dir).unwrap();
        let linked = root.path().join("linked");
        std::os::unix::fs::symlink(&dir, &linked).unwrap();
        restrict_state_dir(&linked).unwrap();
        let target_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            target_mode, 0o700,
            "symlink tighten must not touch the target"
        );
    }
}
