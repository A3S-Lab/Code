use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};

/// Launcher-owned spatial binding; request metadata cannot widen this root.
#[derive(Clone, Debug)]
pub(crate) struct WorkspaceBinding {
    requested: PathBuf,
    canonical: PathBuf,
    #[cfg(unix)]
    directory: std::sync::Arc<std::fs::File>,
}

impl WorkspaceBinding {
    pub(crate) fn new(requested: &Path) -> Result<Self> {
        ensure!(
            requested.is_absolute(),
            "native plugin workspace must be absolute"
        );
        let canonical =
            std::fs::canonicalize(requested).context("native plugin workspace is unavailable")?;
        ensure!(
            canonical.is_dir(),
            "native plugin workspace is not a directory"
        );
        Ok(Self {
            requested: requested.to_owned(),
            #[cfg(unix)]
            directory: std::sync::Arc::new(std::fs::File::open(&canonical)?),
            canonical,
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.canonical
    }

    pub(crate) fn validate(&self, requested: &Path) -> Result<()> {
        ensure!(
            requested.is_absolute(),
            "native plugin workspace is not admitted"
        );
        ensure!(
            std::fs::canonicalize(&self.requested).ok().as_deref() == Some(self.root())
                && std::fs::canonicalize(requested).ok().as_deref() == Some(self.root())
                && self.canonical.is_dir(),
            "native plugin workspace is not admitted"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let accepted = self.directory.metadata()?;
            let current = std::fs::metadata(self.root())?;
            ensure!(
                (accepted.dev(), accepted.ino()) == (current.dev(), current.ino()),
                "native plugin workspace is not admitted"
            );
        }
        Ok(())
    }

    pub(crate) fn validate_current(&self) -> Result<()> {
        self.validate(&self.requested)
    }
}
