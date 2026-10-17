//! Client-held text that takes precedence over the workspace backend on reads.
//!
//! An editor host (for example an ACP client) can hold unsaved buffer text
//! for a file. The overlay lets text reads see that text while writes,
//! listings, search, and git keep using the session's own backend.

use super::{
    WorkspaceDirEntry, WorkspaceFileSystem, WorkspacePath, WorkspaceResult, WorkspaceTextRange,
    WorkspaceTextReader, WorkspaceWriteOutcome,
};
use async_trait::async_trait;
use std::sync::Arc;

/// Source of client-held file text for a session.
#[async_trait]
pub trait WorkspaceReadOverlay: Send + Sync {
    /// Return the client's text for `path`, or `None` to read the backend.
    async fn read_text(&self, path: &WorkspacePath) -> Option<String>;
}

impl WorkspaceTextRange {
    /// Slice complete text into the range a streaming reader would return.
    pub fn from_text(content: &str, offset: usize, limit: usize) -> Self {
        let lines = content.lines().collect::<Vec<_>>();
        if offset >= lines.len() {
            return Self {
                lines: Vec::new(),
                next_offset: None,
                eof: true,
                total_lines: Some(lines.len()),
            };
        }
        let end = offset.saturating_add(limit).min(lines.len());
        Self {
            lines: lines[offset..end]
                .iter()
                .map(|line| (*line).to_string())
                .collect(),
            next_offset: (end < lines.len()).then_some(end),
            eof: end == lines.len(),
            total_lines: Some(lines.len()),
        }
    }
}

pub(super) struct OverlayFileSystem {
    pub(super) overlay: Arc<dyn WorkspaceReadOverlay>,
    pub(super) inner: Arc<dyn WorkspaceFileSystem>,
}

#[async_trait]
impl WorkspaceFileSystem for OverlayFileSystem {
    async fn read_text(&self, path: &WorkspacePath) -> WorkspaceResult<String> {
        if let Some(content) = self.overlay.read_text(path).await {
            return Ok(content);
        }
        self.inner.read_text(path).await
    }

    async fn write_text(
        &self,
        path: &WorkspacePath,
        content: &str,
    ) -> WorkspaceResult<WorkspaceWriteOutcome> {
        self.inner.write_text(path, content).await
    }

    async fn list_dir(&self, path: &WorkspacePath) -> WorkspaceResult<Vec<WorkspaceDirEntry>> {
        self.inner.list_dir(path).await
    }
}

pub(super) struct OverlayTextReader {
    pub(super) overlay: Arc<dyn WorkspaceReadOverlay>,
    pub(super) inner: Option<Arc<dyn WorkspaceTextReader>>,
    pub(super) fs: Arc<dyn WorkspaceFileSystem>,
}

#[async_trait]
impl WorkspaceTextReader for OverlayTextReader {
    async fn read_text_range(
        &self,
        path: &WorkspacePath,
        offset: usize,
        limit: usize,
    ) -> WorkspaceResult<WorkspaceTextRange> {
        if let Some(content) = self.overlay.read_text(path).await {
            return Ok(WorkspaceTextRange::from_text(&content, offset, limit));
        }
        match &self.inner {
            Some(reader) => reader.read_text_range(path, offset, limit).await,
            None => {
                let content = self.fs.read_text(path).await?;
                Ok(WorkspaceTextRange::from_text(&content, offset, limit))
            }
        }
    }
}

#[cfg(test)]
#[path = "read_overlay/tests.rs"]
mod tests;
