//! Original legacy goal persistence, scoped by the existing session identity.

use super::{AppliedGoal, DurableGoal, GoalOp};
use a3s_code_core::{AgentSession, CodeError};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

fn goal_file(workspace: &Path, session_id: &str) -> Result<PathBuf> {
    Ok(
        match a3s_code_core::session_folder::find_session_dir(workspace, session_id)? {
            Some(directory) => directory.join("durable-goal.json"),
            None => a3s_code_core::session_folder::legacy_effect_log_dir(workspace).join(format!(
                "{}.durable-goal.json",
                a3s_code_core::fact_control::thread_for_session(session_id)
            )),
        },
    )
}

pub(crate) async fn load_goal(workspace: &Path, session_id: &str) -> Result<Option<DurableGoal>> {
    let path = goal_file(workspace, session_id)?;
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        other => {
            other.context("legacy session goal unavailable")?;
        }
    }
    let relative = path.strip_prefix(workspace)?.to_string_lossy();
    let (_, raw) = super::import::read_source_at(workspace, &relative, 16 * 1024 * 1024).await?;
    let goal: DurableGoal = serde_json::from_str(&raw).context("invalid session goal")?;
    if goal.objective.trim().is_empty() {
        anyhow::bail!("empty session goal");
    }
    Ok(Some(goal))
}

fn state_error(error: impl std::fmt::Display) -> CodeError {
    CodeError::Session(format!("legacy session goal: {error}"))
}

async fn store_goal(session: &AgentSession, goal: Option<&DurableGoal>) -> Result<()> {
    let path = goal_file(session.workspace(), session.session_id())?;
    if let Some(goal) = goal {
        let raw = serde_json::to_string_pretty(goal)?;
        if raw.len() > 16 * 1024 * 1024 {
            anyhow::bail!("session goal exceeds read bound");
        }
        let services = a3s_code_core::workspace::WorkspaceServices::local(session.workspace());
        let relative = path.strip_prefix(session.workspace())?.to_string_lossy();
        let path = services.normalize_path(relative.as_ref())?;
        services.fs().write_text(&path, &raw).await?;
    } else {
        match std::fs::remove_file(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            other => {
                other?;
            }
        }
    }
    Ok(())
}

pub(crate) async fn apply_session_goal(session: &AgentSession, op: GoalOp) -> Result<AppliedGoal> {
    Ok(session
        .with_legacy_goal_state(|| async {
            let current = load_goal(session.workspace(), session.session_id())
                .await
                .map_err(state_error)?;
            let applied = super::apply_goal(current.clone(), op);
            if applied.next != current {
                store_goal(session, applied.next.as_ref())
                    .await
                    .map_err(state_error)?;
            }
            if let Some(mode) = applied.plan.mode() {
                session.set_planning_mode(mode)?;
            } else if applied.plan.clears_override() {
                session.clear_planning_mode_override()?;
            }
            Ok(applied)
        })
        .await?)
}

/// Called after the original worker has settled; a stale event cannot erase newer state.
pub(crate) async fn retire_achieved_durable_goal(
    session: &AgentSession,
    governing: Option<&DurableGoal>,
) -> Result<bool> {
    let Some(governing) = governing.filter(|goal| !goal.paused) else {
        return Ok(false);
    };
    Ok(session
        .with_legacy_goal_state(|| async {
            let current = load_goal(session.workspace(), session.session_id())
                .await
                .map_err(state_error)?;
            if current.as_ref() != Some(governing) {
                return Ok(false);
            }
            store_goal(session, None).await.map_err(state_error)?;
            session.clear_planning_mode_override()?;
            Ok(true)
        })
        .await?)
}

#[cfg(test)]
mod tests;
