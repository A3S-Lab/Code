//! Configured Bash allows remain subject to the shared request confirmation floor.

use crate::permission::auto_mode::ClassifierSecurityFinding;
use crate::permission::auto_mode::ClassifierSecurityFinding::{UnresolvedArgument, UnvettedEnv};
use crate::permission::grants::{
    BashEvaluation, bash_request_floor_requires_prompt, evaluate_bash_with_ambient,
};
use crate::permission::policy::CompiledPolicy;
use crate::permission::state::PermissionState;
use crate::permission::types::AccessKind;

/// A configured Allow clears the floor for a recovered filename script whose only findings are the
/// recovered argument and the witnessed assignment.
pub(crate) fn configured_filename_allow(evaluation: Option<&BashEvaluation>) -> bool {
    evaluation.is_some_and(|e| {
        e.recovered_eligible
            && ClassifierSecurityFinding::ALL.iter().all(|finding| {
                matches!(finding, UnresolvedArgument | UnvettedEnv)
                    || !e.assessment.contains(*finding)
            })
    })
}

/// Auto mode must classify a recovered script unless an exact whole-script grant already authorizes it.
pub(crate) fn requires_recovered_classification(evaluation: Option<&BashEvaluation>) -> bool {
    evaluation.is_some_and(|e| e.assessment.contains(UnresolvedArgument) && !e.exact_grant)
}

/// Whether a configured allow rule clears the bash request floor in ask/dontAsk. The assessment is `FileWrite`-only (other floor findings describe effects outside the rule's matched words).
/// The writes are command-word operands rather than redirects (which word matching cannot see).
fn narrow_allow_clears_write_floor(
    evaluation: Option<&BashEvaluation>,
    policy: Option<&CompiledPolicy>,
    access: &AccessKind,
) -> bool {
    evaluation.is_some_and(|e| e.assessment.is_file_write_only() && !e.redirect_write)
        && policy.is_some_and(|p| p.narrow_allow_authorizes(access))
}

/// Whether a configured policy Allow is deferred to the confirmation floor for this request.
/// Callers reach this only on a policy Allow. `configured_mode`: configured rules decide (not dontAsk).
pub(crate) fn broad_allow_deferred(
    evaluation: Option<&BashEvaluation>,
    policy: Option<&CompiledPolicy>,
    access: &AccessKind,
    configured_mode: bool,
) -> bool {
    (requires_recovered_classification(evaluation)
        || bash_request_floor_requires_prompt(evaluation))
        && !narrow_allow_clears_write_floor(evaluation, policy, access)
        && !(configured_mode && configured_filename_allow(evaluation))
}

/// Whether a configured Bash Allow still requires confirmation for a caller with no manager or session grants.
/// Each call assesses the command in full, ambient git scan included, on the caller's thread.
/// Non-Bash access is never deferred.
pub fn broad_allow_floor_requires_prompt(
    access: &AccessKind,
    policy: Option<&CompiledPolicy>,
    cwd: &std::path::Path,
    configured_mode: bool,
) -> bool {
    let AccessKind::Bash(cmd) = access else {
        return false;
    };
    let evaluation = evaluate_bash_with_ambient(cmd, &PermissionState::default(), cwd);
    broad_allow_deferred(Some(&evaluation), policy, access, configured_mode)
}

#[cfg(test)]
#[path = "bash_policy_allow_tests.rs"]
mod tests;
