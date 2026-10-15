//! Explicit legacy source selection and original writer-bound import admission.

use super::DurableGoal;
use a3s_code_core::execution_identity::ExecutionIdentityV1;
use a3s_code_core::goal_control::{
    DurableGoalDefinitionV1, GoalDefinitionCommandV1, GOAL_LEGACY_SOURCE_DOMAIN_V1,
};
use a3s_code_core::workspace::{WorkspaceRef, WorkspaceServices};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

const SOURCE_PATH: &str = ".a3s/durable-goal.json";
const MAX_SOURCE_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GoalImportPreviewRequest {
    pub session_id: String,
    #[serde(deserialize_with = "Option::<ExecutionIdentityV1>::deserialize")]
    pub expected_source: Option<ExecutionIdentityV1>,
}

#[derive(Debug, Serialize)]
pub(crate) struct GoalImportPreview {
    session_id: String,
    workspace: WorkspaceRef,
    source: ExecutionIdentityV1,
    goal: DurableGoal,
}

fn parse_source(
    session_id: &str,
    workspace: &WorkspaceRef,
    raw: &str,
    expected: Option<&ExecutionIdentityV1>,
) -> Result<GoalImportPreview> {
    if raw.len() > MAX_SOURCE_BYTES {
        bail!("goal.legacy_ambiguous: legacy source exceeds 64 KiB");
    }
    let goal: DurableGoal = serde_json::from_str(raw)
        .context("goal.legacy_ambiguous: expected one exact objective/paused record")?;
    if goal.objective.trim().is_empty() || goal.objective.len() > 4096 {
        bail!("goal.legacy_ambiguous: objective must contain 1–4096 UTF-8 bytes");
    }
    let source = ExecutionIdentityV1::derive(
        GOAL_LEGACY_SOURCE_DOMAIN_V1,
        &(session_id, workspace, SOURCE_PATH, raw),
    )?;
    if let Some(expected) = expected {
        expected.validate()?;
        if expected != &source {
            bail!("goal.conflict: legacy source changed or belongs to another session");
        }
    }
    Ok(GoalImportPreview {
        session_id: session_id.to_owned(),
        workspace: workspace.clone(),
        source,
        goal,
    })
}

pub(crate) async fn preview_import(
    workspace: &Path,
    request: &GoalImportPreviewRequest,
) -> Result<GoalImportPreview> {
    let (workspace, raw) = read_source(workspace, MAX_SOURCE_BYTES).await?;
    parse_source(
        &request.session_id,
        &workspace,
        &raw,
        request.expected_source.as_ref(),
    )
}

pub(crate) async fn commit_import(
    session: &a3s_code_core::AgentSession,
    command: GoalDefinitionCommandV1,
) -> Result<DurableGoalDefinitionV1> {
    let source = command
        .import_source
        .clone()
        .context("goal.conflict: import source required")?;
    let objective = command.goal.description.clone();
    let request = GoalImportPreviewRequest {
        session_id: command.session_id.clone(),
        expected_source: Some(source.identity),
    };
    Ok(session
        .apply_goal_import(command, || async {
            let selected = preview_import(session.workspace(), &request)
                .await
                .map_err(|error| a3s_code_core::CodeError::Session(error.to_string()))?;
            if selected.goal.objective != objective || selected.goal.paused != source.paused {
                return Err(a3s_code_core::CodeError::Session(
                    "goal.conflict: imported content or pause posture differs from source".into(),
                ));
            }
            Ok(())
        })
        .await?)
}

pub(super) async fn read_source(
    workspace: &Path,
    max_bytes: usize,
) -> Result<(WorkspaceRef, String)> {
    read_source_at(workspace, SOURCE_PATH, max_bytes).await
}

pub(super) async fn read_source_at(
    workspace: &Path,
    relative_path: &str,
    max_bytes: usize,
) -> Result<(WorkspaceRef, String)> {
    let directory = std::fs::symlink_metadata(workspace.join(".a3s"))
        .context("goal.not_found: legacy source directory unavailable")?;
    let metadata = std::fs::symlink_metadata(workspace.join(relative_path))
        .context("goal.not_found: legacy source unavailable")?;
    if !directory.is_dir() || !metadata.is_file() || metadata.len() > max_bytes as u64 {
        bail!("goal.legacy_ambiguous: source must be a regular bounded file");
    }
    // The existing backend enforces its read boundary and growth cap as well.
    let services = WorkspaceServices::local(workspace);
    let path = services.normalize_path(relative_path)?;
    let raw = services.fs().read_text(&path).await?;
    if raw.len() > max_bytes {
        bail!("goal.legacy_ambiguous: source grew beyond the read bound");
    }
    Ok((services.workspace_ref().clone(), raw))
}

#[cfg(test)]
mod tests;
