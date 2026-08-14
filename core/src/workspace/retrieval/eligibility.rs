use crate::workspace::LocalWorkspaceFile;
use std::path::Path;

const DEFAULT_MAX_FILE_BYTES: u64 = 512 * 1024;

/// Conservative sensitive-path rules for files retained in the local catalog.
///
/// Remote embedding egress uses a separate, explicit admission policy; this
/// local policy keeps useful text formats while excluding known secret and
/// generated locations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceEligibilityPolicy {
    pub max_file_bytes: u64,
}

impl Default for WorkspaceEligibilityPolicy {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        }
    }
}

impl WorkspaceEligibilityPolicy {
    pub fn admits(&self, file: &LocalWorkspaceFile) -> bool {
        if file.binary || file.generated || file.size > self.max_file_bytes {
            return false;
        }
        !crate::workspace::source_egress::path_is_denied(Path::new(&file.path))
    }
}
