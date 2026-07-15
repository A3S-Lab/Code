//! Live context provider for the session skill catalog.

use super::{
    ContextItem, ContextProvider, ContextQuery, ContextResult, ContextType, StaticContextProvider,
};
use crate::skills::SkillRegistry;
use std::sync::Arc;

/// Supplies the model-visible skill discovery hint from the current registry.
///
/// The provider keeps the registry handle rather than a rendered string, so a
/// skill added or removed after session creation is reflected on the next
/// context query without rebuilding the session.
#[derive(Clone)]
pub struct SkillCatalogContextProvider {
    skill_registry: Arc<SkillRegistry>,
}

impl SkillCatalogContextProvider {
    pub fn new(skill_registry: Arc<SkillRegistry>) -> Self {
        Self { skill_registry }
    }
}

#[async_trait::async_trait]
impl ContextProvider for SkillCatalogContextProvider {
    fn name(&self) -> &str {
        "skills_catalog"
    }

    async fn query(&self, query: &ContextQuery) -> anyhow::Result<ContextResult> {
        let skill_prompt = self.skill_registry.to_system_prompt();
        if skill_prompt.is_empty() {
            return Ok(ContextResult::new(self.name()));
        }

        let item = ContextItem::new("skills_catalog", ContextType::Skill, skill_prompt)
            .with_source("a3s://skills/catalog")
            .with_provenance("skill_registry")
            .with_priority(0.85)
            .with_trust(0.9)
            .with_freshness(1.0)
            .with_relevance(1.0);
        StaticContextProvider::new(self.name())
            .with_item(item)
            .query(query)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{Skill, SkillKind};

    fn skill() -> Arc<Skill> {
        Arc::new(Skill {
            name: "live-report".to_string(),
            description: "Build a report".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "Build a report".to_string(),
            tags: vec![],
            version: None,
        })
    }

    #[tokio::test]
    async fn reflects_registry_changes_on_each_query() {
        let registry = Arc::new(SkillRegistry::new());
        let provider = SkillCatalogContextProvider::new(Arc::clone(&registry));
        let query = ContextQuery::new("create a report");

        assert!(provider.query(&query).await.unwrap().is_empty());

        registry.register_unchecked(skill());
        let added = provider.query(&query).await.unwrap();
        assert_eq!(added.items.len(), 1);
        assert_eq!(added.items[0].id, "skills_catalog");

        registry.remove("live-report");
        assert!(provider.query(&query).await.unwrap().is_empty());
    }
}
