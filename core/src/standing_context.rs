//! Standing context admitted before the first model call of a run.
//!
//! The effect log stores only the digest. The system projection stays in the
//! process that admitted it and is rebuilt on resume only when the workspace
//! digest still matches. A later edit to `instructions.md`, `skills/`, or the
//! workspace memory directory does not become this run's system text. The
//! in-process coding prompt is part of the projection when the opener has an
//! agent, and is not part of the digest, so a later open without that agent
//! still matches the same files.

use std::fs;
use std::io::Read;
use std::path::Path;

use a3s_effect::{FileLog, LogStore, NewFact};
use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::content_digest::{digest_bytes, digest_json};

pub(crate) const STANDING_CONTEXT_FACT_KIND: &str = "a3s.standing-context";
const STANDING_CONTEXT_FACT_KEY: &str = "standing-context";
const STANDING_CONTEXT_DOMAIN: &str = "a3s.code.standing-context-admission.v1";
const MAX_TEXT_BYTES: usize = 32 * 1024;
const MAX_TREE_FILES: usize = 64;

#[derive(Serialize)]
struct ContextMaterial<'a> {
    schema: &'static str,
    instructions: &'a str,
    memory: &'a str,
    skills: &'a str,
}

#[derive(Serialize)]
struct StandingMaterial<'a> {
    schema: &'static str,
    context: &'a str,
    tools: &'a [String],
    tool_rounds: u32,
    step_limit: u32,
    model_attempts: u32,
}

struct StoredStanding {
    digest: String,
    context: String,
}

#[derive(Debug)]
pub(crate) struct AdmittedStanding {
    pub digest: String,
    pub projection: String,
}

/// Admit the standing projection for one thread.
///
/// An empty log gains one digest fact. A log that already has other facts and
/// no digest is a legacy run: it stays loadable and gets no synthetic digest.
/// A changed `instructions.md`, skill tree, or memory tree fails closed.
/// A later admission whose files still match may record a new tool catalog.
#[allow(clippy::too_many_arguments)]
pub(crate) fn admit(
    log_dir: &Path,
    thread: &str,
    agent_system: Option<&str>,
    harness_system: &[String],
    tools: &[String],
    tool_rounds: u32,
    step_limit: u32,
    model_attempts: u32,
) -> Result<Option<AdmittedStanding>> {
    let workspace = log_dir
        .parent()
        .and_then(Path::parent)
        .context("effect log is not under <workspace>/.a3s/effect-log")?;
    let projection = projection_text(workspace, agent_system, harness_system);
    let (digest, context) =
        standing_digest(workspace, tools, tool_rounds, step_limit, model_attempts)?;
    let log = FileLog::open(log_dir).map_err(|error| anyhow::anyhow!(error))?;
    let facts = log.read(thread).map_err(|error| anyhow::anyhow!(error))?;
    let stored = stored_standings(&facts)?;
    if stored.is_empty() {
        if facts
            .iter()
            .any(|fact| fact.kind != STANDING_CONTEXT_FACT_KIND)
        {
            return Ok(None);
        }
        append_standing(&log, thread, STANDING_CONTEXT_FACT_KEY, &digest, &context)?;
        return Ok(Some(AdmittedStanding { digest, projection }));
    }
    if stored.iter().any(|item| item.digest == digest) {
        return Ok(Some(AdmittedStanding { digest, projection }));
    }
    if stored.iter().any(|item| item.context != context) {
        let admitted = &stored[0].digest;
        bail!("standing context digest mismatch: admitted {admitted} recomputed {digest}");
    }
    append_standing(&log, thread, &digest, &digest, &context)?;
    Ok(Some(AdmittedStanding { digest, projection }))
}

fn stored_standings(facts: &[a3s_effect::Fact]) -> Result<Vec<StoredStanding>> {
    let mut stored = Vec::new();
    for fact in facts {
        if fact.kind != STANDING_CONTEXT_FACT_KIND {
            continue;
        }
        let Some(digest) = fact.payload.get("digest").and_then(|value| value.as_str()) else {
            bail!("standing context fact is missing a digest");
        };
        crate::content_digest::validate_digest(digest)
            .map_err(|_| anyhow::anyhow!("standing context digest is malformed"))?;
        let Some(context) = fact.payload.get("context").and_then(|value| value.as_str()) else {
            bail!("standing context fact is missing a context digest");
        };
        crate::content_digest::validate_digest(context)
            .map_err(|_| anyhow::anyhow!("standing context digest is malformed"))?;
        stored.push(StoredStanding {
            digest: digest.to_string(),
            context: context.to_string(),
        });
    }
    Ok(stored)
}

fn append_standing(
    log: &FileLog,
    thread: &str,
    key: &str,
    digest: &str,
    context: &str,
) -> Result<()> {
    log.append(
        thread,
        &[NewFact {
            kind: STANDING_CONTEXT_FACT_KIND.to_string(),
            key: key.to_string(),
            payload: serde_json::json!({ "digest": digest, "context": context }),
        }],
        None,
    )
    .map_err(|error| anyhow::anyhow!(error))?;
    Ok(())
}

fn projection_text(
    workspace: &Path,
    agent_system: Option<&str>,
    harness_system: &[String],
) -> String {
    let mut parts = Vec::new();
    if let Some(system) = agent_system.map(str::trim).filter(|text| !text.is_empty()) {
        parts.push(system.to_string());
    }
    for line in harness_system {
        let line = line.trim();
        if !line.is_empty() {
            parts.push(line.to_string());
        }
    }
    if let Some(instructions) = instructions_text(workspace) {
        if !instructions.is_empty() {
            parts.push(instructions);
        }
    }
    parts.join("\n\n")
}

fn instructions_text(workspace: &Path) -> Option<String> {
    let instructions = read_text(&workspace.join("instructions.md"))?;
    let instructions = instructions.trim();
    if instructions.is_empty() {
        None
    } else {
        Some(instructions.to_string())
    }
}

fn standing_digest(
    workspace: &Path,
    tools: &[String],
    tool_rounds: u32,
    step_limit: u32,
    model_attempts: u32,
) -> Result<(String, String)> {
    let mut tools = tools.to_vec();
    tools.sort();
    let memory = tree_digest(&workspace.join(".a3s").join("memory"))?;
    let skills = tree_digest(&workspace.join("skills"))?;
    let instructions = instructions_text(workspace).unwrap_or_default();
    let context = digest_json(
        STANDING_CONTEXT_DOMAIN,
        &ContextMaterial {
            schema: STANDING_CONTEXT_DOMAIN,
            instructions: &instructions,
            memory: &memory,
            skills: &skills,
        },
    )
    .context("standing context digest")?;
    let digest = digest_json(
        STANDING_CONTEXT_DOMAIN,
        &StandingMaterial {
            schema: STANDING_CONTEXT_DOMAIN,
            context: &context,
            tools: &tools,
            tool_rounds,
            step_limit,
            model_attempts,
        },
    )
    .context("standing context digest")?;
    Ok((digest, context))
}

fn tree_digest(root: &Path) -> Result<String> {
    if !root.is_dir() || root.is_symlink() {
        return Ok(String::new());
    }
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    let mut body = Vec::new();
    for (path, bytes) in files {
        body.extend(path.as_bytes());
        body.push(0);
        body.extend((bytes.len() as u64).to_le_bytes());
        body.extend(bytes);
        body.push(0xff);
    }
    Ok(digest_bytes(STANDING_CONTEXT_DOMAIN, &body))
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    if out.len() >= MAX_TREE_FILES {
        return Ok(());
    }
    let entries = fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        paths.push(entry?.path());
    }
    paths.sort();
    for path in paths {
        if out.len() >= MAX_TREE_FILES {
            break;
        }
        if path.is_symlink() {
            continue;
        }
        if path.is_dir() {
            collect_files(root, &path, out)?;
            continue;
        }
        let Some(relative) = path
            .strip_prefix(root)
            .ok()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
        else {
            continue;
        };
        out.push((relative, read_prefix(&path)));
    }
    Ok(())
}

fn read_text(path: &Path) -> Option<String> {
    let bytes = read_prefix(path);
    if bytes.is_empty() && !path.is_file() {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn read_prefix(path: &Path) -> Vec<u8> {
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    let mut buffer = Vec::new();
    let mut handle = file.take(MAX_TEXT_BYTES as u64);
    let _ = handle.read_to_end(&mut buffer);
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_change_the_digest_and_the_fact_keeps_only_the_digest() {
        let workspace = tempfile::tempdir().unwrap();
        let log_dir = workspace.path().join(".a3s").join("effect-log");
        std::fs::write(workspace.path().join("instructions.md"), "OPEN-TIME\n").unwrap();
        let admitted = admit(
            &log_dir,
            "standing-test",
            Some("base prompt"),
            &["careful".into()],
            &["read".into(), "bash".into()],
            4,
            32,
            2,
        )
        .unwrap()
        .expect("new log");
        assert!(admitted.projection.contains("OPEN-TIME"));
        assert!(admitted.projection.contains("base prompt"));
        let facts = FileLog::open(&log_dir)
            .unwrap()
            .read("standing-test")
            .unwrap();
        let payload = facts[0].payload.to_string();
        assert!(payload.contains(&admitted.digest));
        assert!(!payload.contains("OPEN-TIME"));
        assert!(!payload.contains("base prompt"));

        std::fs::write(workspace.path().join("instructions.md"), "CHANGED\n").unwrap();
        let error = admit(
            &log_dir,
            "standing-test",
            Some("base prompt"),
            &["careful".into()],
            &["read".into(), "bash".into()],
            4,
            32,
            2,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("standing context digest mismatch"));
    }

    #[test]
    fn a_legacy_log_does_not_gain_a_synthetic_digest() {
        let workspace = tempfile::tempdir().unwrap();
        let log_dir = workspace.path().join(".a3s").join("effect-log");
        let log = FileLog::open(&log_dir).unwrap();
        log.append(
            "legacy",
            &[NewFact {
                kind: "user.message".into(),
                key: "m-0".into(),
                payload: serde_json::json!({ "text": "already running" }),
            }],
            None,
        )
        .unwrap();
        let admitted = admit(&log_dir, "legacy", None, &[], &[], 1, 32, 2).unwrap();
        assert!(admitted.is_none());
        let facts = log.read("legacy").unwrap();
        assert!(facts
            .iter()
            .all(|fact| fact.kind != STANDING_CONTEXT_FACT_KIND));
    }

    #[test]
    fn a_malformed_standing_fact_fails_closed() {
        let workspace = tempfile::tempdir().unwrap();
        let log_dir = workspace.path().join(".a3s").join("effect-log");
        let log = FileLog::open(&log_dir).unwrap();
        log.append(
            "broken",
            &[NewFact {
                kind: STANDING_CONTEXT_FACT_KIND.into(),
                key: STANDING_CONTEXT_FACT_KEY.into(),
                payload: serde_json::json!({ "digest": "not-a-digest" }),
            }],
            None,
        )
        .unwrap();
        log.append(
            "broken",
            &[NewFact {
                kind: "user.message".into(),
                key: "m-0".into(),
                payload: serde_json::json!({ "text": "already running" }),
            }],
            None,
        )
        .unwrap();
        let error = admit(&log_dir, "broken", None, &[], &[], 1, 32, 2).unwrap_err();
        assert!(error.to_string().contains("malformed"));
    }

    #[test]
    fn a_new_tool_catalog_admits_again_when_the_files_stay() {
        let workspace = tempfile::tempdir().unwrap();
        let log_dir = workspace.path().join(".a3s").join("effect-log");
        std::fs::write(workspace.path().join("instructions.md"), "OPEN-TIME\n").unwrap();
        let first = admit(
            &log_dir,
            "tools",
            Some("base"),
            &[],
            &["read".into()],
            4,
            32,
            2,
        )
        .unwrap()
        .expect("first admission");
        let second = admit(
            &log_dir,
            "tools",
            Some("base"),
            &[],
            &["bash".into(), "read".into()],
            4,
            32,
            2,
        )
        .unwrap()
        .expect("tool catalog change");
        assert_ne!(first.digest, second.digest);
        assert_eq!(first.projection, second.projection);
        let again = admit(
            &log_dir,
            "tools",
            Some("base"),
            &[],
            &["read".into()],
            4,
            32,
            2,
        )
        .unwrap()
        .expect("original catalog");
        assert_eq!(again.digest, first.digest);
        std::fs::write(workspace.path().join("instructions.md"), "CHANGED\n").unwrap();
        let error = admit(
            &log_dir,
            "tools",
            Some("base"),
            &[],
            &["bash".into()],
            4,
            32,
            2,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("standing context digest mismatch"));
    }

    #[test]
    fn skills_and_memory_changes_fail_closed() {
        let workspace = tempfile::tempdir().unwrap();
        let log_dir = workspace.path().join(".a3s").join("effect-log");
        let skills = workspace.path().join("skills");
        let memory = workspace.path().join(".a3s").join("memory");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::create_dir_all(&memory).unwrap();
        std::fs::write(skills.join("skill.md"), "SKILL-OPEN\n").unwrap();
        std::fs::write(memory.join("note.md"), "MEMORY-OPEN\n").unwrap();
        let tools = ["read".to_string()];
        let first = admit(&log_dir, "files", None, &[], &tools, 4, 32, 2)
            .unwrap()
            .expect("first admission");
        let payload = FileLog::open(&log_dir).unwrap().read("files").unwrap()[0]
            .payload
            .to_string();
        assert!(payload.contains(&first.digest));
        assert!(!payload.contains("SKILL-OPEN"));
        assert!(!payload.contains("MEMORY-OPEN"));

        std::fs::write(skills.join("skill.md"), "SKILL-CHANGED\n").unwrap();
        let skill_error = admit(&log_dir, "files", None, &[], &tools, 4, 32, 2).unwrap_err();
        assert!(skill_error
            .to_string()
            .contains("standing context digest mismatch"));

        std::fs::write(skills.join("skill.md"), "SKILL-OPEN\n").unwrap();
        std::fs::write(memory.join("note.md"), "MEMORY-CHANGED\n").unwrap();
        let memory_error = admit(&log_dir, "files", None, &[], &tools, 4, 32, 2).unwrap_err();
        assert!(memory_error
            .to_string()
            .contains("standing context digest mismatch"));

        std::fs::write(memory.join("note.md"), "MEMORY-OPEN\n").unwrap();
        let restored = admit(&log_dir, "files", None, &[], &tools, 4, 32, 2)
            .unwrap()
            .expect("restored files");
        assert_eq!(restored.digest, first.digest);
    }
}
