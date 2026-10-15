//! Bounded, read-only discovery over the session's authoritative fact log.
//! No session/store constructor is used here: discovery must not create,
//! recover, quarantine, chmod, or activate anything.

use std::collections::{BTreeMap, HashSet};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::fs::{Dir, Metadata, OpenOptions};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{CodeError, Result};

const MAX_ENTRIES: usize = 4096;
const MAX_LOG_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MARKER_BYTES: u64 = 16 * 1024;
const MAX_FACTS: u64 = 100_000;
const MAX_PROMPT_BYTES: usize = 4096;

/// A projection of stored facts, never a second source of session state.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCatalogEntry {
    pub id: String,
    pub cwd: PathBuf,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    /// Exact stored text, truncated only at a UTF-8 boundary for transport.
    pub first_prompt: Option<String>,
    pub first_prompt_truncated: bool,
    pub summary: String,
    /// User messages and model turns, excluding tools and control facts.
    pub num_messages: u64,
    pub fact_count: u64,
}

/// `truncated` is a page limit; `incomplete` reports unscanned or rejected
/// data. Neither is silently converted to an empty, complete inventory.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCatalog {
    pub sessions: Vec<SessionCatalogEntry>,
    pub truncated: bool,
    pub incomplete: bool,
    pub skipped_entries: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    schema: String,
    id: String,
    created_at: DateTime<Utc>,
    // A captured path is descriptive, not authority. Moving a workspace
    // must not grant access to the path named by its old marker.
    workspace: PathBuf,
    effect_log: String,
}

struct Scan {
    remaining_entries: usize,
    remaining_bytes: u64,
    incomplete: bool,
    skipped: usize,
}

impl Scan {
    fn new() -> Self {
        Self {
            remaining_entries: MAX_ENTRIES,
            remaining_bytes: MAX_SCAN_BYTES,
            incomplete: false,
            skipped: 0,
        }
    }

    fn visit(&mut self) -> bool {
        if self.remaining_entries == 0 {
            self.incomplete = true;
            return false;
        }
        self.remaining_entries -= 1;
        true
    }

    fn skip(&mut self) {
        self.incomplete = true;
        self.skipped += 1;
    }
}

/// List one host-selected workspace. Matching uses literal stored strings;
/// it searches the id and the displayed first-prompt prefix (up to 4 KiB),
/// and does not infer a title, entity, model, or session kind.
///
/// Reads are limited to 4,096 directory entries, 16 MiB per log and 64 MiB
/// total. Oversized, changing, corrupt, ambiguous or unreadable sessions are
/// omitted with `incomplete = true`. The caller must expose that signal.
pub fn list_workspace_sessions(
    workspace: &Path,
    limit: usize,
    query: Option<&str>,
) -> Result<SessionCatalog> {
    if !(1..=100).contains(&limit) || query.is_some_and(|q| q.len() > MAX_PROMPT_BYTES) {
        return Err(CodeError::Session("invalid session catalog query".into()));
    }
    let workspace = workspace.canonicalize()?;
    let mut scan = Scan::new();
    let mut entries: BTreeMap<String, Option<SessionCatalogEntry>> = BTreeMap::new();
    if let Some(state) = open_state_dir(&workspace)? {
        if let Some(sessions) = optional_dir(&state, "sessions")? {
            scan_folders(&sessions, &workspace, &mut scan, &mut entries)?;
        }
        if let Some(legacy) = optional_dir(&state, "effect-log")? {
            scan_legacy(&legacy, &workspace, &mut scan, &mut entries)?;
        }
    }
    let mut sessions: Vec<_> = entries
        .into_values()
        .flatten()
        .filter(|entry| {
            query.is_none_or(|query| {
                entry.id.contains(query)
                    || entry
                        .first_prompt
                        .as_deref()
                        .is_some_and(|s| s.contains(query))
            })
        })
        .collect();
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.id.cmp(&b.id)));
    let truncated = sessions.len() > limit;
    sessions.truncate(limit);
    Ok(SessionCatalog {
        sessions,
        truncated,
        incomplete: scan.incomplete,
        skipped_entries: scan.skipped,
    })
}

/// `<workspace>/.a3s`, or the host-set [`crate::state_root`] override.
fn open_state_dir(workspace: &Path) -> io::Result<Option<Dir>> {
    if let Some(root) = crate::state_root::state_root_override() {
        return Dir::open_ambient_dir(root, cap_std::ambient_authority()).map(Some);
    }
    let root = Dir::open_ambient_dir(workspace, cap_std::ambient_authority())?;
    optional_dir(&root, ".a3s")
}

fn optional_dir(parent: &Dir, name: &str) -> io::Result<Option<Dir>> {
    // Each component is opened independently. No repository-controlled
    // intermediate symlink can turn a catalog read into an ambient read.
    match parent.open_dir_nofollow(name) {
        Ok(dir) => Ok(Some(dir)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Resolve one exact stored identity without a page limit or scans of other
/// sessions' logs. This read-only admission check does not activate recovery
/// or certify a later writer's path: activation must retain its own checks.
/// Ambiguous identity, incomplete directory traversal and invalid data fail
/// closed. Legacy records identify only their exact stored thread stem.
pub fn resolve_workspace_session(
    workspace: &Path,
    session_id: &str,
) -> Result<Option<SessionCatalogEntry>> {
    if session_id.is_empty() || session_id.len() > MAX_PROMPT_BYTES {
        return Err(CodeError::Session("invalid session identity".into()));
    }
    let component = super::folder_component(session_id)
        .map_err(|_| CodeError::Session("invalid session identity".into()))?;
    let workspace = workspace.canonicalize()?;
    let Some(state) = open_state_dir(&workspace)? else {
        return Ok(None);
    };
    let mut scan = Scan::new();
    let mut found = None;
    if let Some(sessions) = optional_dir(&state, "sessions")? {
        for date in sessions.entries()? {
            if !scan.visit() {
                return Err(CodeError::Session(
                    "session identity scan limit reached".into(),
                ));
            }
            let date = date?;
            let name = date.file_name();
            let Some(name) = name.to_str() else { continue };
            let Ok(day) = NaiveDate::parse_from_str(name, "%Y-%m-%d") else {
                continue;
            };
            if day.format("%Y-%m-%d").to_string() != name {
                continue;
            }
            let day = sessions.open_dir_nofollow(name)?;
            match day.symlink_metadata(&component) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
                Ok(_) => {}
            }
            if found.is_some() {
                return Err(CodeError::Session("ambiguous session identity".into()));
            }
            let dir = day.open_dir_nofollow(&component)?;
            let entry = read_folder(&dir, &component, &workspace, &mut scan)?;
            if entry.id != session_id {
                return Err(CodeError::Session(
                    "session identity differs from request".into(),
                ));
            }
            found = Some(entry);
        }
    }
    if found.is_some() {
        return Ok(found);
    }
    if session_id != component {
        return Ok(None);
    }
    let Some(legacy) = optional_dir(&state, "effect-log")? else {
        return Ok(None);
    };
    match read_log(
        &legacy,
        &format!("{component}.jsonl"),
        session_id,
        &workspace,
        None,
        &mut scan,
    ) {
        Ok(entry) => Ok(Some(entry)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn scan_folders(
    sessions: &Dir,
    workspace: &Path,
    scan: &mut Scan,
    entries: &mut BTreeMap<String, Option<SessionCatalogEntry>>,
) -> io::Result<()> {
    for date in sessions.entries()? {
        if !scan.visit() {
            break;
        }
        let date = date?;
        let name = date.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(day) = NaiveDate::parse_from_str(name, "%Y-%m-%d") else {
            continue;
        };
        if day.format("%Y-%m-%d").to_string() != name {
            continue;
        }
        let Ok(day) = sessions.open_dir_nofollow(name) else {
            scan.skip();
            continue;
        };
        let children = match day.entries() {
            Ok(children) => children,
            Err(_) => {
                scan.skip();
                continue;
            }
        };
        for child in children {
            if !scan.visit() {
                break;
            }
            let child = match child {
                Ok(child) => child,
                Err(_) => {
                    scan.skip();
                    continue;
                }
            };
            let name = child.file_name();
            let Some(component) = name.to_str() else {
                scan.skip();
                continue;
            };
            // This is the storage identity encoder, not a name heuristic.
            if super::folder_component(component).ok().as_deref() != Some(component) {
                scan.skip();
                continue;
            }
            if let Some(previous) = entries.get_mut(component) {
                if previous.take().is_some() {
                    scan.skip();
                }
                scan.skip();
                continue;
            }
            // Reserve even an invalid folder: the runtime's folder precedence
            // must not be replaced with a different legacy log during listing.
            let entry = day
                .open_dir_nofollow(component)
                .ok()
                .and_then(|dir| read_folder(&dir, component, workspace, scan).ok());
            if entry.is_none() {
                scan.skip();
            }
            entries.insert(component.to_owned(), entry);
        }
    }
    Ok(())
}

fn scan_legacy(
    legacy: &Dir,
    workspace: &Path,
    scan: &mut Scan,
    entries: &mut BTreeMap<String, Option<SessionCatalogEntry>>,
) -> io::Result<()> {
    for child in legacy.entries()? {
        if !scan.visit() {
            break;
        }
        let child = child?;
        let name = child.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(id) = name.strip_suffix(".jsonl") else {
            continue;
        };
        if entries.contains_key(id) {
            continue;
        }
        if super::folder_component(id).ok().as_deref() != Some(id) {
            scan.skip();
            continue;
        }
        let entry = read_log(legacy, name, id, workspace, None, scan).ok();
        if entry.is_none() {
            scan.skip();
        }
        entries.insert(id.to_owned(), entry);
    }
    Ok(())
}

fn read_folder(
    dir: &Dir,
    component: &str,
    workspace: &Path,
    scan: &mut Scan,
) -> io::Result<SessionCatalogEntry> {
    let (raw, _) = read_regular(dir, "session.json", MAX_MARKER_BYTES, scan)?;
    let marker: Marker = serde_json::from_slice(&raw).map_err(invalid_data)?;
    if marker.schema != super::SESSION_FOLDER_SCHEMA
        || marker.id.is_empty()
        || marker.id.len() > 4096
        || super::folder_component(&marker.id).ok().as_deref() != Some(component)
        || marker.effect_log != format!("{component}.jsonl")
        || marker.workspace.as_os_str().is_empty()
    {
        return Err(invalid_data("invalid session marker"));
    }
    read_log(
        dir,
        &marker.effect_log,
        &marker.id,
        workspace,
        Some(marker.created_at),
        scan,
    )
}

fn read_log(
    dir: &Dir,
    name: &str,
    id: &str,
    workspace: &Path,
    created_at: Option<DateTime<Utc>>,
    scan: &mut Scan,
) -> io::Result<SessionCatalogEntry> {
    let (raw, metadata) = read_regular(dir, name, MAX_LOG_BYTES, scan)?;
    let raw = std::str::from_utf8(&raw).map_err(invalid_data)?;
    let mut keys = HashSet::new();
    let mut fact_count = 0;
    let mut num_messages = 0;
    let mut first_prompt = None;
    let mut first_prompt_truncated = false;
    for line in raw.lines().filter(|line| !line.is_empty()) {
        fact_count += 1;
        if fact_count > MAX_FACTS {
            return Err(invalid_data("too many session facts"));
        }
        let fact = a3s_effect::parse_fact_json(line).map_err(invalid_data)?;
        if fact.seq != fact_count || !keys.insert(fact.key) {
            return Err(invalid_data("invalid session fact order or identity"));
        }
        if fact.kind == "user.message" {
            let text = fact
                .payload
                .get("text")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid_data("invalid user message"))?;
            if first_prompt.is_none() {
                first_prompt = Some(bounded_prompt(text));
                first_prompt_truncated = text.len() > MAX_PROMPT_BYTES;
            }
            num_messages += 1;
        } else if fact.kind == "model.turn" {
            // Use the scheduler's typed decision contract, not an inferred
            // payload shape. Tools/control facts are not user/model messages.
            let _: a3s_effect::coding::ModelDecision =
                serde_json::from_value(fact.payload).map_err(invalid_data)?;
            num_messages += 1;
        }
    }
    let updated_at = DateTime::from(metadata.modified()?.into_std());
    let created_at = created_at.or_else(|| {
        metadata
            .created()
            .ok()
            .map(|time| DateTime::from(time.into_std()))
    });
    let summary = first_prompt
        .as_deref()
        .and_then(|text| text.lines().next())
        .filter(|text| !text.is_empty())
        .unwrap_or(id)
        .to_owned();
    Ok(SessionCatalogEntry {
        id: id.to_owned(),
        cwd: workspace.to_owned(),
        created_at,
        updated_at,
        first_prompt,
        first_prompt_truncated,
        summary,
        num_messages,
        fact_count,
    })
}

fn bounded_prompt(text: &str) -> String {
    let mut end = text.len().min(MAX_PROMPT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn read_regular(
    dir: &Dir,
    name: &str,
    max: u64,
    scan: &mut Scan,
) -> io::Result<(Vec<u8>, Metadata)> {
    let before = dir.symlink_metadata(name)?;
    if !before.is_file() {
        return Err(invalid_data("session data must be a regular file"));
    }
    let limit = max.min(scan.remaining_bytes);
    if before.len() > limit {
        return Err(invalid_data("session catalog byte limit"));
    }
    let (file, opened) = open_regular(dir, name, limit)?;
    let mut bytes = Vec::new();
    (&file).take(limit + 1).read_to_end(&mut bytes)?;
    scan.remaining_bytes = scan.remaining_bytes.saturating_sub(bytes.len() as u64);
    let after = file.metadata()?;
    if bytes.len() as u64 > limit
        || bytes.len() as u64 != opened.len()
        || after.len() != opened.len()
        || after.modified()? != opened.modified()?
    {
        return Err(invalid_data(
            "session data changed while reading or exceeded its limit",
        ));
    }
    Ok((bytes, after))
}

fn open_regular(dir: &Dir, name: &str, limit: u64) -> io::Result<(cap_std::fs::File, Metadata)> {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No).nonblock(true);
    let file = dir.open_with(name, &options)?;
    // Recheck the opened handle. O_NONBLOCK covers a FIFO swapped in after
    // the pre-open gate; no-follow and the parent handles cover symlink races.
    let opened = file.metadata()?;
    if !opened.is_file() || opened.len() > limit {
        return Err(invalid_data("session data changed before reading"));
    }
    Ok((file, opened))
}

fn invalid_data(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests;
