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
#[path = "registry/tests.rs"]
mod tests;
