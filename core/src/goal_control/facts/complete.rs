//! The goal closes once: every plan step accepted, the last one judged against
//! the goal's required criteria, no open wait, and every slice and original
//! receipt settled. Nothing in this closure calls a model.

use super::*;
use crate::planning::TaskStatus;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompletedBody {
    step_sequence: u64,
    settle_sequence: u64,
}

type CompletedFact = GoalFact<CompletedBody>;

fn operation_id(goal_id: &str) -> String {
    crate::content_digest::digest_bytes("a3s.code.goal-complete-operation.v1", goal_id.as_bytes())
}

fn digest(
    view: &DurableGoalDefinitionV1,
    body: &CompletedBody,
) -> Result<String, GoalControlError> {
    command_digest(&(
        GoalFactKindV1::Completed,
        &view.binding,
        view.plan_revision,
        body,
    ))
}

/// The closing step and its slice closeout, when the whole closure holds.
fn closure(
    facts: &[Fact],
    view: &DurableGoalDefinitionV1,
) -> Result<Option<CompletedBody>, GoalControlError> {
    let finished = view.state == GoalState::Active
        && view.plan_accepted
        && view.plan.as_ref().is_some_and(|plan| {
            !plan.steps.is_empty()
                && plan
                    .steps
                    .iter()
                    .all(|step| step.status == TaskStatus::Completed)
        })
        && view.waits.iter().all(|wait| wait.observed.is_some());
    if !finished
        || !settle::unsettled_slices(facts).is_empty()
        || crate::fact_control::has_unsettled_effects(facts)
    {
        return Ok(None);
    }
    let goal_id = view.binding.goal_id.as_str();
    let Some(step) = facts.iter().rfind(|fact| {
        fact.kind == GoalFactKindV1::StepAccepted.as_str()
            && fact.payload["goal_id"].as_str() == Some(goal_id)
    }) else {
        return Ok(None);
    };
    // Older acceptances never judged the goal's own criteria, so they cannot close it.
    let body = &step.payload["body"];
    if body["closes_goal"] != true
        || step.payload["goal_revision"] != view.binding.goal_revision
        || step.payload["plan_revision"] != serde_json::json!(view.plan_revision)
    {
        return Ok(None);
    }
    let Some(run_id) = body["run_id"].as_str() else {
        return Ok(None);
    };
    let key = goal_fact_key(
        goal_id,
        &settle::operation_id(run_id),
        GoalFactKindV1::SliceSettled,
    )?;
    Ok(facts
        .iter()
        .find(|fact| fact.key == key)
        .map(|settled| CompletedBody {
            step_sequence: step.seq,
            settle_sequence: settled.seq,
        }))
}

/// The goal can close now from recorded facts alone.
pub(crate) fn completion_ready(
    facts: &[Fact],
    view: &DurableGoalDefinitionV1,
) -> Result<bool, GoalControlError> {
    Ok(closure(facts, view)?.is_some())
}

/// The completion fact, or `None` while any part of the closure is open.
pub(crate) fn prepare_completion(
    facts: &[Fact],
    session_id: &str,
    workspace: &WorkspaceRef,
    policy_digest: Option<&str>,
) -> Result<Option<NewFact>, GoalControlError> {
    let Some(view) = definition_from_facts(facts, session_id, workspace, policy_digest)? else {
        return Ok(None);
    };
    let Some(body) = closure(facts, &view)? else {
        return Ok(None);
    };
    let goal_id = view.binding.goal_id.as_str();
    let operation_id = operation_id(goal_id);
    let stored = CompletedFact {
        schema: GOAL_FACT_SCHEMA_V1.into(),
        session_id: session_id.into(),
        goal_id: goal_id.into(),
        operation_id: operation_id.clone(),
        command_digest: digest(&view, &body)?,
        goal_revision: view.binding.goal_revision,
        plan_revision: view.plan_revision,
        work_policy_digest: view.binding.work_policy_digest.clone(),
        body,
    };
    Ok(Some(NewFact {
        kind: GoalFactKindV1::Completed.as_str().into(),
        key: goal_fact_key(goal_id, &operation_id, GoalFactKindV1::Completed)?,
        payload: serde_json::to_value(stored)?,
    }))
}

pub(super) fn fold_complete(
    fact: &Fact,
    current: Option<&DurableGoalDefinitionV1>,
    facts: &[Fact],
    session_id: &str,
    workspace: &WorkspaceRef,
    policy_digest: Option<&str>,
) -> Result<(String, String, DurableGoalDefinitionV1), GoalControlError> {
    let current = current.ok_or_else(|| refused(GoalRefusal::NotFound, "goal is absent"))?;
    let stored = CompletedFact::deserialize(&fact.payload)
        .map_err(|_| refused(GoalRefusal::Conflict, "invalid goal completion fact"))?;
    stored.validate_source(fact, session_id, policy_digest, Some(current))?;
    let prefix = &facts[..facts
        .iter()
        .position(|source| source.seq == fact.seq)
        .unwrap_or(facts.len())];
    let expected = closure(prefix, current)?;
    if expected
        .as_ref()
        .map(|body| (body.step_sequence, body.settle_sequence))
        != Some((stored.body.step_sequence, stored.body.settle_sequence))
        || stored.goal_id != current.binding.goal_id
        || stored.goal_revision != current.binding.goal_revision
        || stored.plan_revision != current.plan_revision
        || &current.binding.workspace != workspace
        || stored.operation_id != operation_id(&current.binding.goal_id)
        || stored.command_digest != digest(current, &stored.body)?
    {
        return Err(refused(
            GoalRefusal::Conflict,
            "goal completion differs from its closure",
        ));
    }
    let mut view = current.clone();
    view.state = GoalState::Completed;
    view.goal_sequence = fact.seq;
    Ok((stored.operation_id, stored.command_digest, view))
}
