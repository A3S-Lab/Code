pub mod ripgrep;

use std::io;
use std::path::Path;

/// Uses symlink_metadata so a dangling symlink counts, and treats unreadable as present so EACCES cannot hide a gated entry.
pub(crate) fn path_present_or_uncertain(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}
