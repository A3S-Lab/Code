//! Scope checks shared by read-only inventory and recovery admission.

use std::path::{Path, PathBuf};

use agent_client_protocol as acp;

pub(super) fn canonical_workspace(launch: &Path, requested: Option<&Path>) -> acp::Result<PathBuf> {
    let workspace = launch
        .canonicalize()
        .map_err(|_| acp::Error::resource_not_found(None))?;
    if let Some(requested) = requested.filter(|path| !path.as_os_str().is_empty()) {
        let requested = requested
            .canonicalize()
            .map_err(|_| acp::Error::resource_not_found(None))?;
        if requested != workspace {
            return Err(acp::Error::resource_not_found(None));
        }
    }
    Ok(workspace)
}
