//! Stable addresses in the original fact log; no writer or alternate journal.

use super::{
    deserialize_goal_definition, validate_goal_definition, validate_id, DurableGoalBindingV1,
    DurableGoalDefinitionV1, GoalCommandKindV1, GoalControlError, GoalDefinitionCommandV1,
    GoalRefusal, GoalState,
};
use crate::execution_identity::ExecutionIdentityV1;
use crate::planning::AgentGoal;
use crate::workspace::WorkspaceRef;
use a3s_effect::{Fact, NewFact};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const GOAL_FACT_SCHEMA_V1: &str = "a3s.code.goal.fact.v1";

closed_set!(GoalFactKindV1 {
    Created => "a3s.goal.created", Revised => "a3s.goal.revised",
    PlanProposed => "a3s.goal.plan.proposed", PlanAccepted => "a3s.goal.plan.accepted",
    SliceAdmitted => "a3s.goal.slice.admitted", SliceSettled => "a3s.goal.slice.settled",
    StepAccepted => "a3s.goal.step.accepted", WaitDeclared => "a3s.goal.wait.declared",
    WaitObserved => "a3s.goal.wait.observed", Paused => "a3s.goal.paused",
    Resumed => "a3s.goal.resumed", Cancelled => "a3s.goal.cancelled",
    Completed => "a3s.goal.completed",
});

pub fn goal_fact_key(
    goal_id: &str,
    operation_id: &str,
    kind: GoalFactKindV1,
) -> Result<String, GoalControlError> {
    validate_id("goal_id", goal_id)?;
    validate_id("operation_id", operation_id)?;
    let goal = crate::content_digest::digest_bytes("a3s.code.goal-id.v1", goal_id.as_bytes());
    let operation = crate::content_digest::digest_bytes(
        "a3s.code.goal-operation-id.v1",
        operation_id.as_bytes(),
    );
    Ok(format!("goal/{goal}/{operation}/{}", kind.as_str()))
}

mod run;
pub(super) use run::matches_tick_run;
pub(crate) use run::{prepare_run, render_run_context, ContinuationSource};
mod settle;
pub(crate) use settle::{prepare_settlement, reconcile_slices};
mod complete;
pub(crate) use complete::{completion_ready, prepare_completion};
mod step;
pub(crate) use step::{check_step_run, closes_goal, prepare_step, validate_step_append};

mod plan;
pub(crate) use plan::prepare_plan;
mod control;
pub(crate) use control::prepare_control;
mod wait;
pub(crate) use wait::prepare_wait;
pub(crate) use wait::slice_unsettled;

const COMMAND_DOMAIN: &str = "a3s.code.goal-command.v1";
const AUTHORIZATION_DOMAIN: &str = "a3s.code.goal-authorization.v1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DefinitionBody {
    binding: DurableGoalBindingV1,
    #[serde(deserialize_with = "deserialize_goal_definition")]
    goal: AgentGoal,
    #[serde(deserialize_with = "Option::<u32>::deserialize")]
    expected_goal_revision: Option<u32>,
    #[serde(deserialize_with = "Option::<u32>::deserialize")]
    expected_plan_revision: Option<u32>,
    #[serde(deserialize_with = "Option::<u64>::deserialize")]
    expected_goal_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    import_source: Option<super::GoalImportSourceV1>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GoalFact<T> {
    schema: String,
    session_id: String,
    goal_id: String,
    operation_id: String,
    command_digest: String,
    goal_revision: u32,
    #[serde(deserialize_with = "Option::<u32>::deserialize")]
    plan_revision: Option<u32>,
    #[serde(deserialize_with = "Option::<String>::deserialize")]
    work_policy_digest: Option<String>,
    body: T,
}

type DefinitionFact = GoalFact<DefinitionBody>;

impl<T> GoalFact<T> {
    fn validate_source(
        &self,
        fact: &Fact,
        session_id: &str,
        policy_digest: Option<&str>,
        current: Option<&DurableGoalDefinitionV1>,
    ) -> Result<(), GoalControlError> {
        if self.schema != GOAL_FACT_SCHEMA_V1
            || self.session_id != session_id
            || self.work_policy_digest.as_deref() != policy_digest
            || fact.key
                != goal_fact_key(
                    &self.goal_id,
                    &self.operation_id,
                    GoalFactKindV1::parse(&fact.kind)?,
                )?
            || fact.cause.is_some()
            || fact.seq == 0
            || current.is_some_and(|view| fact.seq <= view.goal_sequence)
        {
            return Err(refused(
                GoalRefusal::Conflict,
                "goal source identity or order mismatch",
            ));
        }
        Ok(())
    }
}

impl DefinitionFact {
    fn command(&self, kind: GoalCommandKindV1) -> GoalDefinitionCommandV1 {
        GoalDefinitionCommandV1 {
            kind,
            operation_id: self.operation_id.clone(),
            goal_id: self.goal_id.clone(),
            session_id: self.session_id.clone(),
            workspace: self.body.binding.workspace.clone(),
            goal: self.body.goal.clone(),
            non_goals: self.body.binding.non_goals.clone(),
            scope_fact_sequences: self.body.binding.scope_fact_sequences.clone(),
            work_policy_digest: self.work_policy_digest.clone(),
            expected_goal_revision: self.body.expected_goal_revision,
            expected_plan_revision: self.body.expected_plan_revision,
            expected_goal_sequence: self.body.expected_goal_sequence,
            import_source: self.body.import_source.clone(),
        }
    }

    fn view(
        &self,
        sequence: u64,
        current: Option<&DurableGoalDefinitionV1>,
    ) -> DurableGoalDefinitionV1 {
        DurableGoalDefinitionV1 {
            binding: self.body.binding.clone(),
            state: current.map(|view| view.state).unwrap_or_else(|| {
                if self
                    .body
                    .import_source
                    .as_ref()
                    .is_some_and(|source| source.paused)
                {
                    GoalState::Paused
                } else {
                    GoalState::Active
                }
            }),
            import_source: current
                .and_then(|view| view.import_source.clone())
                .or_else(|| self.body.import_source.clone()),
            goal: self.body.goal.clone(),
            goal_sequence: sequence,
            plan_revision: self.plan_revision,
            plan: None,
            plan_fact_sequence: None,
            plan_accepted: false,
            waits: Vec::new(),
        }
    }
}

pub(crate) struct PreparedDefinition {
    pub pending: Option<NewFact>,
    pub definition: DurableGoalDefinitionV1,
}

fn refused(refusal: GoalRefusal, reason: &'static str) -> GoalControlError {
    GoalControlError::Refused { refusal, reason }
}

fn command_digest(command: &impl Serialize) -> Result<String, GoalControlError> {
    Ok(crate::content_digest::digest_json(COMMAND_DOMAIN, command)?)
}

fn binding_for(
    command: &GoalDefinitionCommandV1,
    revision: u32,
) -> Result<DurableGoalBindingV1, GoalControlError> {
    validate_id("operation_id", &command.operation_id)?;
    validate_goal_definition(&command.goal)?;
    if let Some(source) = &command.import_source {
        source.identity.validate()?;
        if command.kind != GoalCommandKindV1::Create
            || source.identity.domain != super::GOAL_LEGACY_SOURCE_DOMAIN_V1
        {
            return Err(refused(
                GoalRefusal::Conflict,
                "invalid import origin or command",
            ));
        }
    }
    let binding = DurableGoalBindingV1 {
        goal_id: command.goal_id.clone(),
        session_id: command.session_id.clone(),
        workspace: command.workspace.clone(),
        goal_revision: revision,
        non_goals: command.non_goals.clone(),
        scope_fact_sequences: command.scope_fact_sequences.clone(),
        authorization_source: ExecutionIdentityV1::derive(
            AUTHORIZATION_DOMAIN,
            &(&command.session_id, &command.workspace, command),
        )?,
        work_policy_digest: command.work_policy_digest.clone(),
    };
    binding.validate()?;
    Ok(binding)
}

fn next_revision(
    command: &GoalDefinitionCommandV1,
    current: Option<&DurableGoalDefinitionV1>,
) -> Result<u32, GoalControlError> {
    let conflict = || {
        refused(
            GoalRefusal::Conflict,
            "definition references are stale or invalid",
        )
    };
    match (command.kind, current) {
        (GoalCommandKindV1::Create, None)
            if command.expected_goal_revision.is_none()
                && command.expected_plan_revision.is_none()
                && command.expected_goal_sequence.is_none() =>
        {
            Ok(1)
        }
        (GoalCommandKindV1::Revise, Some(current))
            if !matches!(current.state, GoalState::Cancelled | GoalState::Completed)
                && command.goal_id == current.binding.goal_id
                && command.expected_goal_revision == Some(current.binding.goal_revision)
                && command.expected_plan_revision == current.plan_revision
                && command.expected_goal_sequence == Some(current.goal_sequence) =>
        {
            current.binding.next_revision()
        }
        (GoalCommandKindV1::Create | GoalCommandKindV1::Revise, _) => Err(conflict()),
        _ => Err(refused(
            GoalRefusal::ConditionUnsupported,
            "not a definition command",
        )),
    }
}

struct FoldedDefinitions {
    current: Option<DurableGoalDefinitionV1>,
    replay: Option<(String, DurableGoalDefinitionV1)>,
    continuation: run::ContinuationUsage,
}

fn fold_definitions(
    facts: &[Fact],
    session_id: &str,
    workspace: &WorkspaceRef,
    policy_digest: Option<&str>,
    operation: Option<&str>,
) -> Result<FoldedDefinitions, GoalControlError> {
    let mut folded = FoldedDefinitions {
        current: None,
        replay: None,
        continuation: run::ContinuationUsage::default(),
    };
    let mut operations = BTreeSet::new();
    for fact in facts
        .iter()
        .filter(|fact| fact.kind.starts_with("a3s.goal.") || fact.key.starts_with("goal/"))
    {
        let fact_kind = GoalFactKindV1::parse(&fact.kind)?;
        let (operation_id, digest, view) = match fact_kind {
            GoalFactKindV1::Created | GoalFactKindV1::Revised => {
                let kind = if fact_kind == GoalFactKindV1::Created {
                    GoalCommandKindV1::Create
                } else {
                    GoalCommandKindV1::Revise
                };
                let stored = DefinitionFact::deserialize(&fact.payload)
                    .map_err(|_| refused(GoalRefusal::Conflict, "invalid goal definition fact"))?;
                stored.validate_source(fact, session_id, policy_digest, folded.current.as_ref())?;
                let command = stored.command(kind);
                let revision = next_revision(&command, folded.current.as_ref())?;
                let binding = binding_for(&command, revision)?;
                if &binding.workspace != workspace
                    || stored.goal_revision != revision
                    || stored.plan_revision
                        != folded.current.as_ref().and_then(|view| view.plan_revision)
                    || stored.body.binding != binding
                    || stored.command_digest != command_digest(&command)?
                    || binding.scope_fact_sequences.iter().any(|sequence| {
                        *sequence >= fact.seq || !facts.iter().any(|source| source.seq == *sequence)
                    })
                {
                    return Err(refused(
                        GoalRefusal::Conflict,
                        "goal source identity or order mismatch",
                    ));
                }
                let view = stored.view(fact.seq, folded.current.as_ref());
                (stored.operation_id, stored.command_digest, view)
            }
            GoalFactKindV1::PlanProposed | GoalFactKindV1::PlanAccepted => plan::fold_plan(
                fact,
                folded.current.as_ref(),
                session_id,
                workspace,
                policy_digest,
            )?,
            GoalFactKindV1::SliceAdmitted => run::fold_run(
                fact,
                folded.current.as_ref(),
                session_id,
                workspace,
                policy_digest,
                &mut folded.continuation,
            )?,
            GoalFactKindV1::SliceSettled => settle::fold_settle(
                fact,
                folded.current.as_ref(),
                facts,
                session_id,
                workspace,
                policy_digest,
            )?,
            GoalFactKindV1::Completed => complete::fold_complete(
                fact,
                folded.current.as_ref(),
                facts,
                session_id,
                workspace,
                policy_digest,
            )?,
            GoalFactKindV1::StepAccepted => step::fold_step(
                fact,
                folded.current.as_ref(),
                facts,
                session_id,
                workspace,
                policy_digest,
            )
            .map(|(operation, digest, view, _)| {
                folded.continuation.slices_since_progress = 0;
                (operation, digest, view)
            })?,
            GoalFactKindV1::Paused | GoalFactKindV1::Resumed | GoalFactKindV1::Cancelled => {
                control::fold_control(
                    fact,
                    folded.current.as_ref(),
                    session_id,
                    workspace,
                    policy_digest,
                )?
            }
            GoalFactKindV1::WaitDeclared | GoalFactKindV1::WaitObserved => wait::fold_wait(
                fact,
                facts,
                folded.current.as_ref(),
                session_id,
                workspace,
                policy_digest,
                &mut folded.continuation.last_observed_unix_ms,
            )?,
        };
        if !operations.insert(operation_id.clone()) {
            return Err(refused(
                GoalRefusal::Conflict,
                "operation has multiple goal facts",
            ));
        }
        if fact_kind == GoalFactKindV1::PlanAccepted {
            folded.continuation.accepted_plan = view
                .plan
                .as_ref()
                .map(crate::planning::ExecutionPlan::definition_identity)
                .transpose()?;
        }
        if operation == Some(operation_id.as_str()) {
            folded.replay = Some((digest, view.clone()));
        }
        folded.current = Some(view);
    }
    Ok(folded)
}

pub(crate) fn definition_from_facts(
    facts: &[Fact],
    session_id: &str,
    workspace: &WorkspaceRef,
    policy_digest: Option<&str>,
) -> Result<Option<DurableGoalDefinitionV1>, GoalControlError> {
    Ok(fold_definitions(facts, session_id, workspace, policy_digest, None)?.current)
}

/// Check one more automatic opportunity against the same fold; nothing is reserved.
pub(crate) fn check_continuation(
    facts: &[Fact],
    session_id: &str,
    workspace: &WorkspaceRef,
    policy_digest: Option<&str>,
    source: &ContinuationSource,
) -> Result<(), GoalControlError> {
    source.policy.check_admission(source.observed_unix_ms)?;
    fold_definitions(facts, session_id, workspace, policy_digest, None)?
        .continuation
        .admit(source)
}

pub(crate) fn prepare_definition(
    facts: &[Fact],
    session_id: &str,
    workspace: &WorkspaceRef,
    policy_digest: Option<&str>,
    command: &GoalDefinitionCommandV1,
) -> Result<PreparedDefinition, GoalControlError> {
    if command.session_id != session_id || &command.workspace != workspace {
        return Err(refused(
            GoalRefusal::NotFound,
            "goal definition is unavailable",
        ));
    }
    if command.work_policy_digest.as_deref() != policy_digest {
        return Err(refused(
            GoalRefusal::Conflict,
            "policy binding differs from the admitted session",
        ));
    }
    // Validate full input even for replay; source identity is always host-derived.
    binding_for(command, 1)?;
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
    if command
        .scope_fact_sequences
        .iter()
        .any(|sequence| !facts.iter().any(|fact| fact.seq == *sequence))
    {
        return Err(refused(
            GoalRefusal::NotFound,
            "scope source is unavailable",
        ));
    }
    let revision = next_revision(command, folded.current.as_ref())?;
    let binding = binding_for(command, revision)?;
    let kind = if command.kind == GoalCommandKindV1::Create {
        GoalFactKindV1::Created
    } else {
        GoalFactKindV1::Revised
    };
    let stored = DefinitionFact {
        schema: GOAL_FACT_SCHEMA_V1.into(),
        session_id: session_id.into(),
        goal_id: command.goal_id.clone(),
        operation_id: command.operation_id.clone(),
        command_digest: digest,
        goal_revision: revision,
        plan_revision: folded.current.as_ref().and_then(|view| view.plan_revision),
        work_policy_digest: command.work_policy_digest.clone(),
        body: DefinitionBody {
            binding,
            goal: command.goal.clone(),
            expected_goal_revision: command.expected_goal_revision,
            expected_plan_revision: command.expected_plan_revision,
            expected_goal_sequence: command.expected_goal_sequence,
            import_source: command.import_source.clone(),
        },
    };
    Ok(PreparedDefinition {
        pending: Some(NewFact {
            kind: kind.as_str().into(),
            key: goal_fact_key(&command.goal_id, &command.operation_id, kind)?,
            payload: serde_json::to_value(&stored)?,
        }),
        definition: stored.view(0, folded.current.as_ref()),
    })
}

pub(crate) fn refuse_unbound_goal_execution(facts: &[Fact]) -> Result<(), GoalControlError> {
    if facts.iter().any(|fact| fact.kind.starts_with("a3s.goal.")) {
        return Err(refused(
            GoalRefusal::HostDriverConflict,
            "goal-bound execution requires the original qualified goal projection",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
