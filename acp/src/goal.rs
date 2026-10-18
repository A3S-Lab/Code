//! Host-side `/goal` for the pager.
//!
//! The command stores one durable objective per ACP session and turns Core
//! planning on or off with [`AgentSession::set_planning_mode`]. It does not
//! start a second planner, classifier, or background workflow.

use a3s_code_core::goal_control::{
    DurableGoalDefinitionV1, GoalCommandKindV1, GoalControlCommandV1, GoalControlError,
    GoalRefusal, GoalState,
};
use a3s_code_core::PlanningMode;

/// One user-set objective for an ACP session.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableGoal {
    pub objective: String,
    pub paused: bool,
}

mod import;
pub(crate) use import::{commit_import, preview_import, GoalImportPreviewRequest};
mod persistence;
pub(crate) use persistence::{apply_session_goal, load_goal, retire_achieved_durable_goal};

pub(crate) fn host_clock_ms() -> anyhow::Result<u64> {
    use anyhow::Context;
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .context("host clock")?
            .as_millis(),
    )
    .context("host clock overflow")
}

/// Product-host bounds; a trusted ACL declaration, including disabled, wins.
pub(crate) fn default_continuation_policy(
    now_ms: u64,
) -> anyhow::Result<a3s_code_core::goal_control::GoalContinuationPolicyV1> {
    use anyhow::Context;
    use std::num::NonZeroU64;
    let deadline = now_ms
        .checked_add(24 * 60 * 60 * 1_000)
        .context("goal deadline overflow")?;
    let policy = a3s_code_core::goal_control::GoalContinuationPolicyV1 {
        automatic_enabled: true,
        max_slices: NonZeroU64::new(8).context("goal slice bound")?,
        max_no_progress_slices: NonZeroU64::new(2).context("goal progress bound")?,
        deadline_unix_ms: NonZeroU64::new(deadline).context("goal deadline")?,
    };
    policy.check_admission(now_ms)?;
    Ok(policy)
}

/// What the user asked `/goal` to do.
#[derive(Debug, PartialEq, Eq)]
pub enum GoalOp {
    Status,
    Pause,
    Resume,
    Clear,
    Set(String),
}

/// How the next Core turn should treat planning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalPlan {
    Unchanged,
    Enabled,
    Disabled,
    ClearOverride,
}

impl GoalPlan {
    pub fn mode(self) -> Option<PlanningMode> {
        match self {
            Self::Enabled => Some(PlanningMode::Enabled),
            Self::Disabled => Some(PlanningMode::Disabled),
            Self::Unchanged | Self::ClearOverride => None,
        }
    }

    pub fn clears_override(self) -> bool {
        matches!(self, Self::ClearOverride)
    }
}

/// Result of applying one `/goal` operation to the stored objective.
#[derive(Debug, PartialEq, Eq)]
pub struct AppliedGoal {
    /// `None` removes the stored objective.
    pub next: Option<DurableGoal>,
    pub plan: GoalPlan,
    /// Shown to the user without a model turn.
    pub reply: Option<String>,
    /// Sent to Core when the operation starts work.
    pub model_prompt: Option<String>,
}

const USAGE: &str = "Usage: /goal <objective> | status | pause | resume | clear";

/// Parse a whole prompt that is a `/goal` command.
///
/// A longer token such as `/goalie` is not a goal command.
pub fn parse_goal_command(prompt: &str) -> Option<GoalOp> {
    let trimmed = prompt.trim();
    let rest = trimmed.strip_prefix("/goal")?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let args = rest.trim();
    match args.to_ascii_lowercase().as_str() {
        "" | "status" => Some(GoalOp::Status),
        "pause" => Some(GoalOp::Pause),
        "resume" => Some(GoalOp::Resume),
        "clear" => Some(GoalOp::Clear),
        _ => Some(GoalOp::Set(args.to_string())),
    }
}

fn none_message() -> String {
    format!("No durable goal is set.\n{USAGE}")
}

fn status_line(goal: &DurableGoal) -> String {
    if goal.paused {
        format!("Durable goal paused: {}", goal.objective)
    } else {
        format!("Durable goal active: {}", goal.objective)
    }
}

/// Prompt Core sees for a newly set goal.
pub fn goal_turn_prompt(objective: &str) -> String {
    format!(
        "{objective}\n\nThis is a durable goal. Work until it is met, then stop. \
Do not ask whether to continue unless you are blocked."
    )
}

pub fn apply_goal(current: Option<DurableGoal>, op: GoalOp) -> AppliedGoal {
    match op {
        GoalOp::Status => AppliedGoal {
            reply: Some(
                current
                    .as_ref()
                    .map(status_line)
                    .unwrap_or_else(none_message),
            ),
            next: current,
            plan: GoalPlan::Unchanged,
            model_prompt: None,
        },
        GoalOp::Pause => match current {
            None => AppliedGoal {
                next: None,
                plan: GoalPlan::Unchanged,
                reply: Some(none_message()),
                model_prompt: None,
            },
            Some(mut goal) => {
                goal.paused = true;
                let reply = status_line(&goal);
                AppliedGoal {
                    next: Some(goal),
                    plan: GoalPlan::Disabled,
                    reply: Some(reply),
                    model_prompt: None,
                }
            }
        },
        GoalOp::Resume => match current {
            None => AppliedGoal {
                next: None,
                plan: GoalPlan::Unchanged,
                reply: Some(none_message()),
                model_prompt: None,
            },
            Some(mut goal) => {
                let prompt = format!(
                    "Continue this durable goal until it is met:\n\n{}",
                    goal.objective
                );
                goal.paused = false;
                AppliedGoal {
                    next: Some(goal),
                    plan: GoalPlan::Enabled,
                    reply: None,
                    model_prompt: Some(prompt),
                }
            }
        },
        GoalOp::Clear => {
            if current.is_none() {
                AppliedGoal {
                    next: None,
                    plan: GoalPlan::Unchanged,
                    reply: Some(none_message()),
                    model_prompt: None,
                }
            } else {
                AppliedGoal {
                    next: None,
                    plan: GoalPlan::ClearOverride,
                    reply: Some("Cleared the durable goal.".to_string()),
                    model_prompt: None,
                }
            }
        }
        GoalOp::Set(objective) => AppliedGoal {
            next: Some(DurableGoal {
                objective: objective.clone(),
                paused: false,
            }),
            plan: GoalPlan::Enabled,
            reply: None,
            model_prompt: Some(goal_turn_prompt(&objective)),
        },
    }
}

/// `a3s/goal/state` request: the bound definition of one live session.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GoalStateRequest {
    pub session_id: String,
}

/// A bound `/goal` reply, and whether this operation wakes bounded execution.
pub(crate) struct BoundGoalReply {
    pub reply: String,
    pub tick: bool,
}

fn control_command(
    current: &DurableGoalDefinitionV1,
    kind: GoalCommandKindV1,
    operation_id: String,
) -> GoalControlCommandV1 {
    GoalControlCommandV1 {
        kind,
        operation_id,
        goal_id: current.binding.goal_id.clone(),
        session_id: current.binding.session_id.clone(),
        workspace: current.binding.workspace.clone(),
        work_policy_digest: current.binding.work_policy_digest.clone(),
        expected_goal_revision: current.binding.goal_revision,
        expected_plan_revision: current.plan_revision,
        expected_goal_sequence: current.goal_sequence,
    }
}

/// Bound sessions use the original Core facts, never the legacy workspace file.
/// A first `set` binds the exact explicit task; `set` and `resume` wake the
/// original bounded driver. Core gates each slice on the trusted host policy.
pub(crate) async fn apply_bound_goal(
    session: &a3s_code_core::AgentSession,
    op: &GoalOp,
) -> anyhow::Result<Option<BoundGoalReply>> {
    let created = if let GoalOp::Set(task) = op {
        if session.goal_definition()?.is_none()
            && load_goal(session.workspace(), session.session_id())
                .await?
                .is_none()
        {
            bind_task_goal(session, task)?;
            true
        } else {
            false
        }
    } else {
        false
    };
    let Some(mut current) = session.goal_definition()? else {
        return Ok(None);
    };
    let control = |current: &DurableGoalDefinitionV1, kind: GoalCommandKindV1| {
        let operation_id = format!("acp-{}-{}", kind.as_str(), current.goal_sequence);
        session.apply_goal_control(control_command(current, kind, operation_id))
    };
    let mut tick = false;
    match op {
        GoalOp::Status => {}
        GoalOp::Pause if current.state != GoalState::Paused => {
            current = control(&current, GoalCommandKindV1::Pause)?;
        }
        GoalOp::Pause => {}
        GoalOp::Resume => {
            if current.state == GoalState::Paused {
                current = control(&current, GoalCommandKindV1::Resume)?;
            }
            tick = !matches!(current.state, GoalState::Completed | GoalState::Cancelled);
        }
        GoalOp::Clear if current.state != GoalState::Cancelled => {
            current = control(&current, GoalCommandKindV1::Cancel)?;
        }
        GoalOp::Clear => {}
        GoalOp::Set(_) if created => tick = true,
        GoalOp::Set(_) => {
            return Err(GoalControlError::Refused {
                refusal: GoalRefusal::ConditionUnsupported,
                reason: "a bound Goal is revised only through a3s/goal/definition",
            }
            .into());
        }
    }
    let mut reply = bound_status(session, &current).await?;
    if matches!(op, GoalOp::Clear) {
        reply.push_str("\nUse /new to start a different goal or ordinary work.");
    }
    Ok(Some(BoundGoalReply { reply, tick }))
}

/// State, accepted plan progress and the next decision Core would take.
pub(crate) async fn bound_status(
    session: &a3s_code_core::AgentSession,
    current: &DurableGoalDefinitionV1,
) -> anyhow::Result<String> {
    let mut lines = vec![format!(
        "Durable goal {}: {}",
        current.state.as_str(),
        current.goal.description
    )];
    match (&current.plan, current.plan_revision) {
        (Some(plan), Some(revision)) => {
            let done = plan
                .steps
                .iter()
                .filter(|step| step.status == a3s_code_core::planning::TaskStatus::Completed)
                .count();
            let accepted = if current.plan_accepted {
                "accepted"
            } else {
                "proposed"
            };
            lines.push(format!(
                "Plan r{revision} {accepted}: {done}/{} steps done",
                plan.steps.len()
            ));
        }
        _ => lines.push("Plan: none".into()),
    }
    if !matches!(current.state, GoalState::Completed | GoalState::Cancelled) {
        let preview = control_command(current, GoalCommandKindV1::Tick, "acp-status".into());
        lines.push(format!(
            "Next: {}",
            session.preview_goal_tick(&preview).await?.as_str()
        ));
    }
    Ok(lines.join("\n"))
}

/// What the host should do with a bound goal after a slice, or before the
/// first one. `Advance` carries a fresh tick command. A preview does not
/// admit a Run.
pub(crate) enum BoundGoalStep {
    Advance(GoalControlCommandV1),
    Finished,
    Stopped(&'static str),
}

pub(crate) async fn bound_goal_step(
    session: &a3s_code_core::AgentSession,
) -> anyhow::Result<BoundGoalStep> {
    let Some(current) = session.goal_definition()? else {
        return Ok(BoundGoalStep::Finished);
    };
    if matches!(current.state, GoalState::Completed | GoalState::Cancelled) {
        return Ok(BoundGoalStep::Finished);
    }
    let Some(command) = bound_tick_command(session)? else {
        return Ok(BoundGoalStep::Finished);
    };
    let decision = session.preview_goal_tick(&command).await?;
    Ok(match decision {
        a3s_code_core::goal_control::GoalTickDecisionV1::Advance => BoundGoalStep::Advance(command),
        a3s_code_core::goal_control::GoalTickDecisionV1::Complete => BoundGoalStep::Finished,
        other => BoundGoalStep::Stopped(other.as_str()),
    })
}

/// Bind an explicit task as a durable goal with one accepted step, then
/// return the tick for that step. The task text is the goal, the step and its
/// criterion; nothing is derived from it.
pub(crate) fn bind_task_goal(
    session: &a3s_code_core::AgentSession,
    task: &str,
) -> anyhow::Result<GoalControlCommandV1> {
    use a3s_code_core::goal_control::{GoalDefinitionCommandV1, GoalPlanCommandV1};
    use a3s_code_core::planning::{AgentGoal, Complexity, ExecutionPlan, Task};
    let criterion = "Every requirement of the task holds in the workspace.";
    let definition = session.apply_goal_definition(GoalDefinitionCommandV1 {
        kind: GoalCommandKindV1::Create,
        import_source: None,
        operation_id: "unattended-create".into(),
        goal_id: "unattended-task".into(),
        session_id: session.session_id().into(),
        workspace: a3s_code_core::workspace::WorkspaceServices::local(session.workspace())
            .workspace_ref()
            .clone(),
        goal: AgentGoal::new(task).with_criteria(vec![criterion.into()]),
        non_goals: vec![],
        scope_fact_sequences: vec![],
        work_policy_digest: session.work_policy_digest()?,
        expected_goal_revision: None,
        expected_plan_revision: None,
        expected_goal_sequence: None,
    })?;
    let mut plan = ExecutionPlan::new(task, Complexity::Simple);
    plan.upsert_step(Task::new("task", task).with_success_criteria(criterion));
    let mut proposal = GoalPlanCommandV1 {
        kind: GoalCommandKindV1::ProposePlan,
        operation_id: "unattended-plan".into(),
        goal_id: definition.binding.goal_id,
        session_id: definition.binding.session_id,
        workspace: definition.binding.workspace,
        work_policy_digest: definition.binding.work_policy_digest,
        expected_goal_revision: definition.binding.goal_revision,
        expected_plan_revision: definition.plan_revision,
        expected_goal_sequence: definition.goal_sequence,
        plan: Some(plan),
        proposal_sequence: None,
    };
    let proposed = session.apply_goal_plan(proposal.clone())?;
    proposal.kind = GoalCommandKindV1::AcceptPlan;
    proposal.operation_id = "unattended-accept".into();
    proposal.expected_plan_revision = proposed.plan_revision;
    proposal.expected_goal_sequence = proposed.goal_sequence;
    proposal.plan = None;
    proposal.proposal_sequence = proposed.plan_fact_sequence;
    session.apply_goal_plan(proposal)?;
    bound_tick_command(session)?
        .ok_or_else(|| anyhow::anyhow!("goal.not_found: the bound task goal is missing"))
}

pub(crate) fn bound_tick_command(
    session: &a3s_code_core::AgentSession,
) -> anyhow::Result<Option<GoalControlCommandV1>> {
    Ok(session.goal_definition()?.map(|current| {
        let operation_id = format!("acp-tick-{}", uuid::Uuid::new_v4());
        control_command(&current, GoalCommandKindV1::Tick, operation_id)
    }))
}

/// The slice's Run outcome (when one ran) and the goal status after it.
pub(crate) async fn bound_slice_report(
    session: &a3s_code_core::AgentSession,
    run_id: Option<&str>,
) -> anyhow::Result<String> {
    let mut lines = Vec::new();
    if let Some(run_id) = run_id {
        let snapshot = session
            .run_snapshot(run_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("goal slice {run_id} has no Run record"))?;
        lines.push(format!("Goal slice {}: {:?}", snapshot.id, snapshot.status));
    }
    if let Some(after) = session.goal_definition()? {
        lines.push(bound_status(session, &after).await?);
    }
    Ok(lines.join("\n\n"))
}

/// Later user turns carry an active objective until it is paused or cleared.
pub fn standing_goal_prompt(goal: Option<&DurableGoal>, user: &str) -> String {
    match goal {
        Some(goal) if !goal.paused && !goal.objective.trim().is_empty() => format!(
            "Note: a durable goal is set for this session: \"{}\". It stays in force until \"/goal clear\".\
If the user's message asks to work on this goal or clearly continues it, proceed; a greeting or \
an unrelated question needs only a normal reply — do not start executing the goal unprompted.\n\n{user}",
            goal.objective.trim()
        ),
        _ => user.to_string(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn product_default_bounds_are_finite_and_explicit() {
        let policy = super::default_continuation_policy(5_000).unwrap();
        assert!(policy.automatic_enabled);
        assert_eq!(policy.max_slices.get(), 8);
        assert_eq!(policy.max_no_progress_slices.get(), 2);
        assert_eq!(policy.deadline_unix_ms.get(), 5_000 + 24 * 60 * 60 * 1_000);
        assert!(policy.check_limits(5_000, 8, 0).is_err());
        assert!(policy.check_limits(5_000, 0, 2).is_err());
        assert!(policy
            .check_limits(policy.deadline_unix_ms.get(), 0, 0)
            .is_err());
        assert!(super::default_continuation_policy(u64::MAX).is_err());
    }

    use super::*;

    #[test]
    fn parse_rejects_a_longer_token() {
        assert!(parse_goal_command("/goalie ship it").is_none());
        assert!(parse_goal_command("please /goal ship it").is_none());
    }

    #[test]
    fn parse_verbs_and_objectives() {
        assert_eq!(parse_goal_command("/goal"), Some(GoalOp::Status));
        assert_eq!(parse_goal_command("  /goal status"), Some(GoalOp::Status));
        assert_eq!(parse_goal_command("/goal PAUSE"), Some(GoalOp::Pause));
        assert_eq!(parse_goal_command("/goal resume"), Some(GoalOp::Resume));
        assert_eq!(parse_goal_command("/goal clear"), Some(GoalOp::Clear));
        assert_eq!(
            parse_goal_command("/goal Pause the rollout"),
            Some(GoalOp::Set("Pause the rollout".to_string()))
        );
    }

    #[test]
    fn set_enables_planning_and_keeps_the_objective() {
        let applied = apply_goal(None, GoalOp::Set("Ship the fix".into()));
        assert_eq!(applied.plan, GoalPlan::Enabled);
        assert_eq!(
            applied.next,
            Some(DurableGoal {
                objective: "Ship the fix".into(),
                paused: false,
            })
        );
        let prompt = applied.model_prompt.expect("model prompt");
        assert!(prompt.starts_with("Ship the fix\n"));
        assert!(!prompt.contains("/goal"));
        assert!(applied.reply.is_none());
    }

    #[test]
    fn status_pause_resume_and_clear_round_trip() {
        let set = apply_goal(None, GoalOp::Set("Ship the fix".into()));
        let goal = set.next.clone().expect("goal");

        let status = apply_goal(Some(goal.clone()), GoalOp::Status);
        assert_eq!(status.plan, GoalPlan::Unchanged);
        assert_eq!(
            status.reply.as_deref(),
            Some("Durable goal active: Ship the fix")
        );
        assert!(status.model_prompt.is_none());

        let paused = apply_goal(Some(goal.clone()), GoalOp::Pause);
        assert_eq!(paused.plan, GoalPlan::Disabled);
        assert!(paused.next.as_ref().is_some_and(|goal| goal.paused));
        assert!(paused.model_prompt.is_none());

        let resumed = apply_goal(paused.next, GoalOp::Resume);
        assert_eq!(resumed.plan, GoalPlan::Enabled);
        assert!(resumed.next.as_ref().is_some_and(|goal| !goal.paused));
        assert!(resumed
            .model_prompt
            .as_deref()
            .is_some_and(|prompt| prompt.contains("Ship the fix")));

        let cleared = apply_goal(resumed.next, GoalOp::Clear);
        assert_eq!(cleared.next, None);
        assert!(cleared.plan.clears_override());
        assert_eq!(cleared.reply.as_deref(), Some("Cleared the durable goal."));
    }

    #[test]
    fn missing_goal_does_not_start_a_turn() {
        for op in [GoalOp::Status, GoalOp::Pause, GoalOp::Resume, GoalOp::Clear] {
            let applied = apply_goal(None, op);
            assert_eq!(applied.next, None);
            assert_eq!(applied.plan, GoalPlan::Unchanged);
            assert!(applied.model_prompt.is_none());
            assert!(applied.reply.unwrap().contains("No durable goal is set."));
        }
    }
}
