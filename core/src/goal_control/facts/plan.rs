//! Original plan definitions and their explicit acceptance in the same Goal fact stream.

use super::*;
use crate::goal_control::{deserialize_plan, validate_goal_plan, GoalPlanCommandV1};
use crate::planning::{ExecutionPlan, TaskStatus};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanBody {
    #[serde(deserialize_with = "deserialize_plan")]
    plan: Option<ExecutionPlan>,
    identity: ExecutionIdentityV1,
    #[serde(deserialize_with = "Option::<u64>::deserialize")]
    proposal_sequence: Option<u64>,
    expected_goal_revision: u32,
    #[serde(deserialize_with = "Option::<u32>::deserialize")]
    expected_plan_revision: Option<u32>,
    expected_goal_sequence: u64,
}

type PlanFact = GoalFact<PlanBody>;

fn validate_request(command: &GoalPlanCommandV1) -> Result<(), GoalControlError> {
    validate_id("operation_id", &command.operation_id)?;
    validate_id("goal_id", &command.goal_id)?;
    validate_id("session_id", &command.session_id)?;
    if command.expected_goal_revision == 0 || command.expected_goal_sequence == 0 {
        return Err(refused(
            GoalRefusal::Conflict,
            "plan references must be positive",
        ));
    }
    match (command.kind, &command.plan, command.proposal_sequence) {
        (GoalCommandKindV1::ProposePlan, Some(plan), None) => {
            validate_goal_plan(plan)?;
        }
        (GoalCommandKindV1::AcceptPlan, None, Some(sequence)) if sequence > 0 => {}
        (GoalCommandKindV1::ProposePlan | GoalCommandKindV1::AcceptPlan, _, _) => {
            return Err(refused(
                GoalRefusal::Conflict,
                "plan command content does not match its kind",
            ));
        }
        _ => {
            return Err(refused(
                GoalRefusal::ConditionUnsupported,
                "not a plan command",
            ))
        }
    }
    Ok(())
}

fn transition(
    current: &DurableGoalDefinitionV1,
    command: &GoalPlanCommandV1,
    sequence: u64,
) -> Result<(DurableGoalDefinitionV1, ExecutionIdentityV1), GoalControlError> {
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
            "plan references are stale or invalid",
        ));
    }
    let mut view = current.clone();
    let identity = if let Some(plan) = &command.plan {
        let identity = validate_goal_plan(plan)?;
        view.plan_revision = Some(
            current
                .plan_revision
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| refused(GoalRefusal::Conflict, "plan revision overflow"))?,
        );
        let mut projected = plan.clone();
        // Original mark_status preserves terminal states; this copy has no accepted evidence yet.
        for step in &mut projected.steps {
            step.status = TaskStatus::Pending;
        }
        view.plan = Some(projected);
        view.plan_fact_sequence = Some(sequence);
        view.plan_accepted = false;
        view.waits.clear();
        identity
    } else {
        if current.plan_accepted || command.proposal_sequence != current.plan_fact_sequence {
            return Err(refused(
                GoalRefusal::Conflict,
                "plan proposal is not current",
            ));
        }
        let plan = current
            .plan
            .as_ref()
            .ok_or_else(|| refused(GoalRefusal::NotFound, "plan proposal is unavailable"))?;
        let identity = validate_goal_plan(plan)?;
        view.plan_accepted = true;
        identity
    };
    view.goal_sequence = sequence;
    Ok((view, identity))
}

pub(super) fn fold_plan(
    fact: &Fact,
    current: Option<&DurableGoalDefinitionV1>,
    session_id: &str,
    workspace: &WorkspaceRef,
    policy_digest: Option<&str>,
) -> Result<(String, String, DurableGoalDefinitionV1), GoalControlError> {
    let current =
        current.ok_or_else(|| refused(GoalRefusal::NotFound, "goal definition is unavailable"))?;
    let stored = PlanFact::deserialize(&fact.payload)
        .map_err(|_| refused(GoalRefusal::Conflict, "invalid goal plan fact"))?;
    stored.validate_source(fact, session_id, policy_digest, Some(current))?;
    let command = GoalPlanCommandV1 {
        kind: if fact.kind == GoalFactKindV1::PlanProposed.as_str() {
            GoalCommandKindV1::ProposePlan
        } else {
            GoalCommandKindV1::AcceptPlan
        },
        operation_id: stored.operation_id.clone(),
        goal_id: stored.goal_id.clone(),
        session_id: stored.session_id.clone(),
        workspace: workspace.clone(),
        work_policy_digest: stored.work_policy_digest.clone(),
        expected_goal_revision: stored.body.expected_goal_revision,
        expected_plan_revision: stored.body.expected_plan_revision,
        expected_goal_sequence: stored.body.expected_goal_sequence,
        plan: stored.body.plan.clone(),
        proposal_sequence: stored.body.proposal_sequence,
    };
    let (view, identity) = transition(current, &command, fact.seq)?;
    if stored.goal_revision != current.binding.goal_revision
        || stored.plan_revision != view.plan_revision
        || stored.body.identity != identity
        || stored.command_digest != command_digest(&command)?
    {
        return Err(refused(
            GoalRefusal::Conflict,
            "plan source identity mismatch",
        ));
    }
    Ok((stored.operation_id, stored.command_digest, view))
}

pub(crate) fn prepare_plan(
    facts: &[Fact],
    session_id: &str,
    workspace: &WorkspaceRef,
    policy_digest: Option<&str>,
    command: &GoalPlanCommandV1,
) -> Result<PreparedDefinition, GoalControlError> {
    if command.session_id != session_id || &command.workspace != workspace {
        return Err(refused(GoalRefusal::NotFound, "goal plan is unavailable"));
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
    let (view, identity) = transition(&current, command, 0)?;
    let kind = if command.kind == GoalCommandKindV1::ProposePlan {
        GoalFactKindV1::PlanProposed
    } else {
        GoalFactKindV1::PlanAccepted
    };
    let stored = PlanFact {
        schema: GOAL_FACT_SCHEMA_V1.into(),
        session_id: session_id.into(),
        goal_id: command.goal_id.clone(),
        operation_id: command.operation_id.clone(),
        command_digest: digest,
        goal_revision: current.binding.goal_revision,
        plan_revision: view.plan_revision,
        work_policy_digest: command.work_policy_digest.clone(),
        body: PlanBody {
            plan: command.plan.clone(),
            identity,
            proposal_sequence: command.proposal_sequence,
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
