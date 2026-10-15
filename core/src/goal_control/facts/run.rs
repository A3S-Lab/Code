//! Association of one actual original Run with an admitted Goal/plan step.

use super::*;

mod continuation;
pub(crate) use continuation::ContinuationSource;
pub(super) use continuation::ContinuationUsage;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RunBody {
    pub(super) run_id: String,
    pub(super) step_id: String,
    expected_goal_sequence: u64,
    plan_fact_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    continuation: Option<ContinuationSource>,
}

pub(super) type RunFact = GoalFact<RunBody>;

/// The caller has folded this exact log and checked its workspace binding.
/// A historical association requests reconciliation; it grants no execution.
pub(crate) fn matches_tick_run(
    facts: &[Fact],
    command: &super::super::GoalControlCommandV1,
) -> Result<bool, GoalControlError> {
    let key = goal_fact_key(
        &command.goal_id,
        &command.operation_id,
        GoalFactKindV1::SliceAdmitted,
    )?;
    let Some(fact) = facts.iter().find(|fact| fact.key == key) else {
        return Ok(false);
    };
    let stored = RunFact::deserialize(&fact.payload)
        .map_err(|_| refused(GoalRefusal::Conflict, "invalid original tick Run source"))?;
    if fact.kind != GoalFactKindV1::SliceAdmitted.as_str()
        || command.kind != GoalCommandKindV1::Tick
        || stored.body.run_id != command.operation_id
        || stored.goal_id != command.goal_id
        || stored.session_id != command.session_id
        || stored.work_policy_digest != command.work_policy_digest
        || stored.goal_revision != command.expected_goal_revision
        || stored.plan_revision != command.expected_plan_revision
        || stored.body.expected_goal_sequence != command.expected_goal_sequence
    {
        return Err(refused(
            GoalRefusal::Conflict,
            "tick operation differs from its original Run source",
        ));
    }
    Ok(true)
}

pub(super) fn current_step(
    view: &DurableGoalDefinitionV1,
) -> Result<&crate::planning::Task, GoalControlError> {
    if !view.plan_accepted {
        return Err(refused(
            GoalRefusal::HostDriverConflict,
            "goal execution requires an admitted plan",
        ));
    }
    match view.state {
        GoalState::Active => {}
        GoalState::Paused => return Err(refused(GoalRefusal::Paused, "goal is paused")),
        GoalState::Waiting => return Err(refused(GoalRefusal::Waiting, "goal is waiting")),
        GoalState::Cancelled | GoalState::Completed => {
            return Err(refused(GoalRefusal::Conflict, "goal is terminal"))
        }
    }
    view.ready_steps()
        .first()
        .copied()
        .ok_or_else(|| refused(GoalRefusal::Waiting, "goal has no ready admitted step"))
}

fn digest(view: &DurableGoalDefinitionV1, body: &RunBody) -> Result<String, GoalControlError> {
    command_digest(&(
        GoalFactKindV1::SliceAdmitted,
        &view.binding,
        view.plan_revision,
        body,
    ))
}

pub(super) fn fold_run(
    fact: &Fact,
    current: Option<&DurableGoalDefinitionV1>,
    session_id: &str,
    workspace: &WorkspaceRef,
    policy_digest: Option<&str>,
    continuation: &mut ContinuationUsage,
) -> Result<(String, String, DurableGoalDefinitionV1), GoalControlError> {
    let current =
        current.ok_or_else(|| refused(GoalRefusal::NotFound, "goal definition is unavailable"))?;
    let stored = RunFact::deserialize(&fact.payload)
        .map_err(|_| refused(GoalRefusal::Conflict, "invalid original goal Run binding"))?;
    stored.validate_source(fact, session_id, policy_digest, Some(current))?;
    validate_id("run_id", &stored.body.run_id)?;
    let step = current_step(current)?;
    if &current.binding.workspace != workspace
        || stored.operation_id != stored.body.run_id
        || stored.goal_id != current.binding.goal_id
        || stored.goal_revision != current.binding.goal_revision
        || stored.plan_revision != current.plan_revision
        || stored.body.expected_goal_sequence != current.goal_sequence
        || Some(stored.body.plan_fact_sequence) != current.plan_fact_sequence
        || stored.body.step_id != step.id
        || stored.command_digest != digest(current, &stored.body)?
    {
        return Err(refused(
            GoalRefusal::Conflict,
            "original goal Run binding differs from its source",
        ));
    }
    if let Some(source) = &stored.body.continuation {
        continuation.admit(source)?;
    }
    let mut view = current.clone();
    view.goal_sequence = fact.seq;
    Ok((stored.operation_id, stored.command_digest, view))
}

pub(crate) fn prepare_run(
    facts: &[Fact],
    session_id: &str,
    workspace: &WorkspaceRef,
    policy_digest: Option<&str>,
    run_id: Option<&str>,
    continuation: Option<&ContinuationSource>,
) -> Result<Option<PreparedDefinition>, GoalControlError> {
    let mut folded = fold_definitions(facts, session_id, workspace, policy_digest, run_id)?;
    let Some(current) = folded.current else {
        if continuation.is_some() {
            return Err(refused(
                GoalRefusal::NotFound,
                "automatic Run requires its original goal",
            ));
        }
        return Ok(None);
    };
    let run_id = run_id.ok_or_else(|| {
        refused(
            GoalRefusal::HostDriverConflict,
            "goal execution requires the original Run identity",
        )
    })?;
    validate_id("run_id", run_id)?;
    let step = current_step(&current)?;
    if let Some((_, prior)) = folded.replay {
        let key = goal_fact_key(
            &current.binding.goal_id,
            run_id,
            GoalFactKindV1::SliceAdmitted,
        )?;
        let original_kind = facts.iter().find(|fact| fact.key == key);
        if original_kind.is_none() || prior.goal_sequence != current.goal_sequence {
            return Err(refused(
                GoalRefusal::Conflict,
                "original goal Run binding is stale or conflicts",
            ));
        }
        let original = original_kind
            .and_then(|fact| RunFact::deserialize(&fact.payload).ok())
            .ok_or_else(|| refused(GoalRefusal::Conflict, "original Run source is invalid"))?;
        if original.body.continuation.as_ref() != continuation {
            return Err(refused(
                GoalRefusal::Conflict,
                "original Run continuation source differs",
            ));
        }
        return Ok(Some(PreparedDefinition {
            pending: None,
            definition: current,
        }));
    }
    if let Some(source) = continuation {
        source.policy.check_admission(source.observed_unix_ms)?;
        folded.continuation.admit(source)?;
    }
    let body = RunBody {
        run_id: run_id.into(),
        step_id: step.id.clone(),
        expected_goal_sequence: current.goal_sequence,
        plan_fact_sequence: current
            .plan_fact_sequence
            .ok_or_else(|| refused(GoalRefusal::Conflict, "admitted plan source is absent"))?,
        continuation: continuation.cloned(),
    };
    let stored = RunFact {
        schema: GOAL_FACT_SCHEMA_V1.into(),
        session_id: session_id.into(),
        goal_id: current.binding.goal_id.clone(),
        operation_id: run_id.into(),
        command_digest: digest(&current, &body)?,
        goal_revision: current.binding.goal_revision,
        plan_revision: current.plan_revision,
        work_policy_digest: current.binding.work_policy_digest.clone(),
        body,
    };
    Ok(Some(PreparedDefinition {
        pending: Some(NewFact {
            kind: GoalFactKindV1::SliceAdmitted.as_str().into(),
            key: goal_fact_key(
                &current.binding.goal_id,
                run_id,
                GoalFactKindV1::SliceAdmitted,
            )?,
            payload: serde_json::to_value(stored)?,
        }),
        definition: current,
    }))
}

pub(crate) fn render_run_context(
    view: &DurableGoalDefinitionV1,
    run_id: &str,
) -> Result<String, GoalControlError> {
    const SHOWN: usize = 4;
    let step = current_step(view)?;
    let mut binding = serde_json::to_value(&view.binding)?;
    let non_goals = binding
        .as_object_mut()
        .and_then(|fields| fields.remove("non_goals"));
    let mut context = serde_json::json!({
        "run_id": run_id, "binding": binding,
        "goal_sequence": view.goal_sequence, "plan_revision": view.plan_revision,
        "plan_fact_sequence": view.plan_fact_sequence, "current_step": step,
    });
    let answered = view
        .waits
        .iter()
        .filter(|wait| wait.step_id == step.id && wait.observed.is_some())
        .collect::<Vec<_>>();
    let waiting = view
        .waits
        .iter()
        .filter(|wait| wait.observed.is_none())
        .map(|wait| serde_json::json!({"step_id": wait.step_id, "kind": wait.condition.kind().as_str()}))
        .collect::<Vec<_>>();
    let mut note = "";
    if !answered.is_empty() || !waiting.is_empty() {
        // Bounded hints only; the full list stays in the original goal definition view.
        context["current_step_waits"] =
            serde_json::to_value(&answered[answered.len().saturating_sub(SHOWN)..])?;
        context["other_steps_waiting"] =
            serde_json::to_value(&waiting[..waiting.len().min(SHOWN)])?;
        context["waits_truncated"] = serde_json::json!(
            answered.len().saturating_sub(SHOWN) + waiting.len().saturating_sub(SHOWN)
        );
        note = " Wait answers and observations are inputs, not approval.";
    }
    let steps = view.plan.as_ref().map_or(&[][..], |plan| &plan.steps[..]);
    let accepted = steps
        .iter()
        .filter(|step| step.status == crate::planning::TaskStatus::Completed)
        .map(|step| step.id.as_str())
        .collect::<Vec<_>>();
    context["accepted_steps"] =
        serde_json::to_value(&accepted[accepted.len().saturating_sub(SHOWN)..])?;
    context["remaining_steps"] = serde_json::json!(steps.len() - accepted.len());
    // Refs, current step and waits come first so truncation only cuts the goal text.
    Ok(bounded_brief(format!(
        "## Current admitted goal\nThis binding describes the current task; it does not prove accepted progress or completion. Advance only with fresh evidence that meets current_step.success_criteria.{note}\n{}\ngoal: {}",
        serde_json::to_string(&context)?,
        serde_json::to_string(&serde_json::json!({"goal": view.goal, "non_goals": non_goals}))?
    )))
}

const MAX_BRIEF_BYTES: usize = 4 * 1024;
const MAX_BRIEF_TOKENS: usize = 512;
const BRIEF_TRUNCATED: &str = "\n[brief truncated; the full view is the goal definition]";

fn bounded_brief(mut brief: String) -> String {
    let fits = |text: &str| {
        text.len() <= MAX_BRIEF_BYTES
            && crate::llm::estimate_prompt_tokens(&[], Some(text), &[]) <= MAX_BRIEF_TOKENS
    };
    if fits(&brief) {
        return brief;
    }
    let mut cut = brief.len().min(MAX_BRIEF_BYTES);
    let mut candidate = String::with_capacity(MAX_BRIEF_BYTES);
    while cut > 0 {
        if brief.is_char_boundary(cut) {
            candidate.clear();
            candidate.push_str(&brief[..cut]);
            candidate.push_str(BRIEF_TRUNCATED);
            if fits(&candidate) {
                break;
            }
        }
        cut -= 1;
    }
    brief.truncate(cut);
    brief.push_str(BRIEF_TRUNCATED);
    brief
}

#[cfg(test)]
mod tests;
