//! Static context provider for session-local context items.

use super::{ContextItem, ContextProvider, ContextQuery, ContextResult};
use crate::text::truncate_utf8;

/// Provides pre-built context items through the same retrieval pipeline as
/// external providers.
#[derive(Debug, Clone)]
pub struct StaticContextProvider {
    name: String,
    items: Vec<ContextItem>,
}

impl StaticContextProvider {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            items: Vec::new(),
        }
    }

    pub fn from_items(
        name: impl Into<String>,
        items: impl IntoIterator<Item = ContextItem>,
    ) -> Self {
        Self {
            name: name.into(),
            items: items.into_iter().collect(),
        }
    }

    pub fn with_item(mut self, item: ContextItem) -> Self {
        self.items.push(item);
        self
    }
}

#[async_trait::async_trait]
impl ContextProvider for StaticContextProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn query(&self, query: &ContextQuery) -> anyhow::Result<ContextResult> {
        let mut result = ContextResult::new(&self.name);
        let max_results = query.max_results.max(1);
        let max_tokens = query.max_tokens.max(1);
        let mut total_tokens = 0usize;

        for item in self.items.iter().take(max_results) {
            let item_tokens = estimated_tokens(item);
            if item.is_required() {
                total_tokens = total_tokens.saturating_add(item_tokens);
                result.add_item(item.clone());
                continue;
            }
            if total_tokens + item_tokens > max_tokens {
                result.truncated = true;

                if result.items.is_empty() {
                    result.add_item(truncate_item(item, max_tokens));
                }

                break;
            }

            let mut item = item.clone();
            if item.token_count == 0 {
                item.token_count = item_tokens;
            }
            total_tokens += item_tokens;
            result.add_item(item);
        }

        if self.items.len() > max_results {
            result.truncated = true;
        }

        Ok(result)
    }
}

fn estimated_tokens(item: &ContextItem) -> usize {
    if item.token_count > 0 {
        item.token_count
    } else {
        item.content.split_whitespace().count().max(1)
    }
}

fn truncate_item(item: &ContextItem, max_tokens: usize) -> ContextItem {
    let max_bytes = max_tokens.saturating_mul(4).max(1);
    let mut truncated = item.clone();
    let shown = truncate_utf8(&item.content, max_bytes).trim_end();
    truncated.content = format!("{shown}\n\n[context truncated]");
    truncated.token_count = max_tokens;
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ContextType;

    #[tokio::test]
    async fn returns_static_items() {
        let provider = StaticContextProvider::new("static").with_item(
            ContextItem::new("agents_md", ContextType::Resource, "Follow local rules")
                .with_relevance(0.95),
        );

        let result = provider.query(&ContextQuery::new("prompt")).await.unwrap();

        assert_eq!(result.provider, "static");
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].id, "agents_md");
        assert!(result.items[0].token_count > 0);
        assert!(!result.truncated);
    }

    #[tokio::test]
    async fn respects_result_limit() {
        let provider = StaticContextProvider::from_items(
            "static",
            [
                ContextItem::new("a", ContextType::Resource, "a").with_token_count(1),
                ContextItem::new("b", ContextType::Resource, "b").with_token_count(1),
            ],
        );

        let result = provider
            .query(&ContextQuery::new("prompt").with_max_results(1))
            .await
            .unwrap();

        assert_eq!(result.items.len(), 1);
        assert!(result.truncated);
    }

    #[tokio::test]
    async fn truncates_oversized_single_item() {
        let provider = StaticContextProvider::new("static").with_item(
            ContextItem::new(
                "large",
                ContextType::Resource,
                "one two three four five six",
            )
            .with_token_count(6),
        );

        let result = provider
            .query(&ContextQuery::new("prompt").with_max_tokens(2))
            .await
            .unwrap();

        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].token_count, 2);
        assert!(result.items[0].content.contains("[context truncated]"));
        assert!(result.truncated);
    }

    #[tokio::test]
    async fn required_static_item_bypasses_retrieval_token_budget() {
        let provider = StaticContextProvider::new("static").with_item(
            ContextItem::new(
                "instructions",
                ContextType::Resource,
                "one two three four five six",
            )
            .with_token_count(6)
            .with_required(),
        );

        let result = provider
            .query(&ContextQuery::new("prompt").with_max_tokens(2))
            .await
            .unwrap();

        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].token_count, 6);
        assert!(!result.truncated);
    }
}
