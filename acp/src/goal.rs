//! Host-side `/goal` for the pager.
//!
//! The command stores one durable objective per ACP session and turns Core
//! planning on or off with [`AgentSession::set_planning_mode`]. It does not
//! start a second planner, classifier, or background workflow.

use std::path::Path;

use a3s_code_core::PlanningMode;

/// One user-set objective for an ACP session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableGoal {
    pub objective: String,
    pub paused: bool,
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

/// Later user turns carry an active objective until it is paused or cleared.
pub fn standing_goal_prompt(goal: Option<&DurableGoal>, user: &str) -> String {
    match goal {
        Some(goal) if !goal.paused && !goal.objective.trim().is_empty() => format!(
            "Durable goal, still in force until it is cleared:\n{}\n\n{user}",
            goal.objective.trim()
        ),
        _ => user.to_string(),
    }
}

fn goal_file(workspace: &Path) -> std::path::PathBuf {
    workspace.join(".a3s").join("durable-goal.json")
}

/// Persist the objective so a new process can reload it.
pub fn store_goal(workspace: &Path, goal: &DurableGoal) -> std::io::Result<()> {
    let path = goal_file(workspace);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::json!({
        "objective": goal.objective,
        "paused": goal.paused,
    });
    let bytes = serde_json::to_vec_pretty(&body).map_err(std::io::Error::other)?;
    std::fs::write(path, bytes)
}

pub fn load_goal(workspace: &Path) -> Option<DurableGoal> {
    let raw = std::fs::read_to_string(goal_file(workspace)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let objective = value.get("objective")?.as_str()?.trim().to_string();
    if objective.is_empty() {
        return None;
    }
    let paused = value
        .get("paused")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    Some(DurableGoal { objective, paused })
}

pub fn clear_stored_goal(workspace: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(goal_file(workspace)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
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
    fn an_active_goal_stays_on_later_turns_and_survives_reload() {
        let dir = std::env::temp_dir().join(format!(
            "a3s-goal-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp workspace");
        let goal = DurableGoal {
            objective: "Ship the parser".into(),
            paused: false,
        };
        store_goal(&dir, &goal).expect("store");
        let loaded = load_goal(&dir).expect("reload");
        assert_eq!(loaded, goal);
        let later = standing_goal_prompt(Some(&loaded), "look at the next file");
        assert!(later.contains("Ship the parser"));
        assert!(later.contains("look at the next file"));
        let paused = DurableGoal {
            objective: goal.objective.clone(),
            paused: true,
        };
        assert_eq!(
            standing_goal_prompt(Some(&paused), "look at the next file"),
            "look at the next file"
        );
        clear_stored_goal(&dir).expect("clear");
        assert!(load_goal(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
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
