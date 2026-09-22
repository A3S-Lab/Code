//! Host-only attestation hook for the completion gate.
//!
//! [`crate::harness_loop::decide_with_observations`] fails a run whose
//! workspace mutation has no [`crate::verification::VerificationReport`]
//! bound to its effect digest and no matching
//! [`crate::harness_loop::CompletionWaiverV1`]. Both of those are
//! host-supplied, but neither can be produced *before* the run starts: the
//! effect digest is a hash over what the agent actually wrote
//! ([`crate::harness_loop::MutationLedger::digest`]), which does not exist
//! until the mutation has already happened.
//!
//! [`CompletionAttestor`] closes that gap without touching the gate's
//! guarantee. It is invoked once the digest and the mutated paths exist, and
//! strictly *before* [`crate::harness_loop::decide_with_observations`] runs
//! — see
//! [`AgentLoop::complete_no_tool_response`](crate::agent::AgentLoop::complete_no_tool_response).
//! Its only power is to hand back a
//! [`crate::verification::VerificationReport`], which the gate still checks
//! with exactly the same rule it applies to any other report: the digest
//! must match, at least one check must be `required`, and every required
//! check must be [`crate::verification::VerificationStatus::Passed`]
//! (`crate::harness_loop::report_binds_pass`). An attestor cannot mint a
//! waiver, cannot see or influence any other run, and is never exposed to
//! the model as a tool — a model cannot invoke it and cannot forge its
//! output.
//!
//! Plug one in with
//! [`SessionOptions::with_completion_attestor`](crate::SessionOptions::with_completion_attestor).
//! The default is `None`: no existing host is affected until it opts in.

use crate::verification::VerificationReport;
use async_trait::async_trait;
use std::path::Path;

/// Everything a [`CompletionAttestor`] needs to decide whether a workspace
/// mutation is attestable, without granting it any access the framework
/// does not already have at the call site.
pub struct CompletionAttestationRequest<'a> {
    /// The session this mutation happened in, when the framework has one.
    pub session_id: Option<&'a str>,
    /// The workspace root the mutation was observed against.
    pub workspace: &'a Path,
    /// [`crate::harness_loop::MutationLedger::digest`] — the exact digest a
    /// returned report must bind via
    /// [`VerificationReport::effect_digest`] for the gate to accept it.
    pub effect_digest: &'a str,
    /// [`crate::harness_loop::MutationLedger::paths`], collected. May
    /// contain duplicates (one ledger record per write).
    pub mutated_paths: &'a [String],
}

/// Host-supplied completion-gate attestation.
///
/// All methods are async so an implementation can re-read the workspace,
/// call out to a policy service, or run a sandboxed check before deciding.
/// The default returns `None` — no attestation, same as no host being
/// configured at all.
#[async_trait]
pub trait CompletionAttestor: Send + Sync {
    /// Called once per no-tool-call turn that reaches the completion gate
    /// with a non-empty mutation ledger, before the gate decides. Return
    /// `Some(report)` to hand the gate a report it can evaluate; return
    /// `None` to attest nothing (the run proceeds exactly as it would
    /// without an attestor configured).
    ///
    /// A returned report does not have to bind `request.effect_digest` or
    /// carry a `Passed` required check — if it doesn't, the gate simply
    /// does not accept it, the same as any other unbound or failing report.
    /// The attestor cannot force the gate to pass; it can only supply
    /// something for the gate to check.
    async fn attest(
        &self,
        request: &CompletionAttestationRequest<'_>,
    ) -> Option<VerificationReport>;
}
