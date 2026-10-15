//! One A3S Use generation, projected into the Meta Harness tool surface.
//!
//! The model sees built-in tools plus the skills, MCP servers, and Runtime
//! Tasks published by the leased snapshot. Installing a missing package is a
//! host decision. The current generation does not change until the host
//! applies the next snapshot.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::skills::{Skill, SkillRegistry};

/// Surfaces the model may ask the host to install. Interface, flow, and
/// knowledge stay on the host and are not tools.
const REQUESTABLE_SURFACES: &[&str] = &["skill", "mcp", "tool"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UseHarnessGeneration {
    builtins: BTreeSet<String>,
    skills: BTreeSet<String>,
    mcp_servers: BTreeSet<String>,
    runtime_tasks: BTreeSet<String>,
}

impl UseHarnessGeneration {
    pub fn skills(&self) -> &BTreeSet<String> {
        &self.skills
    }

    pub fn runtime_tasks(&self) -> &BTreeSet<String> {
        &self.runtime_tasks
    }

    pub fn allows_catalog_tool(&self, name: &str) -> bool {
        if name == "a3s-use" || name.is_empty() {
            return false;
        }
        if self.builtins.contains(name) || self.runtime_tasks.contains(name) {
            return true;
        }
        self.mcp_servers.iter().any(|server| {
            let prefix = format!("mcp__{server}__");
            name.starts_with(&prefix) && name.len() > prefix.len()
        })
    }

    /// Names that stay stable for every turn of this generation.
    pub fn frozen_names(&self) -> BTreeSet<String> {
        let mut names = self.builtins.clone();
        names.extend(self.skills.iter().cloned());
        names.extend(self.runtime_tasks.iter().cloned());
        for server in &self.mcp_servers {
            names.insert(format!("mcp__{server}"));
        }
        names
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageInstallRequest {
    pub package_id: String,
    pub surfaces: BTreeSet<String>,
}

impl PackageInstallRequest {
    pub fn parse(package_id: &str, surfaces: &[&str]) -> Option<Self> {
        let package_id = package_id.trim();
        if package_id.is_empty()
            || package_id.contains('\0')
            || !package_id.contains('/')
            || surfaces.is_empty()
        {
            return None;
        }
        let mut parsed = BTreeSet::new();
        for surface in surfaces {
            if !REQUESTABLE_SURFACES.contains(surface) {
                return None;
            }
            parsed.insert((*surface).to_owned());
        }
        Some(Self {
            package_id: package_id.to_owned(),
            surfaces: parsed,
        })
    }
}

/// Project published skills into `registry` and return the generation.
///
/// A protected host skill keeps its name. Any other same-named registration
/// is replaced in memory by the snapshot file. This function does not write
/// the skill directory.
pub fn project_generation(
    registry: &SkillRegistry,
    snapshot: &str,
    builtins: impl IntoIterator<Item = impl AsRef<str>>,
) -> UseHarnessGeneration {
    let published = published_skills(snapshot);
    let mut skills = BTreeSet::new();
    for skill in published {
        if registry.protects(&skill.name) {
            continue;
        }
        let name = skill.name.clone();
        registry.register_unchecked(skill);
        skills.insert(name);
    }
    UseHarnessGeneration {
        builtins: builtins
            .into_iter()
            .map(|name| name.as_ref().to_owned())
            .filter(|name| !name.is_empty() && name != "a3s-use")
            .collect(),
        skills,
        mcp_servers: published_mcp_servers(snapshot),
        runtime_tasks: published_runtime_tasks(snapshot),
    }
}

/// Host answer to a missing package. Denial returns the same generation.
/// Acceptance returns the generation projected from `next_snapshot`.
pub fn cutover(
    current: &UseHarnessGeneration,
    request: &PackageInstallRequest,
    granted: bool,
    registry: &SkillRegistry,
    next_snapshot: &str,
) -> UseHarnessGeneration {
    if !granted || request.surfaces.is_empty() {
        return current.clone();
    }
    project_generation(
        registry,
        next_snapshot,
        current.builtins.iter().map(String::as_str),
    )
}

fn published_skills(snapshot: &str) -> Vec<std::sync::Arc<Skill>> {
    let mut loaded = Vec::new();
    for (path, expected) in skill_entries(snapshot) {
        let Some(skill) = load_verified_skill(&path, &expected) else {
            continue;
        };
        loaded.push(skill);
    }
    loaded
}

fn skill_entries(snapshot: &str) -> Vec<(PathBuf, String)> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(snapshot) else {
        return Vec::new();
    };
    let Some(capabilities) = capabilities_of(&value) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for capability in capabilities {
        if !capability_is_published(capability) {
            continue;
        }
        let Some(skills) = capability.get("skills").and_then(|item| item.as_array()) else {
            continue;
        };
        for skill in skills {
            let Some(path) = skill.get("path").and_then(|item| item.as_str()) else {
                continue;
            };
            let Some(sha) = skill.get("sha256").and_then(|item| item.as_str()) else {
                continue;
            };
            let path = PathBuf::from(path);
            if path.file_name().and_then(|name| name.to_str()) != Some("SKILL.md") {
                continue;
            }
            entries.push((path, sha.to_ascii_lowercase()));
        }
    }
    entries
}

fn published_mcp_servers(snapshot: &str) -> BTreeSet<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(snapshot) else {
        return BTreeSet::new();
    };
    let Some(capabilities) = capabilities_of(&value) else {
        return BTreeSet::new();
    };
    let mut servers = BTreeSet::new();
    for capability in capabilities {
        if !capability_is_published(capability) {
            continue;
        }
        let Some(listed) = capability
            .get("mcpServers")
            .or_else(|| capability.get("mcp_servers"))
            .and_then(|item| item.as_array())
        else {
            continue;
        };
        for server in listed {
            if let Some(name) = server
                .get("serverName")
                .or_else(|| server.get("server_name"))
                .and_then(|item| item.as_str())
            {
                if !name.is_empty() {
                    servers.insert(name.to_owned());
                }
            }
        }
    }
    servers
}

fn published_runtime_tasks(snapshot: &str) -> BTreeSet<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(snapshot) else {
        return BTreeSet::new();
    };
    let Some(capabilities) = capabilities_of(&value) else {
        return BTreeSet::new();
    };
    let mut tasks = BTreeSet::new();
    for capability in capabilities {
        if !capability_is_published(capability) {
            continue;
        }
        let Some(listed) = capability
            .get("toolTasks")
            .or_else(|| capability.get("tool_tasks"))
            .and_then(|item| item.as_array())
        else {
            continue;
        };
        for task in listed {
            if let Some(name) = task
                .get("toolName")
                .or_else(|| task.get("tool_name"))
                .and_then(|item| item.as_str())
            {
                if !name.is_empty() && name != "a3s-use" {
                    tasks.insert(name.to_owned());
                }
            }
        }
    }
    tasks
}

fn capabilities_of(value: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    value
        .pointer("/data/registry/capabilities")
        .or_else(|| value.pointer("/registry/capabilities"))
        .or_else(|| value.get("capabilities"))
        .and_then(|item| item.as_array())
}

fn capability_is_published(capability: &serde_json::Value) -> bool {
    if capability.get("enabled") != Some(&serde_json::Value::Bool(true)) {
        return false;
    }
    match capability.get("readiness").and_then(|item| item.as_str()) {
        Some("ready") | None => true,
        Some(_) => false,
    }
}

fn load_verified_skill(path: &Path, expected_sha: &str) -> Option<std::sync::Arc<Skill>> {
    let meta = fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != expected_sha {
        return None;
    }
    let skill = Skill::from_file(path).ok()?;
    if skill.name.is_empty() {
        return None;
    }
    Some(std::sync::Arc::new(skill))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn sha(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn skill_file(dir: &Path, body: &str) -> (PathBuf, String) {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join("SKILL.md");
        fs::write(&path, body).unwrap();
        let digest = sha(body.as_bytes());
        (path, digest)
    }

    fn snapshot(skill: Option<(&Path, &str)>, extra: serde_json::Value) -> String {
        let mut capability = serde_json::json!({
            "id": "use/a3s/review",
            "enabled": true,
            "readiness": "ready",
            "activityBar": [{ "id": "picker" }],
            "flows": [{ "id": "review-flow" }],
            "knowledge": [{ "id": "review-notes" }]
        });
        if let Some((path, digest)) = skill {
            capability["skills"] = serde_json::json!([{
                "id": "review",
                "path": path.display().to_string(),
                "sha256": digest
            }]);
        }
        if let Some(object) = extra.as_object() {
            for (key, value) in object {
                capability[key] = value.clone();
            }
        }
        serde_json::json!({
            "data": { "registry": { "capabilities": [capability] } }
        })
        .to_string()
    }

    const REVIEW: &str =
        "---\nname: review\ndescription: Review one document\n---\nQuote the document.\n";

    #[test]
    fn empty_snapshot_publishes_no_skill() {
        let registry = SkillRegistry::new();
        let generation = project_generation(&registry, "{}", ["Skill", "read"]);
        assert!(generation.skills().is_empty());
        assert!(registry.get("review").is_none());
    }

    #[test]
    fn published_skill_loads_and_a_digest_mismatch_does_not() {
        let root = std::env::temp_dir().join(format!("a3s-harness-skill-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (path, digest) = skill_file(&root.join("ok"), REVIEW);
        let registry = SkillRegistry::new();
        let generation = project_generation(
            &registry,
            &snapshot(Some((&path, &digest)), serde_json::json!({})),
            ["Skill", "read"],
        );
        assert!(generation.skills().contains("review"));
        assert_eq!(
            registry.get("review").unwrap().content,
            "Quote the document."
        );

        let broken = snapshot(Some((&path, &"ab".repeat(32))), serde_json::json!({}));
        let empty = SkillRegistry::new();
        let hidden = project_generation(&empty, &broken, ["Skill"]);
        assert!(hidden.skills().is_empty());
        assert!(empty.get("review").is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn snapshot_replaces_a_user_registration_and_leaves_the_file() {
        let root = std::env::temp_dir().join(format!("a3s-harness-user-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let user = root.join("user/review/SKILL.md");
        fs::create_dir_all(user.parent().unwrap()).unwrap();
        let user_body = "---\nname: review\ndescription: mine\n---\nMine.\n";
        fs::write(&user, user_body).unwrap();
        let registry = SkillRegistry::new();
        registry
            .register(Arc::new(Skill::from_file(&user).unwrap()))
            .unwrap();

        let (path, digest) = skill_file(&root.join("pkg"), REVIEW);
        project_generation(
            &registry,
            &snapshot(Some((&path, &digest)), serde_json::json!({})),
            ["Skill"],
        );
        assert_eq!(
            registry.get("review").unwrap().content,
            "Quote the document."
        );
        assert_eq!(fs::read_to_string(&user).unwrap(), user_body);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_protected_host_skill_is_not_replaced() {
        let root = std::env::temp_dir().join(format!("a3s-harness-host-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (path, digest) = skill_file(&root, REVIEW);
        let registry = SkillRegistry::new();
        registry.register_host(Arc::new(Skill {
            name: "review".to_owned(),
            description: "host".to_owned(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: crate::skills::SkillKind::Instruction,
            content: "Host copy.".to_owned(),
            tags: Vec::new(),
            version: None,
        }));
        let generation = project_generation(
            &registry,
            &snapshot(Some((&path, &digest)), serde_json::json!({})),
            ["Skill"],
        );
        assert!(generation.skills().is_empty());
        assert_eq!(registry.get("review").unwrap().content, "Host copy.");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn one_generation_freezes_tools_and_omits_host_surfaces() {
        let root = std::env::temp_dir().join(format!("a3s-harness-freeze-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (path, digest) = skill_file(&root, REVIEW);
        let raw = snapshot(
            Some((&path, &digest)),
            serde_json::json!({
                "mcpServers": [{ "serverName": "docs" }],
                "toolTasks": [{ "toolName": "use_review_task" }]
            }),
        );
        let registry = SkillRegistry::new();
        let generation = project_generation(&registry, &raw, ["Skill", "read", "bash"]);
        let first = generation.frozen_names();
        let second = generation.frozen_names();
        assert_eq!(first, second);
        assert!(first.contains("Skill"));
        assert!(first.contains("review"));
        assert!(first.contains("use_review_task"));
        assert!(first.contains("mcp__docs"));
        assert!(!first.iter().any(|name| {
            name.contains("picker") || name.contains("flow") || name.contains("knowledge")
        }));
        assert!(generation.allows_catalog_tool("read"));
        assert!(generation.allows_catalog_tool("mcp__docs__lookup"));
        assert!(generation.allows_catalog_tool("use_review_task"));
        assert!(!generation.allows_catalog_tool("a3s-use"));
        assert!(!generation.allows_catalog_tool("review"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn denial_keeps_this_generation_and_acceptance_waits_for_the_next() {
        let current = project_generation(&SkillRegistry::new(), "{}", ["Skill", "read"]);
        let request = PackageInstallRequest::parse("a3s/review", &["skill", "ui"]);
        assert!(request.is_none());
        let request = PackageInstallRequest::parse("a3s/review", &["skill"]).unwrap();
        let denied = cutover(
            &current,
            &request,
            false,
            &SkillRegistry::new(),
            r#"{"capabilities":[]}"#,
        );
        assert_eq!(denied, current);
        assert!(!denied.skills().contains("review"));

        let root = std::env::temp_dir().join(format!("a3s-harness-next-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (path, digest) = skill_file(&root, REVIEW);
        let next_registry = SkillRegistry::new();
        let accepted = cutover(
            &current,
            &request,
            true,
            &next_registry,
            &snapshot(Some((&path, &digest)), serde_json::json!({})),
        );
        assert!(accepted.skills().contains("review"));
        assert!(!current.skills().contains("review"));
        assert!(next_registry.get("review").is_some());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_generation_cannot_clear_the_kernel_policy() {
        let policy = crate::meta_harness::KernelPolicy {
            permission_overlay: false,
            completion_gate: false,
        }
        .admit();
        assert!(policy.permission_overlay);
        assert!(policy.completion_gate);
    }
}
