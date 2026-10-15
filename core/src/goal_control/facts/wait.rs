//! Step-scoped waits in the original Goal stream. An observation makes its step
//! ready again; it never approves, accepts progress or starts a Run.

use super::*;
use crate::external_observation::EXTERNAL_OBSERVATION_SCHEMA;
use crate::goal_control::{
    validate_text, GoalWaitCommandV1, GoalWaitConditionV1, GoalWaitEvidenceV1,
    GoalWaitObservationV1, GoalWaitV1,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitBody {
    expected_goal_revision: u32,
    #[serde(deserialize_with = "Option::<u32>::deserialize")]
    expected_plan_revision: Option<u32>,
    expected_goal_sequence: u64,
    step_id: String,
    #[serde(deserialize_with = "Option::<GoalWaitConditionV1>::deserialize")]
    condition: Option<GoalWaitConditionV1>,
    #[serde(deserialize_with = "Option::<GoalWaitEvidenceV1>::deserialize")]
    evidence: Option<GoalWaitEvidenceV1>,
    #[serde(deserialize_with = "Option::<u64>::deserialize")]
    observed_unix_ms: Option<u64>,
}

type WaitFact = GoalFact<WaitBody>;

/// A recorded slice has no original closeout, or an effect is unsettled.
pub(crate) fn slice_unsettled(facts: &[Fact]) -> bool {
    !super::settle::unsettled_slices(facts).is_empty()
        || crate::fact_control::has_unsettled_effects(facts)
}

fn validate_request(command: &GoalWaitCommandV1) -> Result<(), GoalControlError> {
    validate_id("operation_id", &command.operation_id)?;
    validate_id("goal_id", &command.goal_id)?;
    validate_id("session_id", &command.session_id)?;
    validate_id("step_id", &command.step_id)?;
    if command.expected_goal_revision == 0
        || command.expected_goal_sequence == 0
        || command
            .expected_plan_revision
            .is_none_or(|revision| revision == 0)
    {
        return Err(refused(
            GoalRefusal::Conflict,
            "wait references must be positive and name an admitted plan",
        ));
    }
    match (command.kind, &command.condition, &command.evidence) {
        (GoalCommandKindV1::DeclareWait, Some(condition), None) => condition.validate(),
        (GoalCommandKindV1::ObserveWait, None, Some(_)) => Ok(()),
        _ => Err(refused(
            GoalRefusal::ConditionUnsupported,
            "wait command requires exactly its own condition or evidence",
        )),
    }
}

fn fact_kind(command: &GoalWaitCommandV1) -> GoalFactKindV1 {
    if command.kind == GoalCommandKindV1::DeclareWait {
        GoalFactKindV1::WaitDeclared
    } else {
        GoalFactKindV1::WaitObserved
    }
}

/// `Ok(None)` is a quiet observation (not due or unchanged) that must not be appended.
fn transition(
    current: &DurableGoalDefinitionV1,
    command: &GoalWaitCommandV1,
    sequence: u64,
    clock: Option<u64>,
    last_clock: u64,
) -> Result<Option<DurableGoalDefinitionV1>, GoalControlError> {
    validate_request(command)?;
    if current.state != GoalState::Active
        || !current.plan_accepted
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
            "goal is not active with an admitted plan, or wait references are stale",
        ));
    }
    let step_open = current.plan.as_ref().is_some_and(|plan| {
        plan.steps.iter().any(|step| {
            step.id == command.step_id && step.status != crate::planning::TaskStatus::Completed
        })
    });
    if !step_open {
        return Err(refused(
            GoalRefusal::NotFound,
            "wait step is not an open step of the admitted plan",
        ));
    }
    let mut view = current.clone();
    view.goal_sequence = sequence;
    if let Some(condition) = &command.condition {
        if current.open_wait(&command.step_id).is_some() {
            return Err(refused(
                GoalRefusal::Conflict,
                "step already has an open wait",
            ));
        }
        view.waits.push(GoalWaitV1 {
            step_id: command.step_id.clone(),
            declared_sequence: sequence,
            condition: condition.clone(),
            observed: None,
        });
        return Ok(Some(view));
    }
    let evidence = command.evidence.clone().ok_or_else(|| {
        refused(
            GoalRefusal::ConditionUnsupported,
            "observation requires evidence",
        )
    })?;
    let index = view
        .waits
        .iter()
        .position(|wait| wait.step_id == command.step_id && wait.observed.is_none())
        .ok_or_else(|| refused(GoalRefusal::Conflict, "step has no open wait"))?;
    let mut observed_unix_ms = None;
    match (&view.waits[index].condition, &evidence) {
        (
            GoalWaitConditionV1::QuestionAnswer { .. },
            GoalWaitEvidenceV1::QuestionAnswer { answer },
        ) => {
            validate_text("wait.answer", answer, 4096)?;
        }
        (
            GoalWaitConditionV1::ConfirmationAnswer { .. },
            GoalWaitEvidenceV1::ConfirmationAnswer { .. },
        ) => {}
        (
            GoalWaitConditionV1::SourceRevision {
                subject,
                after_digest,
            },
            GoalWaitEvidenceV1::SourceRevision { observation },
        ) => {
            validate_id("observation.digest", &observation.digest)?;
            if observation.schema != EXTERNAL_OBSERVATION_SCHEMA
                || &observation.subject != subject
                || observation.payload.len() > 8_192
            {
                return Err(refused(
                    GoalRefusal::Conflict,
                    "observation does not belong to the waited source",
                ));
            }
            if &observation.digest == after_digest {
                return Ok(None);
            }
        }
        (GoalWaitConditionV1::UtcDue { due_unix_ms }, GoalWaitEvidenceV1::UtcDue {}) => {
            let now = clock.filter(|now| *now > 0).ok_or_else(|| {
                refused(
                    GoalRefusal::Conflict,
                    "due observation requires the host clock",
                )
            })?;
            if now < last_clock {
                return Err(refused(GoalRefusal::Conflict, "host clock moved backwards"));
            }
            if now < due_unix_ms.get() {
                return Ok(None);
            }
            observed_unix_ms = Some(now);
        }
        _ => {
            return Err(refused(
                GoalRefusal::ConditionUnsupported,
                "evidence kind differs from the open wait",
            ))
        }
    }
    view.waits[index].observed = Some(GoalWaitObservationV1 {
        sequence,
        evidence,
        observed_unix_ms,
    });
    Ok(Some(view))
}

pub(super) fn fold_wait(
    fact: &Fact,
    facts: &[Fact],
    current: Option<&DurableGoalDefinitionV1>,
    session_id: &str,
    workspace: &WorkspaceRef,
    policy_digest: Option<&str>,
    last_clock: &mut u64,
) -> Result<(String, String, DurableGoalDefinitionV1), GoalControlError> {
    let current =
        current.ok_or_else(|| refused(GoalRefusal::NotFound, "goal definition is unavailable"))?;
    let stored = WaitFact::deserialize(&fact.payload)
        .map_err(|_| refused(GoalRefusal::Conflict, "invalid goal wait fact"))?;
    stored.validate_source(fact, session_id, policy_digest, Some(current))?;
    let declared = fact.kind == GoalFactKindV1::WaitDeclared.as_str();
    let command = GoalWaitCommandV1 {
        kind: if declared {
            GoalCommandKindV1::DeclareWait
        } else {
            GoalCommandKindV1::ObserveWait
        },
        operation_id: stored.operation_id.clone(),
        goal_id: stored.goal_id.clone(),
        session_id: stored.session_id.clone(),
        workspace: workspace.clone(),
        work_policy_digest: stored.work_policy_digest.clone(),
        expected_goal_revision: stored.body.expected_goal_revision,
        expected_plan_revision: stored.body.expected_plan_revision,
        expected_goal_sequence: stored.body.expected_goal_sequence,
        step_id: stored.body.step_id.clone(),
        condition: stored.body.condition.clone(),
        evidence: stored.body.evidence.clone(),
    };
    let prefix = &facts[..facts.partition_point(|source| source.seq < fact.seq)];
    let clocked = matches!(command.evidence, Some(GoalWaitEvidenceV1::UtcDue {}));
    if slice_unsettled(prefix) || stored.body.observed_unix_ms.is_some() != clocked {
        return Err(refused(
            GoalRefusal::Conflict,
            "wait fact order or clock source mismatch",
        ));
    }
    let view = transition(
        current,
        &command,
        fact.seq,
        stored.body.observed_unix_ms,
        *last_clock,
    )?
    .ok_or_else(|| {
        refused(
            GoalRefusal::Conflict,
            "a quiet observation cannot be a fact",
        )
    })?;
    if stored.goal_revision != view.binding.goal_revision
        || stored.plan_revision != view.plan_revision
        || stored.command_digest != command_digest(&command)?
    {
        return Err(refused(
            GoalRefusal::Conflict,
            "wait source identity mismatch",
        ));
    }
    if let Some(clock) = stored.body.observed_unix_ms {
        *last_clock = clock;
    }
    Ok((stored.operation_id, stored.command_digest, view))
}

/// Returns `(recorded, prepared)`. `now_unix_ms` is the host clock, used only by due waits.
pub(crate) fn prepare_wait(
    facts: &[Fact],
    session_id: &str,
    workspace: &WorkspaceRef,
    policy_digest: Option<&str>,
    command: &GoalWaitCommandV1,
    now_unix_ms: u64,
) -> Result<(bool, PreparedDefinition), GoalControlError> {
    if command.session_id != session_id || &command.workspace != workspace {
        return Err(refused(GoalRefusal::NotFound, "goal wait is unavailable"));
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
        return Ok((
            true,
            PreparedDefinition {
                pending: None,
                definition,
            },
        ));
    }
    let current = folded
        .current
        .ok_or_else(|| refused(GoalRefusal::NotFound, "goal definition is unavailable"))?;
    if slice_unsettled(facts) {
        return Err(refused(
            GoalRefusal::ReceiptUnsettled,
            "the original Run association is not settled",
        ));
    }
    let clock =
        matches!(command.evidence, Some(GoalWaitEvidenceV1::UtcDue {})).then_some(now_unix_ms);
    let Some(view) = transition(
        &current,
        command,
        0,
        clock,
        folded.continuation.last_observed_unix_ms,
    )?
    else {
        return Ok((
            false,
            PreparedDefinition {
                pending: None,
                definition: current,
            },
        ));
    };
    let kind = fact_kind(command);
    let stored = WaitFact {
        schema: GOAL_FACT_SCHEMA_V1.into(),
        session_id: session_id.into(),
        goal_id: command.goal_id.clone(),
        operation_id: command.operation_id.clone(),
        command_digest: digest,
        goal_revision: current.binding.goal_revision,
        plan_revision: current.plan_revision,
        work_policy_digest: command.work_policy_digest.clone(),
        body: WaitBody {
            expected_goal_revision: command.expected_goal_revision,
            expected_plan_revision: command.expected_plan_revision,
            expected_goal_sequence: command.expected_goal_sequence,
            step_id: command.step_id.clone(),
            condition: command.condition.clone(),
            evidence: command.evidence.clone(),
            observed_unix_ms: clock,
        },
    };
    Ok((
        true,
        PreparedDefinition {
            pending: Some(NewFact {
                kind: kind.as_str().into(),
                key: goal_fact_key(&command.goal_id, &command.operation_id, kind)?,
                payload: serde_json::to_value(stored)?,
            }),
            definition: view,
        },
    ))
}

#[cfg(test)]
mod tests;
