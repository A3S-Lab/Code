//! Recent workspace files context provider.

use super::{ContextItem, ContextProvider, ContextQuery, ContextResult, ContextType};
use crate::workspace::LocalWorkspaceManifest;
use std::sync::Arc;

const DEFAULT_RECENT_CONTEXT_LIMIT: usize = 12;
const MAX_RECENT_CONTEXT_LIMIT: usize = 32;

/// Injects a compact, high-freshness list of recently touched workspace files.
///
/// The provider does not read file contents. It gives the model a cheap
/// navigation hint so prompts like "update this file" or "what about the file I
/// just opened?" can often skip an exploratory glob/list round.
#[derive(Clone)]
pub struct RecentWorkspaceFilesContextProvider {
    manifest: Arc<LocalWorkspaceManifest>,
    limit: usize,
}

impl RecentWorkspaceFilesContextProvider {
    pub fn new(manifest: Arc<LocalWorkspaceManifest>) -> Self {
        Self {
            manifest,
            limit: DEFAULT_RECENT_CONTEXT_LIMIT,
        }
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit.clamp(1, MAX_RECENT_CONTEXT_LIMIT);
        self
    }
}

#[async_trait::async_trait]
impl ContextProvider for RecentWorkspaceFilesContextProvider {
    fn name(&self) -> &str {
        "recent_workspace_files"
    }

    async fn query(&self, query: &ContextQuery) -> anyhow::Result<ContextResult> {
        if !query.context_types.is_empty() && !query.context_types.contains(&ContextType::Resource)
        {
            return Ok(ContextResult::new(self.name()));
        }

        let limit = self.limit.min(query.max_results.max(1));
        let entries = self.manifest.recent_file_entries(limit);
        if entries.is_empty() {
            return Ok(ContextResult::new(self.name()));
        }

        let mut content = String::from(
            "Recently touched workspace files, ranked by recency and repeated use. \
Use these as navigation hints only; verify contents with read before editing.\n",
        );
        for (index, entry) in entries.iter().enumerate() {
            content.push_str(&format!(
                "{}. {} (score {:.3}, touches {})\n",
                index + 1,
                entry.path,
                entry.score,
                entry.touch_count
            ));
        }

        let token_count = content.split_whitespace().count().max(1);
        let freshness = entries
            .first()
            .map(|entry| entry.score.clamp(0.0, 1.0))
            .unwrap_or_default();
        let item = ContextItem::new("workspace_recent_files", ContextType::Resource, content)
            .with_source("a3s://workspace/recent-files")
            .with_provenance("workspace_recent_files")
            .with_relevance(0.72)
            .with_priority(0.62)
            .with_trust(0.70)
            .with_freshness(freshness)
            .with_token_count(token_count);

        let mut result = ContextResult::new(self.name());
        result.add_item(item);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_recent_files_as_context() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("src.rs"), "fn main() {}\n").unwrap();
        let manifest = LocalWorkspaceManifest::start(temp.path());
        let mut rx = manifest.subscribe();
        tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();

        manifest.touch_file("src.rs");
        let provider = RecentWorkspaceFilesContextProvider::new(manifest);
        let result = provider
            .query(&ContextQuery::new("work on this"))
            .await
            .unwrap();

        assert_eq!(result.provider, "recent_workspace_files");
        assert_eq!(result.items.len(), 1);
        assert!(result.items[0].content.contains("src.rs"));
        assert_eq!(
            result.items[0].source.as_deref(),
            Some("a3s://workspace/recent-files")
        );
    }
}
