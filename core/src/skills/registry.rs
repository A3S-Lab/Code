//! Skill Registry
//!
//! Manages skill registration, loading, and lookup.
//! Integrates with `SkillValidator` (safety gate) and `SkillScorer` (feedback loop).

use super::feedback::SkillScorer;
use super::validator::SkillValidator;
use super::Skill;
use anyhow::Context;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

/// Skill registry for managing available skills
///
/// Provides skill registration, loading from directories, and lookup by name.
/// Optionally validates skills before registration and filters disabled skills
/// from the system prompt based on feedback scores.
pub struct SkillRegistry {
    skills: Arc<RwLock<HashMap<String, Arc<Skill>>>>,
    validator: Arc<RwLock<Option<Arc<dyn SkillValidator>>>>,
    scorer: Arc<RwLock<Option<Arc<dyn SkillScorer>>>>,
}

impl SkillRegistry {
    /// Create a new empty skill registry
    pub fn new() -> Self {
        Self {
            skills: Arc::new(RwLock::new(HashMap::new())),
            validator: Arc::new(RwLock::new(None)),
            scorer: Arc::new(RwLock::new(None)),
        }
    }

    /// Create a registry with built-in skills
    pub fn with_builtins() -> Self {
        let registry = Self::new();
        for skill in super::builtin::builtin_skills() {
            // Built-in skills bypass validation
            registry.register_unchecked(skill);
        }
        registry
    }

    /// Set the skill validator (safety gate)
    pub fn set_validator(&self, validator: Arc<dyn SkillValidator>) {
        *self.validator.write().unwrap() = Some(validator);
    }

    /// Set the skill scorer (feedback loop)
    pub fn set_scorer(&self, scorer: Arc<dyn SkillScorer>) {
        *self.scorer.write().unwrap() = Some(scorer);
    }

    /// Get the scorer (for external use, e.g., ManageSkillTool)
    pub fn scorer(&self) -> Option<Arc<dyn SkillScorer>> {
        self.scorer.read().unwrap().clone()
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

    /// Register a skill without validation (for built-in skills)
    pub fn register_unchecked(&self, skill: Arc<Skill>) {
        let mut skills = self.skills.write().unwrap();
        skills.insert(skill.name.clone(), skill);
    }

    /// Get a skill by name
    pub fn get(&self, name: &str) -> Option<Arc<Skill>> {
        let skills = self.skills.read().unwrap();
        skills.get(name).cloned()
    }

    /// List all registered skill names
    pub fn list(&self) -> Vec<String> {
        let skills = self.skills.read().unwrap();
        skills.keys().cloned().collect()
    }

    /// Get all registered skills
    pub fn all(&self) -> Vec<Arc<Skill>> {
        let skills = self.skills.read().unwrap();
        skills.values().cloned().collect()
    }

    /// Load skills from a directory
    ///
    /// Scans the directory for `.md` files and attempts to parse them as skills.
    /// Silently skips files that fail to parse.
    pub fn load_from_dir(&self, dir: impl AsRef<Path>) -> anyhow::Result<usize> {
        let dir = dir.as_ref();

        if !dir.exists() {
            return Ok(0);
        }

        if !dir.is_dir() {
            anyhow::bail!("Path is not a directory: {}", dir.display());
        }

        let mut loaded = 0;

        for entry in std::fs::read_dir(dir)
            .with_context(|| format!("Failed to read directory: {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();

            // Only process .md files
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }

            // Try to load the skill
            match Skill::from_file(&path) {
                Ok(skill) => {
                    let skill = Arc::new(skill);
                    match self.register(skill) {
                        Ok(()) => loaded += 1,
                        Err(e) => {
                            tracing::warn!("Skill validation failed for {}: {}", path.display(), e);
                        }
                    }
                }
                Err(e) => {
                    // Log but don't fail - some .md files might not be skills
                    tracing::debug!("Skipped {}: {}", path.display(), e);
                }
            }
        }

        Ok(loaded)
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
        skills
            .values()
            .filter(|s| s.kind == kind)
            .cloned()
            .collect()
    }

    /// Get all skills with a specific tag
    pub fn by_tag(&self, tag: &str) -> Vec<Arc<Skill>> {
        let skills = self.skills.read().unwrap();
        skills
            .values()
            .filter(|s| s.tags.iter().any(|t| t == tag))
            .cloned()
            .collect()
    }

    /// Get all persona-kind skills
    ///
    /// Personas are session-level system prompts bound at session creation.
    /// They are NOT injected into the global system prompt via `to_system_prompt()`.
    pub fn personas(&self) -> Vec<Arc<Skill>> {
        self.by_kind(super::SkillKind::Persona)
    }

    /// Generate system prompt content from all instruction skills
    ///
    /// Concatenates the content of all instruction-type skills for injection
    /// into the system prompt. Skills disabled by the scorer are excluded.
    /// Persona-kind skills are excluded — they are bound per-session, not globally.
    pub fn to_system_prompt(&self) -> String {
        let skills = self.skills.read().unwrap();
        let scorer = self.scorer.read().unwrap();

        let instruction_skills: Vec<_> = skills
            .values()
            .filter(|s| s.kind == super::SkillKind::Instruction)
            .filter(|s| {
                // Skip skills disabled by scorer
                match scorer.as_ref() {
                    Some(sc) => !sc.should_disable(&s.name),
                    None => true,
                }
            })
            .collect();

        if instruction_skills.is_empty() {
            return String::new();
        }

        let mut prompt = String::from("# Available Skills\n\n");

        for skill in instruction_skills {
            prompt.push_str(&skill.to_system_prompt());
            prompt.push_str("\n\n---\n\n");
        }

        prompt
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
    fn test_with_builtins() {
        let registry = SkillRegistry::with_builtins();
        assert_eq!(registry.len(), 7, "Expected 7 built-in skills");
        assert!(!registry.is_empty());

        // Code assistance skills
        assert!(registry.get("code-search").is_some());
        assert!(registry.get("code-review").is_some());
        assert!(registry.get("explain-code").is_some());
        assert!(registry.get("find-bugs").is_some());

        // Tool documentation skills
        assert!(registry.get("builtin-tools").is_some());
        assert!(registry.get("delegate-task").is_some());
        assert!(registry.get("find-skills").is_some());
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

        assert_eq!(names.len(), 7, "Expected 7 built-in skills");
        assert!(names.contains(&"code-search".to_string()));
        assert!(names.contains(&"code-review".to_string()));
        assert!(names.contains(&"builtin-tools".to_string()));
        assert!(names.contains(&"delegate-task".to_string()));
        assert!(names.contains(&"find-skills".to_string()));
    }

    #[test]
    fn test_remove() {
        let registry = SkillRegistry::with_builtins();
        assert_eq!(registry.len(), 7);

        let removed = registry.remove("code-search");
        assert!(removed.is_some());
        assert_eq!(registry.len(), 6);
        assert!(registry.get("code-search").is_none());
    }

    #[test]
    fn test_clear() {
        let registry = SkillRegistry::with_builtins();
        assert_eq!(registry.len(), 7);

        registry.clear();
        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());
    }

    #[test]
    fn test_by_kind() {
        let registry = SkillRegistry::with_builtins();
        let instruction_skills = registry.by_kind(SkillKind::Instruction);

        assert_eq!(
            instruction_skills.len(),
            7,
            "Expected 7 instruction skills (4 code assistance + 3 tool documentation)"
        );

        let tool_skills = registry.by_kind(SkillKind::Tool);
        assert_eq!(tool_skills.len(), 0);
    }

    #[test]
    fn test_by_tag() {
        let registry = SkillRegistry::with_builtins();
        let search_skills = registry.by_tag("search");

        assert_eq!(search_skills.len(), 1);
        assert_eq!(search_skills[0].name, "code-search");

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

        assert!(prompt.contains("# Available Skills"));
        assert!(prompt.contains("code-search"));
        assert!(prompt.contains("code-review"));
        assert!(prompt.contains("explain-code"));
        assert!(prompt.contains("find-bugs"));
    }

    #[test]
    fn test_load_from_nonexistent_dir() {
        let registry = SkillRegistry::new();
        let result = registry.load_from_dir("/nonexistent/path");

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    // --- Validator integration ---

    #[test]
    fn test_register_with_validator_rejects_reserved() {
        use crate::skills::validator::DefaultSkillValidator;

        let registry = SkillRegistry::new();
        registry.set_validator(Arc::new(DefaultSkillValidator::default()));

        let skill = Arc::new(Skill {
            name: "code-search".to_string(), // reserved
            description: "Override builtin".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "Malicious override".to_string(),
            tags: vec![],
            version: None,
        });

        let result = registry.register(skill);
        assert!(result.is_err());
        assert_eq!(registry.len(), 0);
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
            name: "code-search".to_string(), // reserved name, but no validator
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
    fn test_load_from_file_with_validator_rejects() {
        use crate::skills::validator::DefaultSkillValidator;

        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("code-search.md");

        let mut file = std::fs::File::create(&skill_path).unwrap();
        writeln!(file, "---").unwrap();
        writeln!(file, "name: code-search").unwrap(); // reserved
        writeln!(file, "description: Override").unwrap();
        writeln!(file, "---").unwrap();
        writeln!(file, "# Override").unwrap();
        drop(file);

        let registry = SkillRegistry::new();
        registry.set_validator(Arc::new(DefaultSkillValidator::default()));

        let result = registry.load_from_file(&skill_path);
        assert!(result.is_err());
        assert_eq!(registry.len(), 0);
    }

    // --- Scorer integration ---

    #[test]
    fn test_to_system_prompt_skips_disabled_skills() {
        use crate::skills::feedback::{DefaultSkillScorer, SkillFeedback, SkillOutcome};

        let registry = SkillRegistry::new();
        let scorer = Arc::new(DefaultSkillScorer::default());
        registry.set_scorer(scorer.clone());

        // Register two skills (unchecked to bypass validator)
        registry.register_unchecked(Arc::new(Skill {
            name: "good-skill".to_string(),
            description: "Good".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "Good instructions".to_string(),
            tags: vec![],
            version: None,
        }));
        registry.register_unchecked(Arc::new(Skill {
            name: "bad-skill".to_string(),
            description: "Bad".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "Bad instructions".to_string(),
            tags: vec![],
            version: None,
        }));

        // Give bad-skill enough negative feedback to disable it
        for _ in 0..5 {
            scorer.record(SkillFeedback {
                skill_name: "bad-skill".to_string(),
                outcome: SkillOutcome::Failure,
                score_delta: -1.0,
                reason: "Did not help".to_string(),
                timestamp: 0,
            });
        }

        let prompt = registry.to_system_prompt();
        assert!(prompt.contains("good-skill"));
        assert!(!prompt.contains("bad-skill"));
    }

    #[test]
    fn test_to_system_prompt_without_scorer_includes_all() {
        let registry = SkillRegistry::new();
        // No scorer set

        registry.register_unchecked(Arc::new(Skill {
            name: "skill-a".to_string(),
            description: "A".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "Content A".to_string(),
            tags: vec![],
            version: None,
        }));

        let prompt = registry.to_system_prompt();
        assert!(prompt.contains("skill-a"));
    }
}
