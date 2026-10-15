//! Standing context admitted before the first model call of a run.
//!
//! The effect log stores only the digest. The system projection stays in the
//! process that admitted it and is rebuilt on resume only when the workspace
//! digest still matches. The digest freezes exactly the standing material the
//! frozen projection embeds: the workspace governance text (`instructions.md`).
//! A later edit to that file does not become this run's system text.
//!
//! Live surfaces are deliberately outside the digest. The workspace memory
//! tree is runtime state: turn-end extraction writes it, recalling a memory
//! rewrites its read telemetry, and the memory tool reads it live mid-run, so
//! freezing it would only fail the turn after the system's own write. The
//! skill catalog is a live context provider by design — a skill installed
//! mid-session is reflected on the next context query — so the `skills/` tree
//! stays out too; a skill that changes the exposed tool surface is recorded
//! through the admitted tool catalog instead. The in-process coding prompt is
//! part of the projection when the opener has an agent, and is not part of the
//! digest, so a later open without that agent still matches the same files.

use std::io::Read;
use std::path::Path;

use a3s_effect::{LogStore, NewFact};
use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::content_digest::digest_json;

pub(crate) const STANDING_CONTEXT_FACT_KIND: &str = "a3s.standing-context";
const STANDING_CONTEXT_FACT_KEY: &str = "standing-context";
const STANDING_CONTEXT_DOMAIN: &str = "a3s.code.standing-context-admission.v1";
const MAX_TEXT_BYTES: usize = 32 * 1024;

#[derive(Serialize)]
struct ContextMaterial<'a> {
    schema: &'static str,
    instructions: &'a str,
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
/// A changed `instructions.md` fails closed. The memory tree and the skill
/// catalog are live surfaces and do not participate in the digest.
/// A later admission whose files still match may record a new tool catalog.
/// The caller supplies its original log; this does not grant writer exclusion.
#[allow(clippy::too_many_arguments)]
pub(crate) fn admit(
    log: &dyn LogStore,
    facts: &[a3s_effect::Fact],
    workspace: &Path,
    thread: &str,
    agent_system: Option<&str>,
    harness_system: &[String],
    tools: &[String],
    tool_rounds: u32,
    step_limit: u32,
    model_attempts: u32,
) -> Result<Option<AdmittedStanding>> {
    let projection = projection_text(workspace, agent_system, harness_system);
    let (digest, context) =
        standing_digest(workspace, tools, tool_rounds, step_limit, model_attempts)?;
    let stored = stored_standings(facts)?;
    if stored.is_empty() {
        if facts
            .iter()
            .any(|fact| fact.kind != STANDING_CONTEXT_FACT_KIND)
        {
            return Ok(None);
        }
        append_standing(log, thread, STANDING_CONTEXT_FACT_KEY, &digest, &context)?;
        return Ok(Some(AdmittedStanding { digest, projection }));
    }
    if stored.iter().any(|item| item.digest == digest) {
        return Ok(Some(AdmittedStanding { digest, projection }));
    }
    if stored.iter().any(|item| item.context != context) {
        let admitted = &stored[0].digest;
        bail!("standing context digest mismatch: admitted {admitted} recomputed {digest}");
    }
    append_standing(log, thread, &digest, &digest, &context)?;
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
    log: &dyn LogStore,
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
    if let Some(system) = agent_system.filter(|text| !text.trim().is_empty()) {
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
    let instructions = instructions_text(workspace).unwrap_or_default();
    let context = digest_json(
        STANDING_CONTEXT_DOMAIN,
        &ContextMaterial {
            schema: STANDING_CONTEXT_DOMAIN,
            instructions: &instructions,
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

fn read_text(path: &Path) -> Option<String> {
    let bytes = read_prefix(path);
    if bytes.is_empty() && !path.is_file() {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn read_prefix(path: &Path) -> Vec<u8> {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut buffer = Vec::new();
    let mut handle = file.take(MAX_TEXT_BYTES as u64);
    let _ = handle.read_to_end(&mut buffer);
    buffer
}

#[cfg(test)]
mod tests;
