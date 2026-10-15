//! Finite automatic opportunities projected from the same original Goal log.

use super::*;
use crate::goal_control::GoalContinuationPolicyV1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContinuationSource {
    pub policy: GoalContinuationPolicyV1,
    pub observed_unix_ms: u64,
}

/// Pending admissions remain counted; this projection never proves settlement.
/// Admission requires every earlier slice settled, so the stall count is over settled slices.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ContinuationUsage {
    pub admitted_slices: u64,
    pub slices_since_progress: u64,
    pub last_observed_unix_ms: u64,
    /// Identity of the currently accepted plan, and of the plan the last slice ran under.
    pub accepted_plan: Option<ExecutionIdentityV1>,
    slice_plan: Option<ExecutionIdentityV1>,
}

impl ContinuationUsage {
    pub fn admit(&mut self, source: &ContinuationSource) -> Result<(), GoalControlError> {
        if source.observed_unix_ms < self.last_observed_unix_ms {
            return Err(refused(
                GoalRefusal::Conflict,
                "automatic host clock moved backwards",
            ));
        }
        source
            .policy
            .check_limits(source.observed_unix_ms, self.admitted_slices, 0)?;
        // At the stall bound, a changed accepted plan earns exactly one more slice;
        // after it the count is past the bound, so a second replan earns nothing.
        let stall = source.policy.max_no_progress_slices.get();
        let replanned =
            self.slices_since_progress == stall && self.accepted_plan != self.slice_plan;
        if self.slices_since_progress >= stall && !replanned {
            return Err(refused(
                GoalRefusal::Waiting,
                "goal stalled without accepted progress; a changed plan or input is required",
            ));
        }
        let next = |value: u64| {
            value.checked_add(1).ok_or_else(|| {
                refused(
                    GoalRefusal::BudgetExhausted,
                    "automatic opportunity count overflowed",
                )
            })
        };
        let admitted = next(self.admitted_slices)?;
        let since_progress = next(self.slices_since_progress)?;
        self.admitted_slices = admitted;
        self.slices_since_progress = since_progress;
        self.last_observed_unix_ms = source.observed_unix_ms;
        self.slice_plan = self.accepted_plan.clone();
        Ok(())
    }
}
