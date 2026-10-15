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
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum SkillRegistrySnapshotError {
    #[error("projected skill name '{name}' conflicts with the compatibility registry")]
    NameConflict { name: String },
    #[error("projected skill '{name}' failed Session validation: {message}")]
    Validation { name: String, message: String },
}

/// Skill map locks are held only by registry methods, and those methods do
/// not panic before the guard drops. A poisoned lock means a previous holder
/// already panicked. Recovering the guard avoids a second panic on the exec
/// path, which would kill every colocated workload.
fn lock_read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

type DiscoveredSkillFiles = HashMap<String, (PathBuf, Arc<Skill>)>;

/// Skill registry for managing available skills
///
/// Provides skill registration, loading from directories, and lookup by name.
/// Optionally validates skills before registration.
pub struct SkillRegistry {
    skills: Arc<RwLock<HashMap<String, Arc<Skill>>>>,
    builtin_names: Arc<RwLock<HashSet<String>>>,
    validator: Arc<RwLock<Option<Arc<dyn SkillValidator>>>>,
    /// Directory each loaded skill came from, so a directory refresh can
    /// evict exactly that directory's skills and reload it.
    origins: Arc<RwLock<HashMap<String, PathBuf>>>,
    /// Directories this registry watches for changes, with the modification
    /// stamp last seen. A directory that did not exist at load time is still
    /// watched: `a3s code skills add` creates it later.
    watched: Arc<std::sync::Mutex<Vec<WatchedDir>>>,
    discovered_files: Arc<RwLock<DiscoveredSkillFiles>>,
    excluded_files: Arc<RwLock<HashSet<PathBuf>>>,
    /// Names left out of this run. The value is the fixed public sentence.
    /// A directory file and an admitted generation that differ are both unused.
    withheld: Arc<RwLock<HashMap<String, String>>>,
}

/// One watched skills directory and the stamp last observed for it.
#[derive(Clone)]
struct WatchedDir {
    path: PathBuf,
    stamp: Option<std::time::SystemTime>,
}

impl SkillRegistry {
    /// Create a new empty skill registry
    pub fn new() -> Self {
        Self {
            skills: Arc::new(RwLock::new(HashMap::new())),
            builtin_names: Arc::new(RwLock::new(HashSet::new())),
            validator: Arc::new(RwLock::new(None)),
            origins: Default::default(),
            watched: Default::default(),
            discovered_files: Default::default(),
            excluded_files: Default::default(),
            withheld: Default::default(),
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
        let skills = lock_read(&self.skills).clone();
        let builtin_names = lock_read(&self.builtin_names).clone();
        let origins = lock_read(&self.origins).clone();
        let watched = self
            .watched
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        Self {
            skills: Arc::new(RwLock::new(skills)),
            builtin_names: Arc::new(RwLock::new(builtin_names)),
            validator: Arc::new(RwLock::new(lock_read(&self.validator).clone())),
            origins: Arc::new(RwLock::new(origins)),
            watched: Arc::new(std::sync::Mutex::new(watched)),
            discovered_files: Arc::new(RwLock::new(lock_read(&self.discovered_files).clone())),
            excluded_files: Arc::new(RwLock::new(lock_read(&self.excluded_files).clone())),
            withheld: Arc::new(RwLock::new(lock_read(&self.withheld).clone())),
        }
    }

    /// Public sentence for a skill left out of a run.
    ///
    /// The discovered file and the admitted generation both stay unused.
    pub(crate) fn withhold_sentence(name: &str) -> String {
        format!(
            "skill '{name}' is withheld because the discovered file and the admitted generation differ"
        )
    }

    /// Freeze the compatibility registry and add one external generation.
    /// Existing names fail closed; no projected Skill may silently shadow a
    /// built-in, host, or session registration.
    pub(crate) fn snapshot_with_external_skills(
        &self,
        external: impl IntoIterator<Item = Arc<Skill>>,
    ) -> Result<Self, SkillRegistrySnapshotError> {
        let mut skills = lock_read(&self.skills).clone();
        let builtin_names = lock_read(&self.builtin_names).clone();
        let validator = lock_read(&self.validator).clone();
        for skill in external {
            let name = skill.name.clone();
            if skills.contains_key(&name) {
                return Err(SkillRegistrySnapshotError::NameConflict { name });
            }
            if let Some(validator) = &validator {
                validator.validate(&skill).map_err(|error| {
                    SkillRegistrySnapshotError::Validation {
                        name: name.clone(),
                        message: error.to_string(),
                    }
                })?;
            }
            skills.insert(name, skill);
        }
        Ok(Self {
            skills: Arc::new(RwLock::new(skills)),
            builtin_names: Arc::new(RwLock::new(builtin_names)),
            validator: Arc::new(RwLock::new(validator)),
            origins: Default::default(),
            watched: Default::default(),
            discovered_files: Default::default(),
            excluded_files: Default::default(),
            withheld: Default::default(),
        })
    }

    /// Drop a directory-discovered skill that still owns its name.
    ///
    /// A host-protected name stays, so a later projection still conflicts.
    /// A live registration that replaced the discovered value also stays:
    /// only the pointer still held by the discovery table is removed.
    pub(crate) fn take_discovered(&self, name: &str) -> Option<PathBuf> {
        if self.protects(name) {
            return None;
        }
        let mut discovered = lock_write(&self.discovered_files);
        let (path, discovered_skill) = discovered.get(name)?;
        let path = path.clone();
        let discovered_skill = Arc::clone(discovered_skill);
        let mut skills = lock_write(&self.skills);
        let still_discovered = skills
            .get(name)
            .is_some_and(|current| Arc::ptr_eq(current, &discovered_skill));
        if !still_discovered {
            return None;
        }
        skills.remove(name);
        discovered.remove(name);
        drop(skills);
        drop(discovered);
        lock_write(&self.origins).remove(name);
        Some(path)
    }

    /// Record why a name has no body in this run.
    pub(crate) fn remember_withheld(&self, name: &str, reason: String) {
        lock_write(&self.withheld).insert(name.to_string(), reason);
    }

    /// The fixed sentence when this run left the name unused.
    pub(crate) fn withheld_reason(&self, name: &str) -> Option<String> {
        lock_read(&self.withheld).get(name).cloned()
    }

    /// Exclude exact canonical files owned by an external capability projection.
    /// Existing host registrations and unrelated same-name files remain intact.
    pub(crate) fn exclude_discovered_files(&self, files: &[PathBuf]) {
        let mut excluded = lock_write(&self.excluded_files);
        excluded.extend(files.iter().cloned());
        let discovered = lock_read(&self.discovered_files);
        let mut skills = lock_write(&self.skills);
        for (name, (path, skill)) in discovered.iter() {
            if excluded.contains(path)
                && skills
                    .get(name)
                    .is_some_and(|current| Arc::ptr_eq(current, skill))
            {
                skills.remove(name);
            }
        }
    }

    /// Set the skill validator (safety gate)
    pub fn set_validator(&self, validator: Arc<dyn SkillValidator>) {
        *lock_write(&self.validator) = Some(validator);
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
        if let Some(ref validator) = *lock_read(&self.validator) {
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
        let mut skills = lock_write(&self.skills);
        lock_write(&self.builtin_names).remove(&skill.name);
        skills.insert(skill.name.clone(), skill);
    }

    /// Register a validated, lifecycle-owned skill and return the registration
    /// it shadowed.
    ///
    /// The lookup and replacement happen under one write lock so a live
    /// session can later restore the exact prior skill by pointer identity.
    /// Built-in skills remain protected from live replacement.
    pub(crate) fn register_with_shadow(
        &self,
        skill: Arc<Skill>,
    ) -> Result<(bool, Option<Arc<Skill>>), super::validator::SkillValidationError> {
        if let Some(ref validator) = *lock_read(&self.validator) {
            validator.validate(&skill)?;
        }

        let name = skill.name.clone();
        let mut skills = lock_write(&self.skills);
        let builtin_names = lock_read(&self.builtin_names);
        if builtin_names.contains(&name) {
            tracing::warn!(
                skill = %name,
                "Rejected live skill registration because a built-in owns the name"
            );
            return Ok((false, None));
        }

        Ok((true, skills.insert(name, skill)))
    }

    /// Restore a shadowed skill only while `expected` still owns the name.
    ///
    /// This compare-and-replace prevents one lifecycle source from deleting or
    /// overwriting a skill installed later by another source.
    pub(crate) fn restore_if_same(
        &self,
        name: &str,
        expected: &Arc<Skill>,
        replacement: Option<Arc<Skill>>,
    ) -> bool {
        let mut skills = lock_write(&self.skills);
        let Some(current) = skills.get(name) else {
            return false;
        };
        if !Arc::ptr_eq(current, expected) {
            return false;
        }

        match replacement {
            Some(skill) => {
                skills.insert(name.to_string(), skill);
            }
            None => {
                skills.remove(name);
            }
        }
        true
    }

    fn register_builtin(&self, skill: Arc<Skill>) {
        let name = skill.name.clone();
        lock_write(&self.skills).insert(name.clone(), skill);
        lock_write(&self.builtin_names).insert(name);
    }

    /// Register a host-owned skill and protect the name from later replacement.
    pub fn register_host(&self, skill: Arc<Skill>) {
        self.register_builtin(skill);
    }

    /// A host-owned name is not replaced by a projected Use skill.
    pub fn protects(&self, name: &str) -> bool {
        lock_read(&self.builtin_names).contains(name)
    }

    /// Get a skill by name
    pub fn get(&self, name: &str) -> Option<Arc<Skill>> {
        let skills = lock_read(&self.skills);
        skills.get(name).cloned()
    }

    /// List all registered skill names
    pub fn list(&self) -> Vec<String> {
        let skills = lock_read(&self.skills);
        let mut names = skills.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    /// Get all registered skills
    pub fn all(&self) -> Vec<Arc<Skill>> {
        let skills = lock_read(&self.skills);
        let mut values = skills.values().cloned().collect::<Vec<_>>();
        values.sort_by(|a, b| a.name.cmp(&b.name));
        values
    }

    /// Load skills from a directory
    ///
    /// Recursively scans the directory for skill files and attempts to parse them.
    ///
    /// Supported layouts:
    /// - `path/to/skill.md` at the scan root
    /// - `path/to/skill/SKILL.md` at any nested directory
    ///
    /// Other Markdown files under a skill package (`README.md`,
    /// `examples/*.md`, `references/*.md`) are not skill candidates.
    ///
    /// Candidate files are processed in deterministic sorted order. A symlink
    /// whose resolved path leaves the directory being scanned is not loaded.
    /// A direct child that is itself a directory symlink is a published skill
    /// package: that package is loaded, and a link inside it that leaves the
    /// package is not. A missing directory is logged at error and loads
    /// nothing. A candidate that fails to parse is logged at error with its
    /// path. Validation failures are logged as warnings.
    pub fn load_from_dir(&self, dir: impl AsRef<Path>) -> anyhow::Result<usize> {
        let dir = dir.as_ref();

        if !dir.exists() {
            tracing::error!(
                path = %dir.display(),
                "skill directory does not exist; no skills were loaded from it"
            );
            return Ok(0);
        }

        if !dir.is_dir() {
            anyhow::bail!("Path is not a directory: {}", dir.display());
        }

        self.note_watched(dir);
        let mut loaded = 0;
        for candidate in Self::collect_skill_candidates(dir)? {
            let canonical = std::fs::canonicalize(&candidate)?;
            if lock_read(&self.excluded_files).contains(&canonical) {
                continue;
            }
            match Skill::from_file(&candidate) {
                Ok(skill) => {
                    let name = skill.name.clone();
                    if skill.kind == crate::skills::SkillKind::Tool && skill.allowed_tools.is_none()
                    {
                        tracing::warn!(
                            skill = %name,
                            path = %candidate.display(),
                            "Tool skill omits allowed-tools; invocation denies tools until allowed-tools is declared"
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
                    lock_write(&self.origins).insert(name.clone(), dir.to_path_buf());
                    match self.register(Arc::clone(&skill)) {
                        Ok(()) => {
                            lock_write(&self.discovered_files).insert(name, (canonical, skill));
                            loaded += 1;
                        }
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
                    tracing::error!(
                        path = %candidate.display(),
                        error = %e,
                        "skill candidate was not loaded"
                    );
                }
            }
        }

        Ok(loaded)
    }

    /// Watch a skill directory for changes without loading it now. A
    /// directory that does not exist yet is still watched: an install creates
    /// it later and the next refresh picks the package up.
    pub fn watch_dir(&self, dir: impl AsRef<Path>) {
        self.note_watched(dir.as_ref());
    }

    fn note_watched(&self, dir: &Path) {
        let stamp = dir_stamp(dir);
        let mut watched = self
            .watched
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match watched.iter_mut().find(|entry| entry.path == dir) {
            Some(entry) => entry.stamp = stamp,
            None => watched.push(WatchedDir {
                path: dir.to_path_buf(),
                stamp,
            }),
        }
    }

    /// Re-scan watched directories whose modification stamp changed and drop
    /// skills that disappeared from them. Returns how many directories were
    /// reloaded. This is what makes the skills directory live for a running
    /// session: `a3s code skills add` (or any file change) shows up on the
    /// next context query without rebuilding the session.
    pub fn refresh_watched_dirs(&self) -> usize {
        let watched = self
            .watched
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let mut reloaded = 0;
        for WatchedDir {
            path: ref dir,
            stamp: last,
        } in watched
        {
            let stamp = dir_stamp(dir);
            if stamp == last {
                continue;
            }
            // Evict exactly this directory's skills, then reload it.
            {
                let mut origins = lock_write(&self.origins);
                let mut skills = lock_write(&self.skills);
                let stale: Vec<String> = origins
                    .iter()
                    .filter(|(_, origin)| *origin == dir)
                    .map(|(name, _)| name.clone())
                    .collect();
                for name in stale {
                    origins.remove(&name);
                    skills.remove(&name);
                }
            }
            if let Err(error) = self.load_from_dir(dir) {
                tracing::warn!(
                    dir = %dir.display(),
                    %error,
                    "skill directory refresh failed"
                );
            }
            reloaded += 1;
        }
        reloaded
    }

    fn collect_skill_candidates(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
        /// Bound nested skill trees; symlink cycles are stopped by `seen`.
        const MAX_DEPTH: usize = 64;

        fn stays_in_root(root: &Path, path: &Path) -> bool {
            let Ok(root) = std::fs::canonicalize(root) else {
                return false;
            };
            std::fs::canonicalize(path)
                .map(|canonical| canonical.starts_with(root))
                .unwrap_or(false)
        }

        /// A directory symlink that is a direct child of the scan root. Hosts
        /// publish a skill by linking `~/.agents/skills/<name>` at the package.
        /// The link target is that package's containment root.
        fn is_published_package(path: &Path) -> bool {
            std::fs::symlink_metadata(path)
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false)
                && path.is_dir()
        }

        fn visit(
            dir: &Path,
            root: &Path,
            out: &mut Vec<PathBuf>,
            seen: &mut HashSet<PathBuf>,
            depth: usize,
            admit_packages: bool,
        ) -> anyhow::Result<()> {
            if depth > MAX_DEPTH {
                tracing::debug!(
                    path = %dir.display(),
                    max_depth = MAX_DEPTH,
                    "Skipping skill directory past max walk depth"
                );
                return Ok(());
            }
            if !stays_in_root(root, dir) {
                return Ok(());
            }

            // Canonicalize so symlink cycles (a→b→a) share one seen key.
            let identity = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
            if !seen.insert(identity) {
                return Ok(());
            }

            let mut entries = std::fs::read_dir(dir)
                .with_context(|| format!("Failed to read directory: {}", dir.display()))?
                .collect::<Result<Vec<_>, std::io::Error>>()?;
            entries.sort_by_key(|entry| entry.path());

            for entry in entries {
                let path = entry.path();
                if admit_packages && depth == 0 && is_published_package(&path) {
                    if let Ok(package_root) = std::fs::canonicalize(&path) {
                        visit(&path, &package_root, out, seen, 0, false)?;
                    }
                    continue;
                }
                if !stays_in_root(root, &path) {
                    continue;
                }
                if path.is_dir() {
                    // Resolve before collecting so `parent/loop -> sibling` does
                    // not push a second path to the same SKILL.md.
                    let child_identity =
                        std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                    if !seen.insert(child_identity) {
                        continue;
                    }
                    let skill_md = path.join("SKILL.md");
                    if skill_md.is_file() && stays_in_root(root, &skill_md) {
                        out.push(skill_md);
                    }
                    // Directory already marked seen; walk children only.
                    visit_children(&path, root, out, seen, depth + 1)?;
                } else if path.extension().and_then(|s| s.to_str()) == Some("md") {
                    out.push(path);
                }
            }
            Ok(())
        }

        fn visit_children(
            dir: &Path,
            root: &Path,
            out: &mut Vec<PathBuf>,
            seen: &mut HashSet<PathBuf>,
            depth: usize,
        ) -> anyhow::Result<()> {
            if depth > MAX_DEPTH || !stays_in_root(root, dir) {
                return Ok(());
            }

            let mut entries = std::fs::read_dir(dir)
                .with_context(|| format!("Failed to read directory: {}", dir.display()))?
                .collect::<Result<Vec<_>, std::io::Error>>()?;
            entries.sort_by_key(|entry| entry.path());

            for entry in entries {
                let path = entry.path();
                if !stays_in_root(root, &path) {
                    continue;
                }
                if path.is_dir() {
                    let child_identity =
                        std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                    if !seen.insert(child_identity) {
                        continue;
                    }
                    let skill_md = path.join("SKILL.md");
                    if skill_md.is_file() && stays_in_root(root, &skill_md) {
                        out.push(skill_md);
                    }
                    visit_children(&path, root, out, seen, depth + 1)?;
                }
            }
            Ok(())
        }

        let root = std::fs::canonicalize(dir)
            .with_context(|| format!("Failed to resolve skill directory: {}", dir.display()))?;
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        visit(dir, &root, &mut out, &mut seen, 0, true)?;
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
        let mut skills = lock_write(&self.skills);
        skills.remove(name)
    }

    /// Clear all skills
    pub fn clear(&self) {
        let mut skills = lock_write(&self.skills);
        skills.clear();
    }

    /// Get the number of registered skills
    pub fn len(&self) -> usize {
        let skills = lock_read(&self.skills);
        skills.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get all skills of a specific kind
    pub fn by_kind(&self, kind: super::SkillKind) -> Vec<Arc<Skill>> {
        let skills = lock_read(&self.skills);
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
        let skills = lock_read(&self.skills);
        let builtin_names = lock_read(&self.builtin_names);
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
        let skills = lock_read(&self.skills);
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
        let skills = lock_read(&self.skills);
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
        // `disable-model-invocation` is the Claude Skills contract for
        // host/user-only skills: keep them registered, but hide them from
        // model search/catalog and from Skill-tool invocation.
        !skill.disable_model_invocation
            && (skill.kind == super::SkillKind::Instruction || skill.kind == super::SkillKind::Tool)
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
    /// Emits only the catalog header telling the model to use `search_skills`
    /// then `Skill`. Full skill bodies are never auto-injected into the main
    /// prompt; they load only when the model invokes `Skill`.
    ///
    /// `match_skills` remains a helper for hosts that want keyword injection,
    /// but the default agent loop does not call it.
    pub fn to_system_prompt(&self) -> String {
        let skills = lock_read(&self.skills);

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

/// Modification stamp for a watched directory: the directory mtime plus the
/// mtimes of its immediate children, so a package added or removed one level
/// down is detected even on filesystems whose directory mtime does not
/// change for nested writes.
fn dir_stamp(dir: &Path) -> Option<std::time::SystemTime> {
    let meta = std::fs::symlink_metadata(dir).ok()?;
    let mut stamp = meta.modified().ok()?;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            // A published package is a symlink. Its own mtime must count even
            // when the target directory is older than this scan root.
            let path = entry.path();
            for child in [
                std::fs::symlink_metadata(&path).ok(),
                std::fs::metadata(&path).ok(),
            ]
            .into_iter()
            .flatten()
            {
                if let Ok(modified) = child.modified() {
                    if modified > stamp {
                        stamp = modified;
                    }
                }
            }
        }
    }
    Some(stamp)
}
