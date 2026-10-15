//! Accepted progress references the original Run and its immutable evidence prefix.

use super::*;
use crate::planning::TaskStatus;
use crate::verification::VerificationReport;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StepBody {
    run_id: String,
    step_id: String,
    run_sequence: u64,
    plan_identity: ExecutionIdentityV1,
    source_sequence: u64,
    source_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attestation: Option<VerificationReport>,
    /// The judgment also covered the goal's required criteria.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    closes_goal: bool,
}

type StepFact = GoalFact<StepBody>;

/// The current step is the plan's last open step, so its acceptance must also
/// judge the goal's own required criteria.
pub(crate) fn closes_goal(view: &DurableGoalDefinitionV1) -> bool {
    let Ok(current) = run::current_step(view) else {
        return false;
    };
    view.plan.as_ref().is_some_and(|plan| {
        plan.steps
            .iter()
            .all(|step| step.id == current.id || step.status == TaskStatus::Completed)
    })
}

pub(super) fn operation_id(run_id: &str) -> String {
    crate::content_digest::digest_bytes("a3s.code.goal-step-operation.v1", run_id.as_bytes())
}

fn digest(view: &DurableGoalDefinitionV1, body: &StepBody) -> Result<String, GoalControlError> {
    command_digest(&(
        GoalFactKindV1::StepAccepted,
        &view.binding,
        view.plan_revision,
        body,
    ))
}

fn bound_run<'a>(
    facts: &'a [Fact],
    view: &DurableGoalDefinitionV1,
    run_id: &str,
) -> Result<&'a Fact, GoalControlError> {
    let fact = facts
        .iter()
        .find(|fact| fact.seq == view.goal_sequence)
        .ok_or_else(|| refused(GoalRefusal::EvidenceStale, "current Run binding is absent"))?;
    let stored = run::RunFact::deserialize(&fact.payload)
        .map_err(|_| refused(GoalRefusal::EvidenceStale, "current Run binding is invalid"))?;
    if fact.kind != GoalFactKindV1::SliceAdmitted.as_str()
        || stored.body.run_id != run_id
        || stored.body.step_id != run::current_step(view)?.id
    {
        return Err(refused(
            GoalRefusal::EvidenceStale,
            "step evidence belongs to another Run",
        ));
    }
    Ok(fact)
}

/// The current slice's Run and step, as a reference check for host review binding.
pub(crate) fn check_step_run(
    facts: &[Fact],
    view: &DurableGoalDefinitionV1,
    step_id: &str,
    run_id: &str,
) -> Result<(), GoalControlError> {
    bound_run(facts, view, run_id)?;
    if run::current_step(view)?.id != step_id {
        return Err(refused(
            GoalRefusal::EvidenceStale,
            "review binds another step",
        ));
    }
    Ok(())
}

pub(crate) fn prepare_step(
    facts: &[Fact],
    session_id: &str,
    workspace: &WorkspaceRef,
    policy_digest: Option<&str>,
    run_id: &str,
    attestation: Option<&VerificationReport>,
) -> Result<Option<(PreparedDefinition, Vec<VerificationReport>, String)>, GoalControlError> {
    let Some(view) = definition_from_facts(facts, session_id, workspace, policy_digest)? else {
        return Ok(None);
    };
    let run = bound_run(facts, &view, run_id)?;
    let Some((reports, evidence)) = crate::fact_control::evidence_for_goal_step(
        facts,
        run.seq,
        session_id,
        run_id,
        attestation,
    ) else {
        return Ok(None);
    };
    let step = run::current_step(&view)?;
    let cut = a3s_effect::cut_log(facts);
    let body = StepBody {
        run_id: run_id.into(),
        step_id: step.id.clone(),
        run_sequence: run.seq,
        plan_identity: view
            .plan
            .as_ref()
            .ok_or_else(|| refused(GoalRefusal::EvidenceStale, "current plan is absent"))?
            .definition_identity()?,
        source_sequence: cut.seq,
        source_digest: cut.digest,
        attestation: attestation.cloned(),
        closes_goal: closes_goal(&view),
    };
    let operation_id = operation_id(run_id);
    let stored = StepFact {
        schema: GOAL_FACT_SCHEMA_V1.into(),
        session_id: session_id.into(),
        goal_id: view.binding.goal_id.clone(),
        operation_id: operation_id.clone(),
        command_digest: digest(&view, &body)?,
        goal_revision: view.binding.goal_revision,
        plan_revision: view.plan_revision,
        work_policy_digest: view.binding.work_policy_digest.clone(),
        body,
    };
    Ok(Some((
        PreparedDefinition {
            pending: Some(NewFact {
                kind: GoalFactKindV1::StepAccepted.as_str().into(),
                key: goal_fact_key(
                    &view.binding.goal_id,
                    &operation_id,
                    GoalFactKindV1::StepAccepted,
                )?,
                payload: serde_json::to_value(stored)?,
            }),
            definition: view,
        },
        reports,
        evidence,
    )))
}

pub(super) fn fold_step(
    fact: &Fact,
    current: Option<&DurableGoalDefinitionV1>,
    facts: &[Fact],
    session_id: &str,
    workspace: &WorkspaceRef,
    policy_digest: Option<&str>,
) -> Result<
    (
        String,
        String,
        DurableGoalDefinitionV1,
        Vec<VerificationReport>,
    ),
    GoalControlError,
> {
    let current = current.ok_or_else(|| refused(GoalRefusal::NotFound, "goal is absent"))?;
    let stored = StepFact::deserialize(&fact.payload)
        .map_err(|_| refused(GoalRefusal::EvidenceStale, "invalid accepted step fact"))?;
    stored.validate_source(fact, session_id, policy_digest, Some(current))?;
    let end = facts
        .iter()
        .position(|source| source.seq == stored.body.source_sequence)
        .ok_or_else(|| refused(GoalRefusal::EvidenceStale, "step evidence prefix is absent"))?;
    let prefix = &facts[..=end];
    let cut = a3s_effect::cut_log(prefix);
    let terminal = facts.get(end + 1).ok_or_else(|| {
        refused(
            GoalRefusal::ReceiptUnsettled,
            "original completion boundary is absent",
        )
    })?;
    let previous = facts.iter().rfind(|source| source.seq < fact.seq);
    if stored.body.source_digest != cut.digest
        || previous != Some(terminal)
        || terminal.kind != crate::fact_control::TURN_COMPLETE_FACT_KIND
        || terminal.payload["log_cut"]["seq"] != cut.seq
        || terminal.payload["log_cut"]["digest"] != cut.digest
        || stored.body.run_sequence != current.goal_sequence
        || stored.body.step_id != run::current_step(current)?.id
        || (stored.body.closes_goal && !closes_goal(current))
        || stored.body.plan_identity
            != current
                .plan
                .as_ref()
                .ok_or_else(|| refused(GoalRefusal::EvidenceStale, "current plan is absent"))?
                .definition_identity()?
        || stored.goal_id != current.binding.goal_id
        || stored.goal_revision != current.binding.goal_revision
        || stored.plan_revision != current.plan_revision
        || &current.binding.workspace != workspace
        || stored.operation_id != operation_id(&stored.body.run_id)
        || stored.command_digest != digest(current, &stored.body)?
    {
        return Err(refused(
            GoalRefusal::EvidenceStale,
            "accepted step differs from its current evidence",
        ));
    }
    bound_run(prefix, current, &stored.body.run_id)?;
    let (reports, _) = crate::fact_control::evidence_for_goal_step(
        prefix,
        stored.body.run_sequence,
        session_id,
        &stored.body.run_id,
        stored.body.attestation.as_ref(),
    )
    .ok_or_else(|| {
        refused(
            GoalRefusal::EvidenceStale,
            "accepted step lacks original reports and receipts",
        )
    })?;
    let mut view = current.clone();
    let plan = view
        .plan
        .as_mut()
        .ok_or_else(|| refused(GoalRefusal::EvidenceStale, "current plan is absent"))?;
    plan.mark_status(&stored.body.step_id, TaskStatus::Completed);
    view.goal_sequence = fact.seq;
    Ok((stored.operation_id, stored.command_digest, view, reports))
}

pub(crate) fn validate_step_append(
    facts: &[Fact],
    pending: &NewFact,
    session_id: &str,
    workspace: &WorkspaceRef,
    policy_digest: Option<&str>,
) -> Result<Vec<VerificationReport>, GoalControlError> {
    let current = definition_from_facts(facts, session_id, workspace, policy_digest)?;
    let seq = facts
        .last()
        .map_or(0, |fact| fact.seq)
        .checked_add(1)
        .ok_or_else(|| refused(GoalRefusal::Conflict, "original fact sequence overflow"))?;
    let fact = Fact {
        seq,
        kind: pending.kind.clone(),
        key: pending.key.clone(),
        cause: None,
        payload: pending.payload.clone(),
    };
    let (_, _, _, reports) = fold_step(
        &fact,
        current.as_ref(),
        facts,
        session_id,
        workspace,
        policy_digest,
    )?;
    Ok(reports)
}
