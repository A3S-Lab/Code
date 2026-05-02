//! Plugin System for A3S Code
//!
//! Provides a unified interface for loading and unloading optional tool plugins.
//! All plugins implement the [`Plugin`] trait, which gives the system a single
//! consistent surface for lifecycle management.
//!
//! # Usage
//!
//! ```rust,ignore
//! use a3s_code_core::{SessionOptions, SkillPlugin};
//!
//! let opts = SessionOptions::new()
//!     .with_plugin(SkillPlugin::new("custom"));
//! ```

use crate::skills::Skill;
use crate::tools::{register_program, register_program_with_catalog, ToolRegistry};
use anyhow::{bail, Result};
use std::sync::Arc;

// ============================================================================
// Plugin context — passed to every plugin on load
// ============================================================================

/// Runtime context provided to plugins when they are loaded.
///
/// Gives plugins access to shared session dependencies such as the LLM client,
/// skill registry, and document parser registry without coupling the `Plugin`
/// trait to specific concrete types.
#[derive(Clone)]
pub struct PluginContext {
    /// LLM client — required by tools that do LLM inference.
    pub llm: Option<Arc<dyn crate::llm::LlmClient>>,
    /// Skill registry — plugins may register companion skills here.
    pub skill_registry: Option<Arc<crate::skills::SkillRegistry>>,
}

impl PluginContext {
    pub fn new() -> Self {
        Self {
            llm: None,
            skill_registry: None,
        }
    }

    pub fn with_llm(mut self, llm: Arc<dyn crate::llm::LlmClient>) -> Self {
        self.llm = Some(llm);
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

/// Plugin that extends or replaces the model-visible `program` tool catalog.
///
/// This is the lightweight asset path for PTC templates: callers can package
/// `ProgramTemplate` values with a plugin and mount them per session without
/// writing a new tool.
pub struct ProgramPlugin {
    plugin_name: String,
    plugin_version: String,
    templates: Vec<crate::program::ProgramTemplate>,
    include_builtin_programs: bool,
}

impl ProgramPlugin {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            plugin_name: name.into(),
            plugin_version: "1.0.0".into(),
            templates: Vec::new(),
            include_builtin_programs: true,
        }
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.plugin_version = version.into();
        self
    }

    pub fn with_template(mut self, template: crate::program::ProgramTemplate) -> Self {
        self.templates.push(template);
        self
    }

    pub fn with_templates(
        mut self,
        templates: impl IntoIterator<Item = crate::program::ProgramTemplate>,
    ) -> Self {
        self.templates.extend(templates);
        self
    }

    pub fn without_builtin_programs(mut self) -> Self {
        self.include_builtin_programs = false;
        self
    }

    pub fn from_json(name: impl Into<String>, content: &str) -> Result<Self> {
        let asset = serde_json::from_str::<ProgramTemplateAsset>(content)?;
        Ok(Self::new(name).with_templates(asset.into_templates()))
    }

    pub fn from_yaml(name: impl Into<String>, content: &str) -> Result<Self> {
        let asset = serde_yaml::from_str::<ProgramTemplateAsset>(content)?;
        Ok(Self::new(name).with_templates(asset.into_templates()))
    }
}

impl Plugin for ProgramPlugin {
    fn name(&self) -> &str {
        &self.plugin_name
    }

    fn version(&self) -> &str {
        &self.plugin_version
    }

    fn tool_names(&self) -> &[&str] {
        &["program"]
    }

    fn load(&self, registry: &Arc<ToolRegistry>, _ctx: &PluginContext) -> Result<()> {
        if self.templates.is_empty() {
            bail!(
                "ProgramPlugin '{}' has no program templates",
                self.plugin_name
            );
        }

        let mut catalog = if self.include_builtin_programs {
            crate::program::ProgramCatalog::with_builtin_programs()
        } else {
            crate::program::ProgramCatalog::new()
        };
        for template in &self.templates {
            catalog.try_register(template.clone())?;
        }
        register_program_with_catalog(registry, catalog);
        Ok(())
    }

    fn unload(&self, registry: &Arc<ToolRegistry>) {
        register_program(registry);
    }

    fn description(&self) -> &str {
        "Registers programmatic tool calling templates"
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum ProgramTemplateAsset {
    Template(crate::program::ProgramTemplate),
    Templates(Vec<crate::program::ProgramTemplate>),
    Catalog {
        programs: Vec<crate::program::ProgramTemplate>,
    },
}

impl ProgramTemplateAsset {
    fn into_templates(self) -> Vec<crate::program::ProgramTemplate> {
        match self {
            Self::Template(template) => vec![template],
            Self::Templates(templates) => templates,
            Self::Catalog { programs } => programs,
        }
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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{Tool, ToolContext, ToolOutput, ToolRegistry};
    use async_trait::async_trait;
    use std::path::PathBuf;

    fn make_registry() -> Arc<ToolRegistry> {
        Arc::new(ToolRegistry::new(PathBuf::from("/tmp")))
    }

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn description(&self) -> &str {
            "Echoes a message"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            })
        }

        async fn execute(
            &self,
            args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            Ok(ToolOutput::success(
                args["message"].as_str().unwrap_or_default(),
            ))
        }
    }

    #[test]
    fn plugin_manager_register_and_query() {
        let mut mgr = PluginManager::new();
        assert!(mgr.is_empty());
        mgr.register(SkillPlugin::new("example"));
        assert_eq!(mgr.len(), 1);
        assert!(mgr.is_loaded("example"));
    }

    #[test]
    fn plugin_manager_load_all() {
        let mut mgr = PluginManager::new();
        mgr.register(SkillPlugin::new("example"));
        let registry = make_registry();
        let ctx = PluginContext::new();
        mgr.load_all(&registry, &ctx);
        assert!(registry.get("example").is_none());
    }

    #[test]
    fn plugin_manager_unload() {
        let mut mgr = PluginManager::new();
        mgr.register(SkillPlugin::new("example"));
        let registry = make_registry();
        let ctx = PluginContext::new();
        mgr.load_all(&registry, &ctx);
        mgr.unload("example", &registry);
        assert!(!mgr.is_loaded("example"));
    }

    #[test]
    fn plugin_manager_unload_all() {
        let mut mgr = PluginManager::new();
        mgr.register(SkillPlugin::new("example"));
        let registry = make_registry();
        let ctx = PluginContext::new();
        mgr.load_all(&registry, &ctx);
        mgr.unload_all(&registry);
        assert!(mgr.is_empty());
    }

    #[test]
    fn plugin_skills_registered_on_load_all() {
        use crate::skills::SkillRegistry;

        let mut mgr = PluginManager::new();
        mgr.register(SkillPlugin::new("test-plugin").with_skill(
            r#"---
name: test-skill
description: Test skill
allowed-tools: "read(*)"
kind: instruction
---
Read carefully."#,
        ));

        let registry = make_registry();
        let skill_reg = Arc::new(SkillRegistry::new());
        let ctx = PluginContext::new().with_skill_registry(Arc::clone(&skill_reg));

        mgr.load_all(&registry, &ctx);
        assert!(skill_reg.get("test-skill").is_some());
    }

    #[test]
    fn plugin_skills_not_registered_when_no_skill_registry_in_ctx() {
        let mut mgr = PluginManager::new();
        mgr.register(SkillPlugin::new("test-plugin"));

        let registry = make_registry();
        // No skill_registry in ctx
        let ctx = PluginContext::new();
        mgr.load_all(&registry, &ctx);
        // No crash — skill registry absence is silently tolerated
    }

    #[tokio::test]
    async fn program_plugin_registers_template_catalog() {
        let registry = make_registry();
        registry.register(Arc::new(EchoTool));
        let plugin = ProgramPlugin::new("program-pack").with_template(
            crate::program::ProgramTemplate::new("custom_echo", "Run a custom echo")
                .with_parameter(crate::program::ProgramParameter::required(
                    "message",
                    "Message to echo",
                ))
                .with_step(
                    crate::program::ProgramStepTemplate::new(
                        "echo",
                        serde_json::json!({ "message": "{{message}}" }),
                    )
                    .with_label("echo_message"),
                ),
        );

        plugin.load(&registry, &PluginContext::new()).unwrap();

        let result = registry
            .execute_with_context(
                "program",
                &serde_json::json!({
                    "name": "custom_echo",
                    "inputs": { "message": "hello" }
                }),
                &ToolContext::new(PathBuf::from("/tmp")),
            )
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.output.contains("hello"));
        assert_eq!(
            result.metadata.as_ref().unwrap()["trace"]["program_name"],
            "custom_echo"
        );
    }

    #[tokio::test]
    async fn program_plugin_can_load_templates_from_yaml_asset() {
        let registry = make_registry();
        registry.register(Arc::new(EchoTool));
        let plugin = ProgramPlugin::from_yaml(
            "program-pack",
            r#"
programs:
  - name: asset_echo
    description: Echo from a YAML asset
    parameters:
      - name: message
        description: Message to echo
        required: true
    steps:
      - tool_name: echo
        label: echo_message
        args:
          message: "{{message}}"
"#,
        )
        .unwrap()
        .without_builtin_programs();

        plugin.load(&registry, &PluginContext::new()).unwrap();

        let result = registry
            .execute_with_context(
                "program",
                &serde_json::json!({
                    "name": "asset_echo",
                    "inputs": { "message": "from asset" }
                }),
                &ToolContext::new(PathBuf::from("/tmp")),
            )
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.output.contains("from asset"));
        assert_eq!(
            result.metadata.as_ref().unwrap()["trace"]["program_name"],
            "asset_echo"
        );
    }

    #[test]
    fn program_plugin_rejects_empty_catalog() {
        let registry = make_registry();
        let plugin = ProgramPlugin::new("empty-program-pack");

        let err = plugin.load(&registry, &PluginContext::new()).unwrap_err();

        assert!(err.to_string().contains("has no program templates"));
    }

    #[test]
    fn program_plugin_rejects_invalid_template_assets() {
        let registry = make_registry();
        let plugin =
            ProgramPlugin::new("bad-program-pack").with_template(crate::program::ProgramTemplate {
                name: "bad-template".to_string(),
                description: "Bad template".to_string(),
                parameters: vec![],
                steps: vec![crate::program::ProgramStepTemplate {
                    tool_name: "grep".to_string(),
                    args: serde_json::json!({ "pattern": "{{missing}}" }),
                    label: None,
                }],
            });

        let err = plugin.load(&registry, &PluginContext::new()).unwrap_err();

        assert!(err.to_string().contains("unknown program parameter"));
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
        mgr.register(SkillPlugin::new("a"));
        mgr.register(SkillPlugin::new("b"));
        let names = mgr.plugin_names();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }
}
