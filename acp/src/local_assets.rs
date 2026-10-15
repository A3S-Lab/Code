//! Short catalog of a Studio tenant's installed connectors, installed skills,
//! and saved wiki pages.
//!
//! The standing prompt receives names and descriptions. [`open_local_asset`]
//! reads one selected item after the caller denies are applied. A deny of
//! `open_local_asset`, of `Skill`, or of `mcp__<name>` skips the read.
//! Files are the source of truth. The `catalog` array on `_meta` is ignored.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::tools::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
#[path = "connector_notes.rs"]
mod connector_notes;

#[allow(unused_imports)] // re-export consumed by the session tests
pub use self::connector_notes::sync_connector_notes;

pub const DESCRIPTION_LIMIT: usize = 160;
const BODY_LIMIT: usize = 32 * 1024;
const FRONTMATTER_LIMIT: usize = 4 * 1024;
const MAX_PER_KIND: usize = 32;
const TOOL_NAME: &str = "open_local_asset";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssetRoots {
    pub connectors: Option<PathBuf>,
    pub skills: Option<PathBuf>,
    pub wiki: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogItem {
    pub kind: &'static str,
    pub name: String,
    pub description: String,
    path: PathBuf,
    /// Returned when a connector has no usage note. Empty for skills and wiki pages.
    fallback: String,
}

pub struct OpenLocalAssetTool {
    roots: AssetRoots,
    denies: Arc<Mutex<Vec<String>>>,
}

impl OpenLocalAssetTool {
    pub fn new(roots: AssetRoots, denies: Arc<Mutex<Vec<String>>>) -> Self {
        Self { roots, denies }
    }
}

#[async_trait]
impl Tool for OpenLocalAssetTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn description(&self) -> &str {
        "Read one installed local connector, skill, or wiki page by kind and name. Returns that item only."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "enum": ["connector", "skill", "wiki"] },
                "name": { "type": "string" }
            },
            "required": ["kind", "name"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: &Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let kind = args.get("kind").and_then(Value::as_str).unwrap_or("");
        let name = args.get("name").and_then(Value::as_str).unwrap_or("");
        let denies = self
            .denies
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poison| poison.into_inner().clone());
        match open_local_asset(&self.roots, kind, name, "default", &denies) {
            Ok(body) => Ok(ToolOutput::success(body)),
            Err(reason) => Ok(ToolOutput::error(reason)),
        }
    }
}

pub fn parse_roots(value: Option<&Value>) -> Option<AssetRoots> {
    let obj = value?.as_object()?;
    let path = |key: &str| {
        let text = obj.get(key)?.as_str()?;
        if text.is_empty() || text.contains('\0') {
            return None;
        }
        let path = PathBuf::from(text);
        path.is_absolute().then_some(path)
    };
    let roots = AssetRoots {
        connectors: path("connectors"),
        skills: path("skills"),
        wiki: path("wiki"),
    };
    (roots.connectors.is_some() || roots.skills.is_some() || roots.wiki.is_some()).then_some(roots)
}

pub fn load_catalog(roots: &AssetRoots) -> Vec<CatalogItem> {
    let mut items = Vec::new();
    if let Some(root) = &roots.connectors {
        items.extend(connectors(root));
    }
    if let Some(root) = &roots.skills {
        items.extend(skills(root));
    }
    if let Some(root) = &roots.wiki {
        items.extend(wiki_pages(root));
    }
    items.sort_by(|left, right| {
        left.kind
            .cmp(right.kind)
            .then_with(|| left.name.cmp(&right.name))
    });
    items
}

/// Names and descriptions of connectors and skills. Wiki pages stay out of the
/// prompt so retrieval goes through the published index.
///
/// `wiki_root` is set when a wiki directory was admitted. The sentence tells
/// the model it may search; it does not require a call.
pub fn catalog_prompt(items: &[CatalogItem], wiki_root: bool) -> String {
    let listed: Vec<&CatalogItem> = items.iter().filter(|item| item.kind != "wiki").collect();
    if listed.is_empty() && !wiki_root {
        return String::new();
    }
    let mut lines = vec!["Local assets:".to_string()];
    if wiki_root {
        lines.push(
            "Choose the wiki sources this question needs. search_wiki scope=agent agent=<id> reads that agent's encyclopedia. scope=local reads personal notes. scope=team reads the organization encyclopedia. web_search reads the public web. Skip a source that the question does not need. A hit names the page and includes a short passage."
                .to_string(),
        );
    }
    if !listed.is_empty() {
        lines.push(
            "Call open_local_asset with kind and name to read one matching connector or skill. Leave every other item unread."
                .to_string(),
        );
        for item in listed {
            if item.description.is_empty() {
                lines.push(format!("- {} {}", item.kind, item.name));
            } else {
                lines.push(format!(
                    "- {} {}: {}",
                    item.kind, item.name, item.description
                ));
            }
        }
    }
    lines.join("\n")
}

/// Read one catalog item. Caller denies are applied before the narrow allow.
/// Deny and ask do not read the file.
pub fn open_local_asset(
    roots: &AssetRoots,
    kind: &str,
    name: &str,
    posture: &str,
    denies: &[String],
) -> Result<String, &'static str> {
    let args = json!({ "kind": kind, "name": name });
    let mut policy = a3s_code_core::permissions::policy_for_posture(posture);
    for rule in denies {
        policy = policy.deny(rule);
    }
    if existing_deny_blocks(&policy, kind, name) {
        policy = policy.deny(TOOL_NAME);
    }
    policy = policy.allow(TOOL_NAME);
    let found =
        match a3s_code_core::fact_control::admit_tool_call(&policy, TOOL_NAME, &args, || {
            load_catalog(roots)
                .into_iter()
                .find(|item| item.kind == kind && item.name == name)
        }) {
            Ok(item) => item,
            Err(PermissionDecision::Deny) => return Err("denied"),
            Err(_) => return Err("denied"),
        };
    let Some(item) = found else {
        return Err("missing");
    };
    read_body(&item).map_err(|_| "unreadable")
}

fn existing_deny_blocks(policy: &PermissionPolicy, kind: &str, name: &str) -> bool {
    match kind {
        "skill" => {
            policy.check("Skill", &json!({ "skill_name": name })) == PermissionDecision::Deny
        }
        "connector" => {
            let root = format!("mcp__{name}");
            let nested = format!("mcp__{name}__call");
            policy.check(&root, &json!({})) == PermissionDecision::Deny
                || policy.check(&nested, &json!({})) == PermissionDecision::Deny
        }
        _ => false,
    }
}

fn connectors(root: &Path) -> Vec<CatalogItem> {
    let Ok(bytes) = std::fs::read(root.join("mcp.json")) else {
        return Vec::new();
    };
    let Ok(doc) = serde_json::from_slice::<Value>(&bytes) else {
        return Vec::new();
    };
    let Some(servers) = doc.get("mcpServers").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut names: Vec<&str> = servers
        .keys()
        .map(String::as_str)
        .filter(|name| slug_ok(name))
        .collect();
    names.sort_unstable();
    names.truncate(MAX_PER_KIND);
    names
        .into_iter()
        .map(|name| {
            let kind = servers
                .get(name)
                .and_then(Value::as_object)
                .and_then(|server| server.get("type"))
                .and_then(Value::as_str);
            let fallback = connector_notes::transport_label(kind);
            let path = root.join(format!("{name}.md"));
            let description = read_prefix(&path, FRONTMATTER_LIMIT)
                .and_then(|text| connector_notes::note_summary(&text))
                .unwrap_or_else(|| fallback.clone());
            CatalogItem {
                kind: "connector",
                name: name.to_string(),
                description,
                path,
                fallback,
            }
        })
        .collect()
}

fn skills(root: &Path) -> Vec<CatalogItem> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return false;
            };
            if name.starts_with('.') || name.contains('/') || name.contains('\\') {
                return false;
            }
            std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir())
        })
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    let mut items = Vec::new();
    for dir in dirs {
        if items.len() >= MAX_PER_KIND {
            break;
        }
        let path = dir.join("SKILL.md");
        if !is_real_file(&path) {
            continue;
        }
        let Some(text) = read_prefix(&path, FRONTMATTER_LIMIT) else {
            continue;
        };
        let (name, description) = frontmatter(&text);
        let Some(name) = name.filter(|name| slug_ok(name)) else {
            continue;
        };
        items.push(CatalogItem {
            kind: "skill",
            name,
            description: description.map(|text| one_line(&text)).unwrap_or_default(),
            path,
            fallback: String::new(),
        });
    }
    items
}

fn wiki_pages(root: &Path) -> Vec<CatalogItem> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().and_then(|ext| ext.to_str()) == Some("md") && is_real_file(path)
        })
        .collect();
    files.sort();
    let mut items = Vec::new();
    for path in files {
        if items.len() >= MAX_PER_KIND {
            break;
        }
        let Some(text) = read_prefix(&path, FRONTMATTER_LIMIT) else {
            continue;
        };
        let (title, description) = wiki_frontmatter(&text);
        let Some(title) = title.filter(|title| title_ok(title)) else {
            continue;
        };
        if items.iter().any(|item: &CatalogItem| item.name == title) {
            continue;
        }
        items.push(CatalogItem {
            kind: "wiki",
            name: title,
            description: description.map(|text| one_line(&text)).unwrap_or_default(),
            path,
            fallback: String::new(),
        });
    }
    items
}

fn read_body(item: &CatalogItem) -> std::io::Result<String> {
    let file = match File::open(&item.path) {
        Ok(file) => file,
        Err(err) => {
            if item.kind == "connector" && !item.fallback.is_empty() {
                return Ok(item.fallback.clone());
            }
            return Err(err);
        }
    };
    let mut bytes = Vec::new();
    file.take(BODY_LIMIT as u64).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let body = match item.kind {
        "connector" => {
            let Some(rest) = connector_notes::note_body(&text) else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "frontmatter must end with ---",
                ));
            };
            if rest.trim().is_empty() {
                item.fallback.clone()
            } else {
                rest.to_string()
            }
        }
        _ => {
            let Some(rest) = after_frontmatter(&text) else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "frontmatter must end with ---",
                ));
            };
            rest.to_string()
        }
    };
    if body.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "empty",
        ));
    }
    Ok(body)
}

fn is_real_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_file())
}

fn read_prefix(path: &Path, limit: usize) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut bytes = vec![0_u8; limit];
    let read = file.read(&mut bytes).ok()?;
    bytes.truncate(read);
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn frontmatter(text: &str) -> (Option<String>, Option<String>) {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, None);
    }
    let mut name = None;
    let mut description = None;
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = unquote(value.trim());
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "name" => name = Some(value),
            "description" => description = Some(value),
            _ => {}
        }
    }
    (name, description)
}

fn wiki_frontmatter(text: &str) -> (Option<String>, Option<String>) {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, None);
    }
    let mut title = None;
    let mut description = None;
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = unquote(value.trim());
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "title" => title = Some(value),
            "description" => description = Some(value),
            _ => {}
        }
    }
    (title, description)
}

fn after_frontmatter(text: &str) -> Option<&str> {
    connector_notes::note_body(text)
}

fn unquote(value: &str) -> String {
    let quoted = value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')));
    if quoted {
        value[1..value.len() - 1].to_owned()
    } else {
        value.to_owned()
    }
}

fn one_line(raw: &str) -> String {
    let collapsed: String = raw
        .chars()
        .map(|ch| if ch.is_whitespace() { ' ' } else { ch })
        .filter(|ch| *ch != '\0')
        .collect();
    collapsed.trim().chars().take(DESCRIPTION_LIMIT).collect()
}

fn slug_ok(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn title_ok(title: &str) -> bool {
    let trimmed = title.trim();
    !trimmed.is_empty()
        && trimmed != "."
        && trimmed != ".."
        && !trimmed
            .chars()
            .any(|ch| ch == '/' || ch == '\\' || ch == '\0')
        && trimmed.chars().count() <= 120
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed_note(separator: &str) -> String {
        let mut text = String::new();
        for line in ["---", "name: docs", "description: hello", "---"] {
            text.push_str(line);
            text.push_str(separator);
        }
        text.push_str("Catalog body");
        text
    }

    #[test]
    fn crlf_note_and_asset_body_match_lf_and_a_missing_closer_fails() {
        let lf = framed_note("\n");
        let crlf = framed_note("\r\n");
        let lf_note = connector_notes::note_body(&lf).expect("lf connector note");
        let crlf_note = connector_notes::note_body(&crlf).expect("crlf connector note");
        let lf_asset = after_frontmatter(&lf).expect("lf local asset");
        let crlf_asset = after_frontmatter(&crlf).expect("crlf local asset");
        assert_eq!(lf_note, "Catalog body");
        assert_eq!(crlf_note, lf_note);
        assert_eq!(crlf_asset, lf_asset);
        assert_eq!(crlf_asset, lf_note);
        assert!(crlf_note.starts_with('C'));
        assert!(!crlf_note.starts_with('\r'));
        assert!(!crlf_asset.starts_with('\r'));
        let broken = "---\r\nname: docs\r\ndescription: hello\r\nCatalog body";
        assert!(connector_notes::note_body(broken).is_none());
        assert!(after_frontmatter(broken).is_none());
    }

    #[test]
    fn open_skips_the_file_when_the_tool_is_denied() {
        let dir = std::env::temp_dir().join(format!(
            "a3s-asset-deny-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let skills = dir.join("skills").join("brief-writer");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::write(
            skills.join("SKILL.md"),
            "---\nname: brief-writer\ndescription: \"Write a short brief\"\n---\nBODY-skill-9f3a-unique\n",
        )
        .unwrap();
        let roots = AssetRoots {
            connectors: None,
            skills: Some(dir.join("skills")),
            wiki: None,
        };
        let denied = open_local_asset(
            &roots,
            "skill",
            "brief-writer",
            "default",
            &["Skill".to_string()],
        );
        assert_eq!(denied, Err("denied"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_connector_without_a_usage_note_opens_as_its_transport() {
        let dir = std::env::temp_dir().join(format!(
            "a3s-asset-transport-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let secret = "do-not-leak-connector-cmd-9f3a";
        std::fs::write(
            dir.join("mcp.json"),
            serde_json::json!({
                "mcpServers": {
                    "session-notes": {
                        "type": "stdio",
                        "command": secret,
                        "args": ["--secret", "do-not-leak-env-9f3a"],
                        "env": { "TOKEN": "do-not-leak-token-9f3a" }
                    }
                }
            })
            .to_string(),
        )
        .unwrap();
        let roots = AssetRoots {
            connectors: Some(dir.clone()),
            skills: None,
            wiki: None,
        };
        let catalog = load_catalog(&roots);
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].name, "session-notes");
        assert_eq!(catalog[0].description, "Local stdio connector");
        assert!(!catalog[0].description.contains(secret));
        let opened = open_local_asset(&roots, "connector", "session-notes", "default", &[])
            .expect("transport label");
        assert_eq!(opened, "Local stdio connector");
        assert!(!opened.contains(secret));
        assert!(!opened.contains("do-not-leak-token-9f3a"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
