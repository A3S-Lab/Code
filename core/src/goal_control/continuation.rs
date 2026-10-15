//! Read-only tick decisions; a preview never grants original Run admission.

use super::*;
use std::num::NonZeroU64;

/// Finite host-selected bounds, without clock, source or execution authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalContinuationPolicyV1 {
    pub automatic_enabled: bool,
    pub max_slices: NonZeroU64,
    pub max_no_progress_slices: NonZeroU64,
    pub deadline_unix_ms: NonZeroU64,
}

/// Ceilings for a newly admitted policy. Replaying recorded facts does not apply
/// them, so a log written before they existed stays readable.
pub(crate) const MAX_GOAL_SLICES: u64 = 1_000;
pub(crate) const MAX_GOAL_HORIZON_MS: u64 = 30 * 24 * 60 * 60 * 1_000;

impl GoalContinuationPolicyV1 {
    /// Clock-free ceiling, checked where the policy is declared.
    pub fn check_bounds(&self) -> Result<(), GoalControlError> {
        if self.max_slices.get() > MAX_GOAL_SLICES {
            return Err(GoalControlError::Invalid {
                field: "max_slices",
                reason: "must not exceed 1000",
            });
        }
        Ok(())
    }

    /// Ceilings checked before a new automatic slice is admitted.
    pub fn check_admission(&self, now_unix_ms: u64) -> Result<(), GoalControlError> {
        self.check_bounds()?;
        if self.deadline_unix_ms.get().saturating_sub(now_unix_ms) > MAX_GOAL_HORIZON_MS {
            return Err(GoalControlError::Invalid {
                field: "deadline_unix_ms",
                reason: "must be within 30 days of the current host clock",
            });
        }
        Ok(())
    }

    /// Check numeric bounds only. Callers must separately validate current host
    /// authority, clock provenance and counts from the original fact history.
    pub fn check_limits(
        &self,
        now_unix_ms: u64,
        admitted_slices: u64,
        no_progress_slices: u64,
    ) -> Result<(), GoalControlError> {
        if now_unix_ms == 0 {
            return Err(GoalControlError::Invalid {
                field: "now_unix_ms",
                reason: "requires a positive current host clock observation",
            });
        }
        if now_unix_ms >= self.deadline_unix_ms.get() {
            return Err(GoalControlError::Refused {
                refusal: GoalRefusal::DeadlineExpired,
                reason: "automatic continuation deadline has expired",
            });
        }
        if !self.automatic_enabled
            || admitted_slices >= self.max_slices.get()
            || no_progress_slices >= self.max_no_progress_slices.get()
        {
            return Err(GoalControlError::Refused {
                refusal: GoalRefusal::BudgetExhausted,
                reason: "automatic continuation is disabled or its finite limit is exhausted",
            });
        }
        Ok(())
    }
}

pub(crate) fn preview_tick(
    facts: &[a3s_effect::Fact],
    session_id: &str,
    workspace: &WorkspaceRef,
    policy_digest: Option<&str>,
    command: &GoalControlCommandV1,
    original_run_unsettled: bool,
    continuation: Option<&super::ContinuationSource>,
) -> Result<GoalTickDecisionV1, GoalControlError> {
    let refuse = |refusal, reason| GoalControlError::Refused { refusal, reason };
    validate_id("operation_id", &command.operation_id)?;
    if command.kind != GoalCommandKindV1::Tick {
        return Err(refuse(
            GoalRefusal::ConditionUnsupported,
            "preview requires a tick command",
        ));
    }
    if command.session_id != session_id || &command.workspace != workspace {
        return Err(refuse(GoalRefusal::NotFound, "goal tick is unavailable"));
    }
    let view = definition_from_facts(facts, session_id, workspace, policy_digest)?
        .filter(|view| view.binding.goal_id == command.goal_id)
        .ok_or_else(|| refuse(GoalRefusal::NotFound, "goal tick is unavailable"))?;
    if command.work_policy_digest.as_deref() != policy_digest {
        return Err(refuse(
            GoalRefusal::Conflict,
            "tick policy differs from its original source",
        ));
    }
    if super::facts::matches_tick_run(facts, command)? {
        return Ok(GoalTickDecisionV1::Reconcile);
    }
    if facts.iter().any(|fact| {
        fact.kind.starts_with("a3s.goal.")
            && fact.payload["operation_id"].as_str() == Some(command.operation_id.as_str())
    }) {
        return Err(refuse(
            GoalRefusal::Conflict,
            "operation already belongs to another goal command",
        ));
    }
    if original_run_unsettled || super::facts::slice_unsettled(facts) {
        return Ok(GoalTickDecisionV1::Reconcile);
    }
    if command.expected_goal_revision != view.binding.goal_revision
        || command.expected_plan_revision != view.plan_revision
        || command.expected_goal_sequence != view.goal_sequence
    {
        return Err(refuse(
            GoalRefusal::Conflict,
            "tick preview references are stale",
        ));
    }
    match view.state {
        GoalState::Paused => return Ok(GoalTickDecisionV1::Paused),
        GoalState::Waiting => return Ok(GoalTickDecisionV1::Wait),
        GoalState::Completed => return Ok(GoalTickDecisionV1::Complete),
        GoalState::Cancelled => return Err(refuse(GoalRefusal::Conflict, "goal is cancelled")),
        GoalState::Active => {}
    }
    if !view.plan_accepted {
        return Ok(GoalTickDecisionV1::NeedsInput);
    }
    if view.ready_steps().is_empty() {
        if super::facts::completion_ready(facts, &view)? {
            return Ok(GoalTickDecisionV1::Reconcile);
        }
        // A finished plan that cannot close, or an open question/confirmation,
        // needs a person; other waits need the host.
        let open = || view.waits.iter().filter(|wait| wait.observed.is_none());
        let needs_input = open().next().is_none()
            || open().any(|wait| {
                matches!(
                    wait.condition.kind(),
                    GoalWaitKindV1::QuestionAnswer | GoalWaitKindV1::ConfirmationAnswer
                )
            });
        return Ok(if needs_input {
            GoalTickDecisionV1::NeedsInput
        } else {
            GoalTickDecisionV1::Wait
        });
    }
    // Absent trusted host bounds mean automatic continuation is disabled.
    let Some(source) = continuation else {
        return Ok(GoalTickDecisionV1::BudgetExhausted);
    };
    match super::facts::check_continuation(facts, session_id, workspace, policy_digest, source) {
        Ok(()) => Ok(GoalTickDecisionV1::Advance),
        Err(GoalControlError::Refused {
            refusal: GoalRefusal::BudgetExhausted,
            ..
        }) => Ok(GoalTickDecisionV1::BudgetExhausted),
        // Stalled: only a changed plan (one budgeted replan) or a person can move it.
        Err(GoalControlError::Refused {
            refusal: GoalRefusal::Waiting,
            ..
        }) => Ok(GoalTickDecisionV1::NeedsInput),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests;
