//! Retrieval over the session's saved wiki pages.
//!
//! Search reads the published a3s-vec generation, then keeps a hit only when that page's parser generation is current.
//! It does not wait for an in-flight build, and it does not change the permission policy.

use std::sync::{Arc, Mutex};

use a3s_code_core::capability::{WorkspaceOperation, WorkspaceOperationAccess};
use a3s_code_core::tools::{Tool, ToolContext, ToolOutput};
use a3s_code_core::workspace::{WikiIndex, WikiSearchHit, WikiSearchPhase, WikiSearchResult};
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::document_read::DocumentBridge;

const TOOL_NAME: &str = "search_wiki";
const HOST_METHOD: &str = "a3s/encyclopedia/search";

pub struct SearchWikiTool {
    index: Option<Arc<WikiIndex>>,
    bridge: Option<Arc<DocumentBridge>>,
    denies: Arc<Mutex<Vec<String>>>,
}

impl SearchWikiTool {
    pub fn new(
        index: Option<Arc<WikiIndex>>,
        bridge: Option<Arc<DocumentBridge>>,
        denies: Arc<Mutex<Vec<String>>>,
    ) -> Self {
        if let Some(index) = &index {
            publish_index(index);
        }
        Self {
            index,
            bridge,
            denies,
        }
    }
}

/// One process keeps the index `search_wiki` already searches.
/// A second index would publish a different generation.
type PublishedIndexes = Mutex<Vec<(std::path::PathBuf, std::sync::Weak<WikiIndex>)>>;

fn indexes() -> &'static PublishedIndexes {
    static INDEXES: std::sync::OnceLock<PublishedIndexes> = std::sync::OnceLock::new();
    INDEXES.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn publish_index(index: &Arc<WikiIndex>) {
    let path = std::fs::canonicalize(index.root()).unwrap_or_else(|_| index.root());
    let mut guard = indexes()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    guard.retain(|(registered, weak)| weak.upgrade().is_some() && registered != &path);
    guard.push((path, Arc::downgrade(index)));
}

/// Search the index registered for `wiki`. Missing and in-flight generations are not-ready.
pub fn search_published(wiki: &std::path::Path, query: &str) -> WikiSearchResult {
    let query = query.trim();
    if query.is_empty() || query.contains('\0') || query.chars().count() > 2_000 {
        return idle_search();
    }
    let wanted = std::fs::canonicalize(wiki).unwrap_or_else(|_| wiki.to_path_buf());
    let index = {
        let guard = indexes()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        guard.iter().find_map(|(path, weak)| {
            let registered = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            (registered == wanted).then(|| weak.upgrade()).flatten()
        })
    };
    match index {
        Some(index) => {
            let mut result = index.search(query).unwrap_or_else(|_| idle_search());
            if result.phase == WikiSearchPhase::Published {
                result.hits.retain(|hit| concept_is_current(wiki, hit));
            }
            result
        }
        None => idle_search(),
    }
}

fn concept_is_current(wiki: &std::path::Path, hit: &WikiSearchHit) -> bool {
    let candidate = std::path::PathBuf::from(&hit.path);
    let path = if candidate.is_absolute() {
        candidate
    } else {
        wiki.join(&hit.path)
    };
    let text =
        std::fs::read_to_string(&path).or_else(|_| std::fs::read_to_string(wiki.join(&hit.name)));
    text.is_ok_and(|body| a3s_okf::published_parser(&body) == Some(a3s_okf::PARSER_GENERATION))
}

pub fn search_published_body(wiki: &std::path::Path, query: &str) -> Value {
    let result = search_published(wiki, query);
    let phase = match result.phase {
        WikiSearchPhase::NotReady => "not-ready",
        WikiSearchPhase::Published => "published",
    };
    let hits = if result.phase == WikiSearchPhase::NotReady {
        Vec::new()
    } else {
        result
            .hits
            .iter()
            .map(|hit| json!({ "name": hit.name, "passage": hit.passage }))
            .collect()
    };
    json!({
        "phase": phase,
        "source_revision": result.source_revision,
        "hits": hits,
    })
}

fn idle_search() -> WikiSearchResult {
    WikiSearchResult {
        phase: WikiSearchPhase::NotReady,
        lexical_engine: "a3s_vec_fts_v1",
        source_revision: 0,
        catalog_revision: 0,
        hits: Vec::new(),
    }
}

#[async_trait]
impl Tool for SearchWikiTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn description(&self) -> &str {
        "Search one wiki source. scope=agent with agent set reads that agent's encyclopedia under wiki/agents/{agent}. scope=local reads personal notes and skips agent encyclopedias. scope=team reads the organization encyclopedia. Omit scope to search personal notes and the team encyclopedia together. Answer from the passages when they match. no-match means that source has nothing. not-ready means the local index is still building. This does not grant another capability."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Plain-text relevance query for the saved wiki."
                },
                "scope": {
                    "type": "string",
                    "enum": ["agent", "local", "team"],
                    "description": "Which wiki to search. Omit it to search personal notes and the team encyclopedia."
                },
                "agent": {
                    "type": "string",
                    "description": "Agent folder id. Required when scope is agent."
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let denies = self
            .denies
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poison| poison.into_inner().clone());
        if wiki_read_denied(&denies) {
            return Ok(ToolOutput::error("denied"));
        }
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if query.is_empty() {
            return Ok(ToolOutput::error("missing query"));
        }
        if query.contains('\0') || query.chars().count() > 2_000 {
            return Ok(ToolOutput::error("invalid query"));
        }
        let scope = args
            .get("scope")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let agent = args
            .get("agent")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if !scope.is_empty() && scope != "agent" && scope != "local" && scope != "team" {
            return Ok(ToolOutput::error("invalid scope"));
        }
        if scope == "agent" && !agent_folder_ok(agent) {
            return Ok(ToolOutput::error("invalid agent"));
        }
        let query = query.to_owned();
        let local = if scope == "team" {
            None
        } else if let Some(index) = &self.index {
            // The native a3s-vec FTS query runs synchronously under a std RwLock;
            // keep it off the async runtime's worker threads exactly like the
            // durable BM25 path does.
            let root = index.root().to_path_buf();
            let index = Arc::clone(index);
            let searched = query.clone();
            let result = tokio::task::spawn_blocking(move || index.search(searched))
                .await
                .map_err(|error| anyhow::anyhow!("wiki search task failed: {error}"))?;
            match result {
                Ok(mut result) => {
                    if scope == "local" || scope == "agent" {
                        result
                            .hits
                            .retain(|hit| hit_in_scope(&hit.path, scope, agent));
                    }
                    let result = bound_hits(result);
                    note_retrieved_pages(&root, &result);
                    Some(render(result))
                }
                Err(error) => return Ok(ToolOutput::error(error.to_string())),
            }
        } else {
            None
        };
        let host = if scope == "local" || scope == "agent" {
            Ok(Vec::new())
        } else {
            self.host_passages(ctx, &query).await
        };
        Ok(merge(local, host))
    }
}

struct HostHit {
    name: String,
    place: String,
    body: String,
}

async fn host_passages_of(
    bridge: &DocumentBridge,
    access: WorkspaceOperationAccess,
    query: &str,
) -> Result<Vec<HostHit>, String> {
    let value = bridge
        .ask(access, HOST_METHOD, json!({ "query": query }))
        .await
        .map_err(|error| error.to_string())?;
    let Some(rows) = value.get("passages").and_then(Value::as_array) else {
        return Err("encyclopedia search response has no passages".into());
    };
    Ok(rows.iter().filter_map(host_hit).collect())
}

fn host_hit(row: &Value) -> Option<HostHit> {
    let body = row.get("body").and_then(Value::as_str)?.trim();
    if body.is_empty() {
        return None;
    }
    let name = row.get("name").and_then(Value::as_str).unwrap_or("").trim();
    let place = row
        .get("place")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    Some(HostHit {
        name: if name.is_empty() {
            "encyclopedia".into()
        } else {
            name.to_owned()
        },
        place: place.to_owned(),
        body: body.to_owned(),
    })
}

fn render_hits(hits: &[HostHit]) -> String {
    hits.iter()
        .map(|hit| format!("{}\n{}\n{}", hit.name, hit.place, hit.body))
        .collect::<Vec<_>>()
        .join("\n\n")
}

impl SearchWikiTool {
    async fn host_passages(&self, ctx: &ToolContext, query: &str) -> Result<Vec<HostHit>, String> {
        let Some(bridge) = &self.bridge else {
            return Ok(Vec::new());
        };
        let access = ctx
            .workspace_operation(WorkspaceOperation::Read)
            .map_err(|error| error.to_string())?;
        host_passages_of(bridge, access, query).await
    }
}

fn merge(local: Option<ToolOutput>, host: Result<Vec<HostHit>, String>) -> ToolOutput {
    let failure = |local: Option<&str>, error: String| {
        let reason = error.chars().take(1024).collect::<String>();
        let note = format!("organization encyclopedia search failed: {reason}");
        let content = match local {
            None | Some("no-match") => note,
            Some("not-ready") => format!("not-ready\n{note}"),
            Some(text) => format!("{text}\n\n{note}"),
        };
        ToolOutput::error(content)
    };
    match (local, host) {
        (None, Err(error)) => failure(None, error),
        (None, Ok(hits)) if hits.is_empty() => ToolOutput::success("no-match"),
        (None, Ok(hits)) => ToolOutput::success(render_hits(&hits)),
        (Some(output), Err(error)) => failure(Some(output.content.as_str()), error),
        (Some(output), Ok(hits)) if hits.is_empty() => output,
        (Some(mut output), Ok(hits)) => {
            let extra = render_hits(&hits);
            if output.content == "no-match" || output.content == "not-ready" {
                output.content = extra;
            } else {
                output.content.push_str("\n\n");
                output.content.push_str(&extra);
            }
            output
        }
    }
}

#[cfg(test)]
mod tests;

fn agent_folder_ok(agent: &str) -> bool {
    let mut chars = agent.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    let mut count = 1usize;
    for character in chars {
        count += 1;
        if count > 64
            || !(character.is_ascii_alphanumeric() || character == '-' || character == '_')
        {
            return false;
        }
    }
    true
}

fn hit_in_scope(path: &str, scope: &str, agent: &str) -> bool {
    let text = path.replace('\\', "/");
    let parts: Vec<&str> = text.split('/').filter(|part| !part.is_empty()).collect();
    let owned = parts
        .windows(2)
        .find(|pair| pair[0] == "agents")
        .map(|pair| pair[1]);
    match scope {
        "local" => owned.is_none(),
        "agent" => owned == Some(agent),
        _ => false,
    }
}

fn wiki_read_denied(denies: &[String]) -> bool {
    denies.iter().any(|rule| {
        let rule = rule.trim();
        rule == "open_local_asset" || rule == TOOL_NAME
    })
}

fn bound_hits(mut result: WikiSearchResult) -> WikiSearchResult {
    let mut chars = 0usize;
    let mut hits = Vec::new();
    for hit in result.hits {
        if hits.len() == 12 {
            break;
        }
        let next = hit.passage.chars().count();
        if chars + next > 24_000 {
            break;
        }
        chars += next;
        hits.push(hit);
    }
    result.hits = hits;
    result
}

/// The documents list reads `{workspace}/.a3s-agent-retrievals`, one canonical path per retrieval.
fn note_retrieved_pages(wiki_root: &std::path::Path, result: &WikiSearchResult) {
    if !matches!(result.phase, WikiSearchPhase::Published) {
        return;
    }
    let workspace = match (
        wiki_root.file_name().and_then(|name| name.to_str()),
        wiki_root.parent(),
    ) {
        (Some("local"), Some(wiki))
            if wiki.file_name().and_then(|name| name.to_str()) == Some("wiki") =>
        {
            wiki.parent().unwrap_or(wiki_root)
        }
        _ => wiki_root,
    };
    let mut seen = std::collections::HashSet::new();
    let mut lines = String::new();
    for hit in &result.hits {
        let relative = hit.path.trim_matches('/');
        if relative.is_empty() || relative == "." || relative.contains('\0') {
            continue;
        }
        let Ok(canon) = std::fs::canonicalize(wiki_root.join(relative)) else {
            continue;
        };
        if !canon.starts_with(workspace) || !canon.is_file() || !seen.insert(canon.clone()) {
            continue;
        }
        lines.push_str(&canon.to_string_lossy());
        lines.push('\n');
    }
    if lines.is_empty() {
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(workspace.join(".a3s-agent-retrievals"))
    {
        use std::io::Write;
        let _ = file.write_all(lines.as_bytes());
    }
}

fn render(result: WikiSearchResult) -> ToolOutput {
    let names: Vec<&str> = result.hits.iter().map(|hit| hit.name.as_str()).collect();
    let phase = match result.phase {
        WikiSearchPhase::NotReady => "not-ready",
        WikiSearchPhase::Published => "published",
    };
    let content = match result.phase {
        WikiSearchPhase::NotReady => "not-ready".to_owned(),
        WikiSearchPhase::Published if result.hits.is_empty() => "no-match".to_owned(),
        WikiSearchPhase::Published => result
            .hits
            .iter()
            .map(|hit| format!("{}\n{}\n{}", hit.name, hit.path, hit.passage))
            .collect::<Vec<_>>()
            .join("\n\n"),
    };
    ToolOutput::success(content).with_metadata(json!({
        "phase": phase,
        "lexical_engine": result.lexical_engine,
        "source_revision": result.source_revision,
        "catalog_revision": result.catalog_revision,
        "names": names,
    }))
}
