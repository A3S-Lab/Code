//! Whether a Code generation may be replaced by one typed decision.
//!
//! Eligibility is the shape of that generation. Planning pre-analysis returns
//! intent, `requires_planning`, a goal, an execution plan, and
//! `optimized_input`. Goal achievement returns `achieved`, `progress`, and
//! `remaining_criteria`. Each of those is more than one typed answer, so
//! neither moment is eligible. The functions do not score the user prompt.

use super::TypedDecisionResult;

/// Code's judgment for one of its own generations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationAdmission {
    /// `true` only when the generation's entire product is one typed answer.
    pub eligible: bool,
    /// Stable reason. Not a model prompt.
    pub reason: &'static str,
}

/// Planning pre-analysis returns five fields. A typed answer cannot replace it.
pub fn admit_planning_pre_analysis() -> GenerationAdmission {
    GenerationAdmission {
        eligible: false,
        reason: "pre-analysis returns intent, requires_planning, goal, execution_plan, and optimized_input",
    }
}

/// Goal achievement returns three fields and stops the control loop.
pub fn admit_goal_achievement() -> GenerationAdmission {
    GenerationAdmission {
        eligible: false,
        reason: "goal achievement returns achieved, progress, and remaining_criteria",
    }
}

/// Honor an ineligible admission.
///
/// An eligible result is a programming error at these call sites: there is no
/// typed replacement, and the generation must not be skipped.
pub fn enforce_ineligible(admission: GenerationAdmission) -> TypedDecisionResult<()> {
    if admission.eligible {
        Err(super::TypedDecisionError::CannotReplace(admission.reason))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rich_generations_are_not_typed_decisions() {
        let planning = admit_planning_pre_analysis();
        let achievement = admit_goal_achievement();
        assert!(!planning.eligible);
        assert!(!achievement.eligible);
        assert!(planning.reason.contains("optimized_input"));
        assert!(achievement.reason.contains("remaining_criteria"));
        assert!(!planning.reason.contains("refactor"));
        assert!(!achievement.reason.contains("done"));
        enforce_ineligible(planning).expect("pre-analysis stays a generation");
        enforce_ineligible(achievement).expect("achievement stays a generation");
    }

    #[test]
    fn eligible_admission_cannot_skip_the_generation() {
        let err = enforce_ineligible(GenerationAdmission {
            eligible: true,
            reason: "single typed answer",
        })
        .expect_err("eligible must not skip");
        assert!(matches!(
            err,
            super::super::TypedDecisionError::CannotReplace(_)
        ));
    }
}
