//! Plugin System for A3S Code
//!
//! Provides a unified interface for loading and unloading optional tool plugins.
//! All plugins implement the [`Plugin`] trait, which gives the system a single
//! consistent surface for lifecycle management.
//!
//! # Architecture
//!
//! ```text
//! SessionOptions::plugins  ──►  PluginManager::load_all(registry, ctx)
//!                                     │
//!                         ┌───────────┴────────────┐
//!                         ▼                        ▼
//!              AgenticSearchPlugin       AgenticParsePlugin
//!              (registers agentic_search) (registers agentic_parse)
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use a3s_code_core::{SessionOptions, AgenticSearchPlugin, AgenticParsePlugin};
//!
//! let opts = SessionOptions::new()
//!     .with_plugin(AgenticSearchPlugin::new())
//!     .with_plugin(AgenticParsePlugin::new());
//! ```

use crate::skills::Skill;
use crate::tools::ToolRegistry;
use anyhow::Result;
use std::sync::Arc;

// ============================================================================
// Plugin context — passed to every plugin on load
// ============================================================================

/// Runtime context provided to plugins when they are loaded.
///
/// Gives plugins access to dependencies they may need (LLM client,
/// skill registry, document parser registry, etc.) without coupling the
/// `Plugin` trait to specific concrete types.
#[derive(Clone)]
pub struct PluginContext {
    /// LLM client — required by tools that do LLM inference (e.g. agentic_parse).
    pub llm: Option<Arc<dyn crate::llm::LlmClient>>,
    /// Document parser registry — required by document-aware tools.
    pub document_parsers: Option<Arc<crate::document_parser::DocumentParserRegistry>>,
    /// Skill registry — plugins may register companion skills here.
    pub skill_registry: Option<Arc<crate::skills::SkillRegistry>>,
}

impl PluginContext {
    pub fn new() -> Self {
        Self {
            llm: None,
            document_parsers: None,
            skill_registry: None,
        }
    }

    pub fn with_llm(mut self, llm: Arc<dyn crate::llm::LlmClient>) -> Self {
        self.llm = Some(llm);
        self
    }

    pub fn with_document_parsers(
        mut self,
        registry: Arc<crate::document_parser::DocumentParserRegistry>,
    ) -> Self {
        self.document_parsers = Some(registry);
        self
    }

    pub fn with_skill_registry(mut self, registry: Arc<crate::skills::SkillRegistry>) -> Self {
        self.skill_registry = Some(registry);
        self
    }
}

impl Default for PluginContext {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Plugin trait
// ============================================================================

/// Unified interface for all A3S Code plugins.
///
/// A plugin is a self-contained unit that registers one or more tools into a
/// `ToolRegistry` when loaded and removes them when unloaded.  This gives the
/// host application a single consistent API for managing optional capabilities.
///
/// # Implementing a plugin
///
/// ```rust,ignore
/// use a3s_code_core::plugin::{Plugin, PluginContext};
/// use a3s_code_core::tools::ToolRegistry;
/// use anyhow::Result;
/// use std::sync::Arc;
///
/// struct MyPlugin;
///
/// impl Plugin for MyPlugin {
///     fn name(&self) -> &str { "my-plugin" }
///     fn version(&self) -> &str { "0.1.0" }
///     fn tool_names(&self) -> &[&str] { &["my_tool"] }
///     fn load(&self, registry: &Arc<ToolRegistry>, _ctx: &PluginContext) -> Result<()> {
///         Ok(())
///     }
/// }
/// ```
pub trait Plugin: Send + Sync {
    /// Unique plugin identifier (kebab-case, e.g. `"agentic-search"`).
    fn name(&self) -> &str;

    /// Plugin version string (semver, e.g. `"1.0.0"`).
    fn version(&self) -> &str;

    /// Names of all tools this plugin registers.
    ///
    /// Used by `PluginManager::unload` to remove the correct tools.
    fn tool_names(&self) -> &[&str];

    /// Register this plugin's tools into `registry`.
    ///
    /// Called once when the plugin is mounted onto a session.
    fn load(&self, registry: &Arc<ToolRegistry>, ctx: &PluginContext) -> Result<()>;

    /// Remove this plugin's tools from `registry`.
    ///
    /// The default implementation unregisters every tool listed in
    /// `tool_names()`.  Override only if you need custom cleanup.
    fn unload(&self, registry: &Arc<ToolRegistry>) {
        for name in self.tool_names() {
            registry.unregister(name);
        }
    }

    /// Human-readable description shown in plugin listings.
    fn description(&self) -> &str {
        ""
    }

    /// Skills bundled with this plugin.
    ///
    /// When the plugin is loaded successfully, each skill returned here is
    /// registered into `PluginContext::skill_registry` (if one is provided).
    /// This allows the skill to appear in the system prompt and be matched
    /// against user requests automatically — no manual skill configuration
    /// is needed by the caller.
    ///
    /// Override to return plugin-specific skills. The default returns an
    /// empty list (no companion skills).
    fn skills(&self) -> Vec<Arc<Skill>> {
        vec![]
    }
}

// ============================================================================
// PluginManager
// ============================================================================

/// Manages the lifecycle of all loaded plugins for a session.
///
/// Each session owns its own `PluginManager`; plugins are not shared across
/// sessions so that sessions can have different capability sets.
#[derive(Default)]
pub struct PluginManager {
    plugins: Vec<Arc<dyn Plugin>>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a plugin. Does not load it yet.
    pub fn register(&mut self, plugin: impl Plugin + 'static) {
        self.plugins.push(Arc::new(plugin));
    }

    /// Register a pre-boxed plugin.
    pub fn register_arc(&mut self, plugin: Arc<dyn Plugin>) {
        self.plugins.push(plugin);
    }

    /// Load all registered plugins into `registry`.
    ///
    /// Plugins are loaded in registration order.  If a plugin fails to load,
    /// the error is logged and loading continues for the remaining plugins.
    ///
    /// On a successful load, the plugin's companion skills (from [`Plugin::skills`])
    /// are registered into `ctx.skill_registry` when one is provided.
    pub fn load_all(&self, registry: &Arc<ToolRegistry>, ctx: &PluginContext) {
        for plugin in &self.plugins {
            tracing::info!("Loading plugin '{}' v{}", plugin.name(), plugin.version());
            match plugin.load(registry, ctx) {
                Ok(()) => {
                    if let Some(ref skill_reg) = ctx.skill_registry {
                        for skill in plugin.skills() {
                            tracing::debug!(
                                "Plugin '{}' registered skill '{}'",
                                plugin.name(),
                                skill.name
                            );
                            skill_reg.register_unchecked(skill);
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Plugin '{}' failed to load: {}", plugin.name(), e);
                }
            }
        }
    }

    /// Unload a single plugin by name.
    ///
    /// Removes the plugin's tools from the registry and deregisters the plugin.
    pub fn unload(&mut self, name: &str, registry: &Arc<ToolRegistry>) {
        if let Some(pos) = self.plugins.iter().position(|p| p.name() == name) {
            let plugin = self.plugins.remove(pos);
            tracing::info!("Unloading plugin '{}'", plugin.name());
            plugin.unload(registry);
        }
    }

    /// Unload all plugins.
    pub fn unload_all(&mut self, registry: &Arc<ToolRegistry>) {
        for plugin in self.plugins.drain(..).rev() {
            tracing::info!("Unloading plugin '{}'", plugin.name());
            plugin.unload(registry);
        }
    }

    /// Returns `true` if a plugin with `name` is registered.
    pub fn is_loaded(&self, name: &str) -> bool {
        self.plugins.iter().any(|p| p.name() == name)
    }

    /// Number of registered plugins.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Returns `true` if no plugins are registered.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// List all registered plugin names.
    pub fn plugin_names(&self) -> Vec<&str> {
        self.plugins.iter().map(|p| p.name()).collect()
    }
}

// ============================================================================
// Built-in plugin implementations
// ============================================================================

// Skill content embedded at compile time.  Paths are relative to this file
// (core/src/plugin.rs → core/skills/).
const AGENTIC_SEARCH_SKILL_MD: &str = include_str!("../skills/agentic-search.md");
const AGENTIC_PARSE_SKILL_MD: &str = include_str!("../skills/agentic-parse.md");

fn parse_embedded_skill(content: &str, name: &str) -> Option<Arc<Skill>> {
    match Skill::parse(content) {
        Some(skill) => Some(Arc::new(skill)),
        None => {
            tracing::warn!(
                "Failed to parse embedded skill '{}' — skill will not be registered",
                name
            );
            None
        }
    }
}

// ============================================================================
// SkillPlugin — skill-only plugin (no tools)
// ============================================================================

/// A skill-only plugin that injects custom skills into the session's skill
/// registry without registering any tools.
///
/// This is the primary way to add custom LLM guidance from Python or Node.js
/// without writing Rust. Provide skill YAML/markdown content strings and they
/// will appear in the system prompt automatically.
///
/// # Example
///
/// ```rust,ignore
/// let plugin = SkillPlugin::new("my-plugin")
///     .with_skill(r#"---
/// name: my-skill
/// description: Use bash when running shell commands
/// allowed-tools: "bash(*)"
/// kind: instruction
/// ---
/// Always explain what command you're about to run before executing it."#);
///
/// let opts = SessionOptions::new().with_plugin(plugin);
/// ```
pub struct SkillPlugin {
    plugin_name: String,
    plugin_version: String,
    skill_contents: Vec<String>,
}

impl SkillPlugin {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            plugin_name: name.into(),
            plugin_version: "1.0.0".into(),
            skill_contents: vec![],
        }
    }

    pub fn with_skill(mut self, content: impl Into<String>) -> Self {
        self.skill_contents.push(content.into());
        self
    }

    pub fn with_skills(mut self, contents: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.skill_contents
            .extend(contents.into_iter().map(|s| s.into()));
        self
    }
}

impl Plugin for SkillPlugin {
    fn name(&self) -> &str {
        &self.plugin_name
    }

    fn version(&self) -> &str {
        &self.plugin_version
    }

    fn tool_names(&self) -> &[&str] {
        &[]
    }

    fn load(&self, _registry: &Arc<ToolRegistry>, _ctx: &PluginContext) -> Result<()> {
        Ok(())
    }

    fn skills(&self) -> Vec<Arc<Skill>> {
        self.skill_contents
            .iter()
            .filter_map(|content| Skill::parse(content).map(Arc::new))
            .collect()
    }
}

/// Plugin that mounts the `agentic_search` tool.
///
/// `agentic_search` is a multi-phase semantic code search tool. It reads
/// files through `ToolContext::document_parsers` when available, falling
/// back to plain-text for unknown formats.
pub struct AgenticSearchPlugin;

impl AgenticSearchPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AgenticSearchPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for AgenticSearchPlugin {
    fn name(&self) -> &str {
        "agentic-search"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn tool_names(&self) -> &[&str] {
        &["agentic_search"]
    }

    fn description(&self) -> &str {
        "Multi-phase semantic code search with IDF-weighted relevance scoring \
         and Monte Carlo deep-search mode."
    }

    fn load(&self, registry: &Arc<ToolRegistry>, _ctx: &PluginContext) -> Result<()> {
        use crate::tools::AgenticSearchTool;
        registry.register(Arc::new(AgenticSearchTool::new()));
        tracing::debug!("agentic_search tool registered");
        Ok(())
    }

    fn skills(&self) -> Vec<Arc<Skill>> {
        parse_embedded_skill(AGENTIC_SEARCH_SKILL_MD, "agentic-search")
            .into_iter()
            .collect()
    }
}

/// Plugin that mounts the `agentic_parse` tool.
///
/// `agentic_parse` uses the `DocumentParserRegistry` for binary format
/// decoding and an LLM for semantic extraction / QA.
///
/// Requires an LLM client in `PluginContext`. If none is provided the plugin
/// still loads but logs a warning; the tool will return an error at runtime.
pub struct AgenticParsePlugin;

impl AgenticParsePlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AgenticParsePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for AgenticParsePlugin {
    fn name(&self) -> &str {
        "agentic-parse"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn tool_names(&self) -> &[&str] {
        &["agentic_parse"]
    }

    fn description(&self) -> &str {
        "LLM-enhanced document parsing with structural extraction, \
         5 parse strategies, and semantic QA."
    }

    fn load(&self, registry: &Arc<ToolRegistry>, ctx: &PluginContext) -> Result<()> {
        use crate::tools::AgenticParseTool;
        let llm = ctx.llm.clone().ok_or_else(|| {
            anyhow::anyhow!("agentic-parse plugin requires an LLM client in PluginContext")
        })?;
        registry.register(Arc::new(AgenticParseTool::new(llm)));
        tracing::debug!("agentic_parse tool registered");
        Ok(())
    }

    fn skills(&self) -> Vec<Arc<Skill>> {
        parse_embedded_skill(AGENTIC_PARSE_SKILL_MD, "agentic-parse")
            .into_iter()
            .collect()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolRegistry;
    use std::path::PathBuf;

    fn make_registry() -> Arc<ToolRegistry> {
        Arc::new(ToolRegistry::new(PathBuf::from("/tmp")))
    }

    #[test]
    fn plugin_manager_register_and_query() {
        let mut mgr = PluginManager::new();
        assert!(mgr.is_empty());
        mgr.register(AgenticSearchPlugin::new());
        assert_eq!(mgr.len(), 1);
        assert!(mgr.is_loaded("agentic-search"));
        assert!(!mgr.is_loaded("agentic-parse"));
    }

    #[test]
    fn plugin_manager_load_all() {
        let mut mgr = PluginManager::new();
        mgr.register(AgenticSearchPlugin::new());
        let registry = make_registry();
        let ctx = PluginContext::new();
        mgr.load_all(&registry, &ctx);
        assert!(registry.get("agentic_search").is_some());
    }

    #[test]
    fn plugin_manager_unload() {
        let mut mgr = PluginManager::new();
        mgr.register(AgenticSearchPlugin::new());
        let registry = make_registry();
        let ctx = PluginContext::new();
        mgr.load_all(&registry, &ctx);
        assert!(registry.get("agentic_search").is_some());
        mgr.unload("agentic-search", &registry);
        assert!(registry.get("agentic_search").is_none());
        assert!(!mgr.is_loaded("agentic-search"));
    }

    #[test]
    fn plugin_manager_unload_all() {
        let mut mgr = PluginManager::new();
        mgr.register(AgenticSearchPlugin::new());
        let registry = make_registry();
        let ctx = PluginContext::new();
        mgr.load_all(&registry, &ctx);
        mgr.unload_all(&registry);
        assert!(mgr.is_empty());
        assert!(registry.get("agentic_search").is_none());
    }

    #[test]
    fn agentic_search_plugin_metadata() {
        let p = AgenticSearchPlugin::new();
        assert_eq!(p.name(), "agentic-search");
        assert_eq!(p.tool_names(), &["agentic_search"]);
        assert!(!p.description().is_empty());
    }

    #[test]
    fn agentic_search_plugin_provides_skill() {
        let p = AgenticSearchPlugin::new();
        let skills = p.skills();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "agentic-search");
        assert!(skills[0].is_tool_allowed("agentic_search"));
    }

    #[test]
    fn agentic_parse_plugin_provides_skill() {
        let p = AgenticParsePlugin::new();
        let skills = p.skills();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "agentic-parse");
        assert!(skills[0].is_tool_allowed("agentic_parse"));
    }

    #[test]
    fn plugin_skills_registered_on_load_all() {
        use crate::skills::SkillRegistry;

        let mut mgr = PluginManager::new();
        mgr.register(AgenticSearchPlugin::new());

        let registry = make_registry();
        let skill_reg = Arc::new(SkillRegistry::new());
        let ctx = PluginContext::new().with_skill_registry(Arc::clone(&skill_reg));

        mgr.load_all(&registry, &ctx);

        // Tool registered
        assert!(registry.get("agentic_search").is_some());
        // Skill registered
        assert!(skill_reg.get("agentic-search").is_some());
    }

    #[test]
    fn plugin_skills_not_registered_when_no_skill_registry_in_ctx() {
        use crate::skills::SkillRegistry;

        let mut mgr = PluginManager::new();
        mgr.register(AgenticSearchPlugin::new());

        let registry = make_registry();
        // No skill_registry in ctx
        let ctx = PluginContext::new();
        mgr.load_all(&registry, &ctx);

        // Tool still registered
        assert!(registry.get("agentic_search").is_some());
        // No crash — skill registry absence is silently tolerated
    }

    #[test]
    fn failed_plugin_skills_not_registered() {
        use crate::skills::SkillRegistry;

        // AgenticParsePlugin requires an LLM client; loading without one fails
        let mut mgr = PluginManager::new();
        mgr.register(AgenticParsePlugin::new());

        let registry = make_registry();
        let skill_reg = Arc::new(SkillRegistry::new());
        let ctx = PluginContext::new().with_skill_registry(Arc::clone(&skill_reg));
        // No LLM in ctx → load() returns Err → skills() must NOT be called

        mgr.load_all(&registry, &ctx);

        assert!(registry.get("agentic_parse").is_none());
        assert!(skill_reg.get("agentic-parse").is_none());
    }

    #[test]
    fn agentic_parse_plugin_fails_without_llm() {
        let p = AgenticParsePlugin::new();
        let registry = make_registry();
        let ctx = PluginContext::new(); // no LLM
        let result = p.load(&registry, &ctx);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("LLM client"));
    }

    #[test]
    fn skill_plugin_no_tools_and_injects_skills() {
        use crate::skills::SkillRegistry;

        let skill_md = r#"---
name: test-skill
description: Test skill
allowed-tools: "bash(*)"
kind: instruction
---
Test instruction."#;

        let mut mgr = PluginManager::new();
        mgr.register(SkillPlugin::new("test-plugin").with_skill(skill_md));

        let registry = make_registry();
        let skill_reg = Arc::new(SkillRegistry::new());
        let ctx = PluginContext::new().with_skill_registry(Arc::clone(&skill_reg));

        mgr.load_all(&registry, &ctx);

        // No tools registered
        assert!(registry.get("test-plugin").is_none());
        // Skill registered
        assert!(skill_reg.get("test-skill").is_some());
    }

    #[test]
    fn skill_plugin_with_skills_builder() {
        let skill1 = "---\nname: s1\ndescription: d1\nkind: instruction\n---\nContent 1";
        let skill2 = "---\nname: s2\ndescription: d2\nkind: instruction\n---\nContent 2";

        let plugin = SkillPlugin::new("multi").with_skills([skill1, skill2]);
        assert_eq!(plugin.skills().len(), 2);
    }

    #[test]
    fn plugin_names() {
        let mut mgr = PluginManager::new();
        mgr.register(AgenticSearchPlugin::new());
        mgr.register(AgenticParsePlugin::new());
        let names = mgr.plugin_names();
        assert!(names.contains(&"agentic-search"));
        assert!(names.contains(&"agentic-parse"));
    }
}
