//! Skill Registry
//!
//! Manages skill registration, loading, and lookup.
//! Integrates with `SkillValidator` as a safety gate for externally loaded skills.

use super::validator::SkillValidator;
use super::{Skill, SkillKind};
use anyhow::Context;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Skill registry for managing available skills
///
/// Provides skill registration, loading from directories, and lookup by name.
/// Optionally validates skills before registration.
pub struct SkillRegistry {
    skills: Arc<RwLock<HashMap<String, Arc<Skill>>>>,
    builtin_names: Arc<RwLock<HashSet<String>>>,
    validator: Arc<RwLock<Option<Arc<dyn SkillValidator>>>>,
}

impl SkillRegistry {
    /// Create a new empty skill registry
    pub fn new() -> Self {
        Self {
            skills: Arc::new(RwLock::new(HashMap::new())),
            builtin_names: Arc::new(RwLock::new(HashSet::new())),
            validator: Arc::new(RwLock::new(None)),
        }
    }

    /// Create a registry with built-in skills.
    ///
    /// Built-in skills have been removed, so this is a compatibility alias for
    /// [`Self::new`]. Load reusable skills through skill directories, inline
    /// skills, or explicit registration.
    pub fn with_builtins() -> Self {
        let registry = Self::new();
        for skill in super::builtin::builtin_skills() {
            registry.register_builtin(skill);
        }
        registry
    }

    /// Fork this registry into an independent copy.
    ///
    /// The fork shares no state with the original — skills added to the fork
    /// do not affect the source registry. The validator is preserved so
    /// that session and delegated-agent registries keep the same safety policy.
    pub fn fork(&self) -> Self {
        let skills = self.skills.read().unwrap().clone();
        let builtin_names = self.builtin_names.read().unwrap().clone();
        Self {
            skills: Arc::new(RwLock::new(skills)),
            builtin_names: Arc::new(RwLock::new(builtin_names)),
            validator: Arc::new(RwLock::new(self.validator.read().unwrap().clone())),
        }
    }

    /// Set the skill validator (safety gate)
    pub fn set_validator(&self, validator: Arc<dyn SkillValidator>) {
        *self.validator.write().unwrap() = Some(validator);
    }

    /// Register a skill with validation
    ///
    /// If a validator is set, the skill must pass validation before registration.
    /// Returns an error if validation fails.
    pub fn register(
        &self,
        skill: Arc<Skill>,
    ) -> Result<(), super::validator::SkillValidationError> {
        // Run validator if set
        if let Some(ref validator) = *self.validator.read().unwrap() {
            validator.validate(&skill)?;
        }
        self.register_unchecked(skill);
        Ok(())
    }

    /// Register a skill without validation.
    ///
    /// If a future embedded skill set contains the same name, this replacement
    /// is treated as external for global tool-restriction purposes.
    pub fn register_unchecked(&self, skill: Arc<Skill>) {
        self.builtin_names.write().unwrap().remove(&skill.name);
        let mut skills = self.skills.write().unwrap();
        skills.insert(skill.name.clone(), skill);
    }

    fn register_builtin(&self, skill: Arc<Skill>) {
        let name = skill.name.clone();
        self.skills.write().unwrap().insert(name.clone(), skill);
        self.builtin_names.write().unwrap().insert(name);
    }

    /// Get a skill by name
    pub fn get(&self, name: &str) -> Option<Arc<Skill>> {
        let skills = self.skills.read().unwrap();
        skills.get(name).cloned()
    }

    /// List all registered skill names
    pub fn list(&self) -> Vec<String> {
        let skills = self.skills.read().unwrap();
        let mut names = skills.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    /// Get all registered skills
    pub fn all(&self) -> Vec<Arc<Skill>> {
        let skills = self.skills.read().unwrap();
        let mut values = skills.values().cloned().collect::<Vec<_>>();
        values.sort_by(|a, b| a.name.cmp(&b.name));
        values
    }

    /// Load skills from a directory
    ///
    /// Recursively scans the directory for skill files and attempts to parse them.
    ///
    /// Supported layouts:
    /// - `path/to/skill.md`
    /// - `path/to/skill/SKILL.md`
    ///
    /// Candidate files are processed in deterministic sorted order. Files that
    /// fail to parse are skipped with debug logging; validation failures are
    /// logged as warnings.
    pub fn load_from_dir(&self, dir: impl AsRef<Path>) -> anyhow::Result<usize> {
        let dir = dir.as_ref();

        if !dir.exists() {
            return Ok(0);
        }

        if !dir.is_dir() {
            anyhow::bail!("Path is not a directory: {}", dir.display());
        }

        let mut loaded = 0;
        for candidate in Self::collect_skill_candidates(dir)? {
            match Skill::from_file(&candidate) {
                Ok(skill) => {
                    let name = skill.name.clone();
                    if skill.allowed_tools.is_none() {
                        tracing::warn!(
                            skill = %name,
                            path = %candidate.display(),
                            "Skill omits allowed-tools; Skill invocation is fail-secure and will deny tool use until allowed-tools is declared"
                        );
                    } else if skill.uses_legacy_allowed_tools_syntax() {
                        tracing::warn!(
                            skill = %name,
                            path = %candidate.display(),
                            "Skill uses legacy whitespace-separated allowed-tools; use comma-separated permissions such as Read(*), Write(*), Bash(*) or a YAML list"
                        );
                    }
                    let skill = Arc::new(skill);
                    if self.get(&name).is_some() {
                        tracing::warn!(
                            skill = %name,
                            path = %candidate.display(),
                            "Duplicate skill name encountered during directory load — overriding previous definition"
                        );
                    }
                    match self.register(skill) {
                        Ok(()) => loaded += 1,
                        Err(e) => {
                            tracing::warn!(
                                "Skill validation failed for {}: {}",
                                candidate.display(),
                                e
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!("Skipped {}: {}", candidate.display(), e);
                }
            }
        }

        Ok(loaded)
    }

    fn collect_skill_candidates(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
        fn visit(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
            let mut entries = std::fs::read_dir(dir)
                .with_context(|| format!("Failed to read directory: {}", dir.display()))?
                .collect::<Result<Vec<_>, std::io::Error>>()?;
            entries.sort_by_key(|entry| entry.path());

            for entry in entries {
                let path = entry.path();
                if path.is_dir() {
                    let skill_md = path.join("SKILL.md");
                    if skill_md.is_file() {
                        out.push(skill_md);
                    }
                    visit(&path, out)?;
                } else if path.extension().and_then(|s| s.to_str()) == Some("md") {
                    out.push(path);
                }
            }
            Ok(())
        }

        let mut out = Vec::new();
        visit(dir, &mut out)?;
        out.sort();
        out.dedup();
        Ok(out)
    }

    /// Load a single skill from a file
    pub fn load_from_file(&self, path: impl AsRef<Path>) -> anyhow::Result<Arc<Skill>> {
        let skill = Skill::from_file(path)?;
        let skill = Arc::new(skill);
        self.register(skill.clone())
            .map_err(|e| anyhow::anyhow!("Skill validation failed: {}", e))?;
        Ok(skill)
    }

    /// Remove a skill by name
    pub fn remove(&self, name: &str) -> Option<Arc<Skill>> {
        let mut skills = self.skills.write().unwrap();
        skills.remove(name)
    }

    /// Clear all skills
    pub fn clear(&self) {
        let mut skills = self.skills.write().unwrap();
        skills.clear();
    }

    /// Get the number of registered skills
    pub fn len(&self) -> usize {
        let skills = self.skills.read().unwrap();
        skills.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get all skills of a specific kind
    pub fn by_kind(&self, kind: super::SkillKind) -> Vec<Arc<Skill>> {
        let skills = self.skills.read().unwrap();
        let mut values = skills
            .values()
            .filter(|s| s.kind == kind)
            .cloned()
            .collect::<Vec<_>>();
        values.sort_by(|a, b| a.name.cmp(&b.name));
        values
    }

    /// Instruction skills that actively constrain normal session tool use.
    ///
    /// Embedded skills, when present, can have local allowlists for explicit
    /// `Skill` invocation, but those allowlists must not make the default
    /// registry globally read-only. User-registered skills remain external.
    pub fn global_tool_restricting_skills(&self) -> Vec<Arc<Skill>> {
        let skills = self.skills.read().unwrap();
        let builtin_names = self.builtin_names.read().unwrap();
        let mut values = skills
            .values()
            .filter(|skill| {
                skill.kind == SkillKind::Instruction
                    && skill.allowed_tools.is_some()
                    && !builtin_names.contains(&skill.name)
            })
            .cloned()
            .collect::<Vec<_>>();
        values.sort_by(|a, b| a.name.cmp(&b.name));
        values
    }

    /// Get all skills with a specific tag
    pub fn by_tag(&self, tag: &str) -> Vec<Arc<Skill>> {
        let skills = self.skills.read().unwrap();
        let mut values = skills
            .values()
            .filter(|s| s.tags.iter().any(|t| t == tag))
            .cloned()
            .collect::<Vec<_>>();
        values.sort_by(|a, b| a.name.cmp(&b.name));
        values
    }

    /// Get all persona-kind skills
    ///
    /// Personas are session-level system prompts bound at session creation.
    /// They are NOT injected into the global system prompt via `to_system_prompt()`.
    pub fn personas(&self) -> Vec<Arc<Skill>> {
        self.by_kind(super::SkillKind::Persona)
    }

    /// Search discoverable instruction/tool skills by name, tag, description, or content.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Arc<Skill>> {
        let skills = self.skills.read().unwrap();
        let query_lower = query.to_lowercase();
        let query_tokens: Vec<&str> = query_lower
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
            .filter(|w| w.len() >= 2)
            .collect();

        let mut scored: Vec<(u32, String, Arc<Skill>)> = skills
            .values()
            .filter(|s| Self::is_discoverable_skill(s))
            .filter_map(|skill| {
                let score = Self::skill_search_score(skill, &query_lower, &query_tokens);
                if score == 0 {
                    None
                } else {
                    Some((score, skill.name.clone(), Arc::clone(skill)))
                }
            })
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        scored
            .into_iter()
            .take(limit.max(1))
            .map(|(_, _, skill)| skill)
            .collect()
    }

    fn is_discoverable_skill(skill: &Skill) -> bool {
        skill.kind == super::SkillKind::Instruction || skill.kind == super::SkillKind::Tool
    }

    fn skill_search_score(skill: &Skill, query_lower: &str, query_tokens: &[&str]) -> u32 {
        if query_lower.trim().is_empty() {
            return 1;
        }

        let name = skill.name.to_lowercase();
        let description = skill.description.to_lowercase();
        let tags: Vec<String> = skill.tags.iter().map(|t| t.to_lowercase()).collect();
        let content = skill.content.to_lowercase();
        let mut score = 0;

        if query_lower.contains(&name) {
            score += 100;
        }
        if tags.iter().any(|tag| query_lower.contains(tag)) {
            score += 80;
        }

        for token in query_tokens {
            if name.contains(token) {
                score += 20;
            }
            if tags.iter().any(|tag| tag.contains(token)) {
                score += 15;
            }
            if description.contains(token) {
                score += 8;
            }
            if content.contains(token) {
                score += 2;
            }
        }

        score
    }

    /// Generate system prompt content from all instruction skills
    ///
    /// Concatenates the content of all instruction-type skills for injection
    /// into the system prompt.
    /// Persona-kind skills are excluded — they are bound per-session, not globally.
    /// Generate the system prompt fragment for this registry.
    ///
    /// Only emits a skill directory (name + description) — NOT the full skill content.
    /// Full content is injected on-demand via `match_skills` when a user request matches.
    pub fn to_system_prompt(&self) -> String {
        let skills = self.skills.read().unwrap();

        let has_discoverable_skill = skills.values().any(|s| Self::is_discoverable_skill(s));

        if !has_discoverable_skill {
            return String::new();
        }

        String::from(crate::prompts::SKILLS_CATALOG_HEADER)
    }

    /// Return the full content of skills relevant to the given user input.
    ///
    /// Matches by checking if any skill name or tag appears in the input (case-insensitive).
    /// Returns an empty string if no skills match — caller should not inject anything.
    pub fn match_skills(&self, user_input: &str) -> String {
        let matched = self.search(user_input, 3);

        if matched.is_empty() {
            return String::new();
        }

        let mut out = String::from("# Skill Instructions\n\n");
        for skill in matched {
            out.push_str(&skill.to_system_prompt());
            out.push_str("\n\n---\n\n");
        }
        out
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::SkillKind;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_new_registry() {
        let registry = SkillRegistry::new();
        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());
    }

    #[test]
    fn test_with_builtins_is_empty_compatibility_registry() {
        let registry = SkillRegistry::with_builtins();
        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());
    }

    #[test]
    fn test_register_and_get() {
        let registry = SkillRegistry::new();

        let skill = Arc::new(Skill {
            name: "test-skill".to_string(),
            description: "A test skill".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "Test content".to_string(),
            tags: vec![],
            version: None,
        });

        registry.register(skill.clone()).unwrap();

        assert_eq!(registry.len(), 1);
        let retrieved = registry.get("test-skill").unwrap();
        assert_eq!(retrieved.name, "test-skill");
    }

    #[test]
    fn test_list() {
        let registry = SkillRegistry::with_builtins();
        let names = registry.list();

        assert!(names.is_empty());
    }

    #[test]
    fn test_remove() {
        let registry = SkillRegistry::with_builtins();
        registry.register_unchecked(Arc::new(Skill {
            name: "code-search".to_string(),
            description: "External skill".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: String::new(),
            tags: Vec::new(),
            version: None,
        }));
        assert_eq!(registry.len(), 1);

        let removed = registry.remove("code-search");
        assert!(removed.is_some());
        assert_eq!(registry.len(), 0);
        assert!(registry.get("code-search").is_none());
    }

    #[test]
    fn test_clear() {
        let registry = SkillRegistry::with_builtins();
        registry.register_unchecked(Arc::new(Skill {
            name: "temporary-skill".to_string(),
            description: "Temporary".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: String::new(),
            tags: Vec::new(),
            version: None,
        }));
        assert_eq!(registry.len(), 1);

        registry.clear();
        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());
    }

    #[test]
    fn test_by_kind() {
        let registry = SkillRegistry::with_builtins();
        let instruction_skills = registry.by_kind(SkillKind::Instruction);

        assert!(instruction_skills.is_empty());

        let persona_skills = registry.by_kind(SkillKind::Persona);
        assert_eq!(persona_skills.len(), 0);
    }

    #[test]
    fn test_empty_compatibility_builtins_do_not_restrict_global_tools() {
        let registry = SkillRegistry::with_builtins();
        assert!(registry.global_tool_restricting_skills().is_empty());
    }

    #[test]
    fn test_old_builtin_names_are_normal_external_skills() {
        let registry = SkillRegistry::with_builtins();
        registry.register_unchecked(Arc::new(Skill {
            name: "code-review".to_string(),
            description: "External skill".to_string(),
            allowed_tools: Some("read(*)".to_string()),
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: String::new(),
            tags: Vec::new(),
            version: None,
        }));

        let restricting = registry.global_tool_restricting_skills();
        assert_eq!(restricting.len(), 1);
        assert_eq!(restricting[0].name, "code-review");
    }

    #[test]
    fn test_global_tool_restricting_skills_are_sorted() {
        let registry = SkillRegistry::new();
        registry.register_unchecked(Arc::new(Skill {
            name: "zeta".to_string(),
            description: "Zeta".to_string(),
            allowed_tools: Some("read(*)".to_string()),
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: String::new(),
            tags: Vec::new(),
            version: None,
        }));
        registry.register_unchecked(Arc::new(Skill {
            name: "alpha".to_string(),
            description: "Alpha".to_string(),
            allowed_tools: Some("grep(*)".to_string()),
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: String::new(),
            tags: Vec::new(),
            version: None,
        }));

        let names: Vec<String> = registry
            .global_tool_restricting_skills()
            .into_iter()
            .map(|skill| skill.name.clone())
            .collect();
        assert_eq!(names, vec!["alpha".to_string(), "zeta".to_string()]);
    }

    #[test]
    fn test_by_tag() {
        let registry = SkillRegistry::new();
        registry.register_unchecked(Arc::new(Skill {
            name: "code-search".to_string(),
            description: "External search skill".to_string(),
            allowed_tools: Some("read(*), grep(*)".to_string()),
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: String::new(),
            tags: vec!["search".to_string()],
            version: None,
        }));
        registry.register_unchecked(Arc::new(Skill {
            name: "find-bugs".to_string(),
            description: "External bug skill".to_string(),
            allowed_tools: Some("read(*), grep(*)".to_string()),
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: String::new(),
            tags: vec!["bugs".to_string(), "security".to_string()],
            version: None,
        }));
        let search_skills = registry.by_tag("search");

        assert_eq!(search_skills.len(), 1);
        let names: Vec<&str> = search_skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"code-search"));

        let security_skills = registry.by_tag("security");
        assert_eq!(security_skills.len(), 1);
        assert_eq!(security_skills[0].name, "find-bugs");
    }

    #[test]
    fn test_load_from_dir() -> anyhow::Result<()> {
        let temp_dir = TempDir::new()?;

        // Create a valid skill file
        let skill_path = temp_dir.path().join("test-skill.md");
        let mut file = std::fs::File::create(&skill_path)?;
        writeln!(file, "---")?;
        writeln!(file, "name: test-skill")?;
        writeln!(file, "description: A test skill")?;
        writeln!(file, "kind: instruction")?;
        writeln!(file, "---")?;
        writeln!(file, "# Test Skill")?;
        writeln!(file, "This is a test skill.")?;
        drop(file);

        // Create a non-skill .md file (should be skipped)
        let readme_path = temp_dir.path().join("README.md");
        std::fs::write(&readme_path, "# README\nNot a skill")?;

        // Create a non-.md file (should be skipped)
        let txt_path = temp_dir.path().join("notes.txt");
        std::fs::write(&txt_path, "Some notes")?;

        let registry = SkillRegistry::new();
        let loaded = registry.load_from_dir(temp_dir.path())?;

        assert_eq!(loaded, 1);
        assert_eq!(registry.len(), 1);
        assert!(registry.get("test-skill").is_some());

        Ok(())
    }

    #[test]
    fn test_load_from_dir_recurses_into_nested_skill_dirs() -> anyhow::Result<()> {
        let temp_dir = TempDir::new()?;
        let nested = temp_dir.path().join("nested").join("code-review-helper");
        std::fs::create_dir_all(&nested)?;

        let skill_path = nested.join("SKILL.md");
        let mut file = std::fs::File::create(&skill_path)?;
        writeln!(file, "---")?;
        writeln!(file, "name: nested-skill")?;
        writeln!(file, "description: A nested skill")?;
        writeln!(file, "kind: instruction")?;
        writeln!(file, "---")?;
        writeln!(file, "# Nested Skill")?;
        writeln!(file, "This skill lives in a nested SKILL.md.")?;
        drop(file);

        let registry = SkillRegistry::new();
        let loaded = registry.load_from_dir(temp_dir.path())?;

        assert_eq!(loaded, 1);
        assert!(registry.get("nested-skill").is_some());
        Ok(())
    }

    #[test]
    fn test_load_from_dir_accepts_yaml_list_allowed_tools() -> anyhow::Result<()> {
        let temp_dir = TempDir::new()?;
        let skill_path = temp_dir.path().join("ci-review.md");
        std::fs::write(
            &skill_path,
            r#"---
name: ci-review
description: Review CI failures
allowed-tools:
  - Read
  - Grep
  - Bash(cargo test -p a3s-code-core:*)
kind: instruction
---
# CI Review
"#,
        )?;

        let registry = SkillRegistry::new();
        let loaded = registry.load_from_dir(temp_dir.path())?;

        assert_eq!(loaded, 1);
        let skill = registry.get("ci-review").unwrap();
        assert_eq!(
            skill.allowed_tools.as_deref(),
            Some("Read, Grep, Bash(cargo test -p a3s-code-core:*)")
        );
        let permissions = skill.parse_allowed_tools();
        assert!(permissions
            .iter()
            .any(|perm| perm.tool == "Read" && perm.pattern == "*"));
        assert!(permissions.iter().any(|perm| {
            perm.tool == "Bash" && perm.pattern == "cargo test -p a3s-code-core:*"
        }));
        Ok(())
    }

    #[test]
    fn test_load_from_file() -> anyhow::Result<()> {
        let temp_dir = TempDir::new()?;
        let skill_path = temp_dir.path().join("my-skill.md");

        let mut file = std::fs::File::create(&skill_path)?;
        writeln!(file, "---")?;
        writeln!(file, "name: my-skill")?;
        writeln!(file, "description: My custom skill")?;
        writeln!(file, "---")?;
        writeln!(file, "# My Skill")?;
        drop(file);

        let registry = SkillRegistry::new();
        let skill = registry.load_from_file(&skill_path)?;

        assert_eq!(skill.name, "my-skill");
        assert_eq!(registry.len(), 1);

        Ok(())
    }

    #[test]
    fn test_to_system_prompt() {
        let registry = SkillRegistry::with_builtins();
        let prompt = registry.to_system_prompt();

        assert!(prompt.is_empty());
    }

    #[test]
    fn test_load_from_nonexistent_dir() {
        let registry = SkillRegistry::new();
        let result = registry.load_from_dir("/nonexistent/path");

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn test_load_from_dir_rejects_file_path() -> anyhow::Result<()> {
        let temp_dir = TempDir::new()?;
        let path = temp_dir.path().join("not-a-directory.md");
        std::fs::write(&path, "# not a directory")?;

        let registry = SkillRegistry::new();
        let err = registry.load_from_dir(&path).unwrap_err();
        assert!(err.to_string().contains("Path is not a directory"));
        Ok(())
    }

    #[test]
    fn test_load_from_dir_duplicate_name_overrides_previous_definition() -> anyhow::Result<()> {
        let temp_dir = TempDir::new()?;

        let first = temp_dir.path().join("first.md");
        std::fs::write(
            &first,
            "---\nname: duplicate-skill\ndescription: First copy\n---\n# First\nalpha\n",
        )?;

        let nested = temp_dir.path().join("nested");
        std::fs::create_dir_all(&nested)?;
        let second = nested.join("SKILL.md");
        std::fs::write(
            &second,
            "---\nname: duplicate-skill\ndescription: Second copy\n---\n# Second\nbeta\n",
        )?;

        let registry = SkillRegistry::new();
        let loaded = registry.load_from_dir(temp_dir.path())?;

        assert_eq!(loaded, 2);
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry.get("duplicate-skill").unwrap().description,
            "Second copy"
        );
        Ok(())
    }

    // --- Validator integration ---

    #[test]
    fn test_register_with_validator_accepts_old_builtin_name() {
        use crate::skills::validator::DefaultSkillValidator;

        let registry = SkillRegistry::new();
        registry.set_validator(Arc::new(DefaultSkillValidator::default()));

        let skill = Arc::new(Skill {
            name: "code-search".to_string(),
            description: "External skill using a formerly built-in name".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "Search code carefully.".to_string(),
            tags: vec![],
            version: None,
        });

        let result = registry.register(skill);
        assert!(result.is_ok());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_register_with_validator_accepts_valid() {
        use crate::skills::validator::DefaultSkillValidator;

        let registry = SkillRegistry::new();
        registry.set_validator(Arc::new(DefaultSkillValidator::default()));

        let skill = Arc::new(Skill {
            name: "my-custom-skill".to_string(),
            description: "A valid skill".to_string(),
            allowed_tools: Some("read(*), grep(*)".to_string()),
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "Help with code review.".to_string(),
            tags: vec![],
            version: None,
        });

        assert!(registry.register(skill).is_ok());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_register_without_validator_accepts_anything() {
        let registry = SkillRegistry::new();
        // No validator set

        let skill = Arc::new(Skill {
            name: "code-search".to_string(),
            description: "test".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "test".to_string(),
            tags: vec![],
            version: None,
        });

        assert!(registry.register(skill).is_ok());
    }

    #[test]
    fn test_all_and_personas() {
        let registry = SkillRegistry::new();

        registry.register_unchecked(Arc::new(Skill {
            name: "persona-skill".to_string(),
            description: "Persona".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Persona,
            content: "Persona content".to_string(),
            tags: vec!["voice".to_string()],
            version: None,
        }));
        registry.register_unchecked(Arc::new(Skill {
            name: "instruction-skill".to_string(),
            description: "Instruction".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "Instruction content".to_string(),
            tags: vec!["workflow".to_string()],
            version: None,
        }));

        let all_names: Vec<String> = registry
            .all()
            .into_iter()
            .map(|skill| skill.name.clone())
            .collect();
        assert_eq!(
            all_names,
            vec!["instruction-skill".to_string(), "persona-skill".to_string()]
        );
        assert_eq!(registry.personas().len(), 1);
        assert_eq!(registry.personas()[0].name, "persona-skill");
    }

    #[test]
    fn test_load_from_file_with_validator_accepts_old_builtin_name() {
        use crate::skills::validator::DefaultSkillValidator;

        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("code-search.md");

        let mut file = std::fs::File::create(&skill_path).unwrap();
        writeln!(file, "---").unwrap();
        writeln!(file, "name: code-search").unwrap();
        writeln!(file, "description: External search skill").unwrap();
        writeln!(file, "---").unwrap();
        writeln!(file, "# Search").unwrap();
        drop(file);

        let registry = SkillRegistry::new();
        registry.set_validator(Arc::new(DefaultSkillValidator::default()));

        let result = registry.load_from_file(&skill_path);
        assert!(result.is_ok());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_fork_is_independent() {
        let original = SkillRegistry::with_builtins();
        let fork = original.fork();

        // Fork has same skills as original
        assert_eq!(fork.len(), original.len());

        // Adding to fork does not affect original
        fork.register_unchecked(Arc::new(Skill {
            name: "session-only".to_string(),
            description: "Only in fork".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "content".to_string(),
            tags: vec![],
            version: None,
        }));

        assert_eq!(fork.len(), original.len() + 1);
        assert!(fork.get("session-only").is_some());
        assert!(original.get("session-only").is_none());
    }

    #[test]
    fn test_fork_inherits_empty_compatibility_builtins() {
        let fork = SkillRegistry::with_builtins().fork();
        assert!(fork.is_empty());
    }

    #[test]
    fn test_fork_preserves_validator() {
        use crate::skills::validator::DefaultSkillValidator;

        let original = SkillRegistry::new();
        original.set_validator(Arc::new(DefaultSkillValidator::default()));

        let fork = original.fork();
        let invalid = Arc::new(Skill {
            name: "BadName".to_string(),
            description: "invalid".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "content".to_string(),
            tags: vec![],
            version: None,
        });

        assert!(fork.register(invalid).is_err());
    }

    #[test]
    fn test_search_skills_ranks_matches() {
        let registry = SkillRegistry::new();

        registry.register_unchecked(Arc::new(Skill {
            name: "build-planner".to_string(),
            description: "Plan complex builds".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "Planner instructions".to_string(),
            tags: vec!["architecture".to_string()],
            version: None,
        }));
        let matches = registry.search("architecture plan", 5);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "build-planner");
    }

    #[test]
    fn test_match_skills_matches_name_tag_and_description() {
        let registry = SkillRegistry::new();

        registry.register_unchecked(Arc::new(Skill {
            name: "build-planner".to_string(),
            description: "Plan complex builds".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "Planner instructions".to_string(),
            tags: vec!["architecture".to_string()],
            version: None,
        }));
        let by_name = registry.match_skills("please use build-planner for this task");
        assert!(by_name.contains("Planner instructions"));

        let by_tag = registry.match_skills("need architecture guidance");
        assert!(by_tag.contains("Planner instructions"));

        let by_description = registry.match_skills("help me plan the release");
        assert!(by_description.contains("Planner instructions"));

        assert!(registry
            .match_skills("totally unrelated request")
            .is_empty());
    }
}
