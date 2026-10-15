//! Explicit pause/resume/cancel in the original Goal stream. Running-Run
//! interruption and settlement stay with the original RunControl in the facade.

use super::*;
use crate::goal_control::GoalControlCommandV1;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlBody {
    expected_goal_revision: u32,
    #[serde(deserialize_with = "Option::<u32>::deserialize")]
    expected_plan_revision: Option<u32>,
    expected_goal_sequence: u64,
}

type ControlFact = GoalFact<ControlBody>;

fn validate_request(command: &GoalControlCommandV1) -> Result<(), GoalControlError> {
    validate_id("operation_id", &command.operation_id)?;
    validate_id("goal_id", &command.goal_id)?;
    validate_id("session_id", &command.session_id)?;
    if !matches!(
        command.kind,
        GoalCommandKindV1::Pause | GoalCommandKindV1::Resume | GoalCommandKindV1::Cancel
    ) {
        return Err(refused(
            GoalRefusal::ConditionUnsupported,
            "unsupported goal control command",
        ));
    }
    if command.expected_goal_revision == 0
        || command.expected_goal_sequence == 0
        || command.expected_plan_revision == Some(0)
    {
        return Err(refused(
            GoalRefusal::Conflict,
            "control references must be positive",
        ));
    }
    Ok(())
}

fn transition(
    current: &DurableGoalDefinitionV1,
    command: &GoalControlCommandV1,
    sequence: u64,
) -> Result<DurableGoalDefinitionV1, GoalControlError> {
    validate_request(command)?;
    if matches!(current.state, GoalState::Cancelled | GoalState::Completed)
        || command.goal_id != current.binding.goal_id
        || command.session_id != current.binding.session_id
        || command.workspace != current.binding.workspace
        || command.work_policy_digest != current.binding.work_policy_digest
        || command.expected_goal_revision != current.binding.goal_revision
        || command.expected_plan_revision != current.plan_revision
        || command.expected_goal_sequence != current.goal_sequence
    {
        return Err(refused(
            GoalRefusal::Conflict,
            "goal is terminal or control references are stale or invalid",
        ));
    }
    let state = match (command.kind, current.state) {
        (GoalCommandKindV1::Pause, GoalState::Active | GoalState::Waiting) => GoalState::Paused,
        (GoalCommandKindV1::Resume, GoalState::Paused) => GoalState::Active,
        (GoalCommandKindV1::Cancel, _) => GoalState::Cancelled,
        _ => {
            return Err(refused(
                GoalRefusal::Conflict,
                "goal state does not admit this control",
            ))
        }
    };
    let mut view = current.clone();
    view.state = state;
    view.goal_sequence = sequence;
    Ok(view)
}

fn fact_kind(kind: GoalCommandKindV1) -> GoalFactKindV1 {
    match kind {
        GoalCommandKindV1::Pause => GoalFactKindV1::Paused,
        GoalCommandKindV1::Resume => GoalFactKindV1::Resumed,
        _ => GoalFactKindV1::Cancelled,
    }
}

pub(super) fn fold_control(
    fact: &Fact,
    current: Option<&DurableGoalDefinitionV1>,
    session_id: &str,
    workspace: &WorkspaceRef,
    policy_digest: Option<&str>,
) -> Result<(String, String, DurableGoalDefinitionV1), GoalControlError> {
    let current =
        current.ok_or_else(|| refused(GoalRefusal::NotFound, "goal definition is unavailable"))?;
    let stored = ControlFact::deserialize(&fact.payload)
        .map_err(|_| refused(GoalRefusal::Conflict, "invalid goal control fact"))?;
    stored.validate_source(fact, session_id, policy_digest, Some(current))?;
    let kind = [
        GoalCommandKindV1::Pause,
        GoalCommandKindV1::Resume,
        GoalCommandKindV1::Cancel,
    ]
    .into_iter()
    .find(|kind| fact_kind(*kind).as_str() == fact.kind)
    .ok_or_else(|| refused(GoalRefusal::Conflict, "invalid goal control fact"))?;
    let command = GoalControlCommandV1 {
        kind,
        operation_id: stored.operation_id.clone(),
        goal_id: stored.goal_id.clone(),
        session_id: stored.session_id.clone(),
        workspace: workspace.clone(),
        work_policy_digest: stored.work_policy_digest.clone(),
        expected_goal_revision: stored.body.expected_goal_revision,
        expected_plan_revision: stored.body.expected_plan_revision,
        expected_goal_sequence: stored.body.expected_goal_sequence,
    };
    let view = transition(current, &command, fact.seq)?;
    if stored.goal_revision != view.binding.goal_revision
        || stored.plan_revision != view.plan_revision
        || stored.command_digest != command_digest(&command)?
    {
        return Err(refused(
            GoalRefusal::Conflict,
            "control source identity mismatch",
        ));
    }
    Ok((stored.operation_id, stored.command_digest, view))
}

pub(crate) fn prepare_control(
    facts: &[Fact],
    session_id: &str,
    workspace: &WorkspaceRef,
    policy_digest: Option<&str>,
    command: &GoalControlCommandV1,
) -> Result<PreparedDefinition, GoalControlError> {
    if command.session_id != session_id || &command.workspace != workspace {
        return Err(refused(
            GoalRefusal::NotFound,
            "goal control is unavailable",
        ));
    }
    if command.work_policy_digest.as_deref() != policy_digest {
        return Err(refused(
            GoalRefusal::Conflict,
            "policy binding differs from the admitted session",
        ));
    }
    validate_request(command)?;
    let digest = command_digest(command)?;
    let folded = fold_definitions(
        facts,
        session_id,
        workspace,
        policy_digest,
        Some(&command.operation_id),
    )?;
    if let Some((stored, definition)) = folded.replay {
        if stored != digest {
            return Err(refused(
                GoalRefusal::Conflict,
                "operation was already admitted with different content",
            ));
        }
        return Ok(PreparedDefinition {
            pending: None,
            definition,
        });
    }
    let current = folded
        .current
        .ok_or_else(|| refused(GoalRefusal::NotFound, "goal definition is unavailable"))?;
    let view = transition(&current, command, 0)?;
    let kind = fact_kind(command.kind);
    let stored = ControlFact {
        schema: GOAL_FACT_SCHEMA_V1.into(),
        session_id: session_id.into(),
        goal_id: command.goal_id.clone(),
        operation_id: command.operation_id.clone(),
        command_digest: digest,
        goal_revision: current.binding.goal_revision,
        plan_revision: current.plan_revision,
        work_policy_digest: command.work_policy_digest.clone(),
        body: ControlBody {
            expected_goal_revision: command.expected_goal_revision,
            expected_plan_revision: command.expected_plan_revision,
            expected_goal_sequence: command.expected_goal_sequence,
        },
    };
    Ok(PreparedDefinition {
        pending: Some(NewFact {
            kind: kind.as_str().into(),
            key: goal_fact_key(&command.goal_id, &command.operation_id, kind)?,
            payload: serde_json::to_value(stored)?,
        }),
        definition: view,
    })
}

#[cfg(test)]
mod tests;
