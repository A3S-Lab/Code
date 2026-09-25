//! Host-only completion attestation (#160).
//!
//! The completion gate stays non-bypassable. Hosts that orchestrate third-party
//! AgentDirs cannot predict [`MutationLedger::digest`](crate::harness_loop::MutationLedger::digest)
//! at session build, so exact waivers are unreachable. A [`CompletionAttestor`]
//! is invoked with the live effect digest and mutated paths **after** they exist
//! and **before** [`decide_with_observations`](crate::harness_loop::decide_with_observations).
//!
//! Each path is paired with the ledger's content digest so a host can re-read
//! the file and compare like-for-like without reaching into harness internals
//! (follow-up shape from community PR #172).
//!
//! The attestor may only supply a [`VerificationReport`].
//! The gate still requires a Passed, digest-bound report. The attestor is not a
//! tool and is not model-grantable. There is no `Observe` policy that lets an
//! unverified mutation complete.

use std::sync::Arc;

use crate::harness_loop::MutationLedger;
use crate::read_only_verifier::{accept_report, ReportAuthor};
use crate::verification::VerificationReport;

/// One mutated path with the content digest the ledger recorded for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutatedPathRecord {
    /// Workspace-relative path as recorded on the ledger.
    pub path: String,
    /// Content digest from [`MutationLedger::content_digest_for_path`].
    pub content_digest: String,
}

/// Host-supplied evidence binder for the completion gate.
///
/// Return `None` to leave the report list unchanged. A returned report still
/// must [`report_binds_pass`](crate::harness_loop) (digest match + required
/// checks Passed) for the gate to allow the run.
pub trait CompletionAttestor: Send + Sync {
    /// Called once the mutation ledger digest and paths exist.
    fn attest(
        &self,
        effect_digest: &str,
        paths: &[MutatedPathRecord],
    ) -> Option<VerificationReport>;
}

/// Invoke the host attestor (if any) and append an accepted Host report.
///
/// No-op when the ledger is empty, the attestor is absent, or attestation
/// returns `None` / is rejected as non-Host.
pub fn merge_attested_report(
    attestor: Option<&Arc<dyn CompletionAttestor>>,
    ledger: &MutationLedger,
    reports: &mut Vec<VerificationReport>,
) {
    let Some(attestor) = attestor else {
        return;
    };
    if ledger.is_empty() {
        return;
    }
    let digest = ledger.digest();
    if digest.trim().is_empty() {
        return;
    }
    let paths: Vec<MutatedPathRecord> = ledger
        .paths()
        .map(|path| MutatedPathRecord {
            path: path.to_string(),
            content_digest: ledger
                .content_digest_for_path(path)
                .unwrap_or("")
                .to_string(),
        })
        .collect();
    let Some(report) = attestor.attest(digest, &paths) else {
        return;
    };
    let Some(report) = accept_report(ReportAuthor::Host, report) else {
        return;
    };
    reports.push(report);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness_loop::{decide_with_observations, MutationLedger};
    use crate::verification::{VerificationCheck, VerificationStatus};

    struct BindingAttestor;

    impl CompletionAttestor for BindingAttestor {
        fn attest(
            &self,
            effect_digest: &str,
            paths: &[MutatedPathRecord],
        ) -> Option<VerificationReport> {
            assert!(!paths.is_empty());
            assert!(paths.iter().any(|p| !p.content_digest.is_empty()));
            Some(
                VerificationReport::new(
                    "host:attestor",
                    vec![VerificationCheck::required(
                        "host:effect",
                        "host_attestation",
                        "host reconciled mutated paths",
                    )
                    .with_status(VerificationStatus::Passed)],
                )
                .with_effect_digest(effect_digest.to_string()),
            )
        }
    }

    struct SilentAttestor;

    impl CompletionAttestor for SilentAttestor {
        fn attest(
            &self,
            _effect_digest: &str,
            _paths: &[MutatedPathRecord],
        ) -> Option<VerificationReport> {
            None
        }
    }

    fn ledger_with_write(path: &str) -> MutationLedger {
        let mut ledger = MutationLedger::default();
        ledger.observe_tool(
            "write",
            0,
            Some(&serde_json::json!({"file_path": path, "after": "fn main() {}"})),
        );
        ledger
    }

    #[test]
    fn attestor_binding_report_closes_gate() {
        let ledger = ledger_with_write("out.txt");
        let mut reports = Vec::new();
        merge_attested_report(
            Some(&(Arc::new(BindingAttestor) as Arc<dyn CompletionAttestor>)),
            &ledger,
            &mut reports,
        );
        assert_eq!(reports.len(), 1);
        let gate = decide_with_observations(&ledger, &reports, &[], false, &[]);
        assert!(matches!(
            gate,
            crate::harness_loop::CompletionGate::Allow(
                crate::harness_loop::CompletionTerminal::Verified { .. }
            )
        ));
    }

    #[test]
    fn silent_attestor_leaves_gate_incomplete() {
        let ledger = ledger_with_write("out.txt");
        let mut reports = Vec::new();
        merge_attested_report(
            Some(&(Arc::new(SilentAttestor) as Arc<dyn CompletionAttestor>)),
            &ledger,
            &mut reports,
        );
        assert!(reports.is_empty());
        let gate = decide_with_observations(&ledger, &reports, &[], false, &[]);
        assert!(matches!(
            gate,
            crate::harness_loop::CompletionGate::Incomplete { .. }
        ));
    }

    #[test]
    fn empty_ledger_skips_attestor() {
        struct PanicAttestor;
        impl CompletionAttestor for PanicAttestor {
            fn attest(&self, _: &str, _: &[MutatedPathRecord]) -> Option<VerificationReport> {
                panic!("must not run on empty ledger");
            }
        }
        let mut reports = Vec::new();
        merge_attested_report(
            Some(&(Arc::new(PanicAttestor) as Arc<dyn CompletionAttestor>)),
            &MutationLedger::default(),
            &mut reports,
        );
        assert!(reports.is_empty());
    }

    #[test]
    fn mismatched_digest_report_cannot_bypass_gate() {
        struct WrongDigestAttestor;
        impl CompletionAttestor for WrongDigestAttestor {
            fn attest(
                &self,
                _effect_digest: &str,
                paths: &[MutatedPathRecord],
            ) -> Option<VerificationReport> {
                assert!(!paths.is_empty());
                Some(
                    VerificationReport::new(
                        "host:attestor",
                        vec![VerificationCheck::required(
                            "host:effect",
                            "host_attestation",
                            "wrong digest on purpose",
                        )
                        .with_status(VerificationStatus::Passed)],
                    )
                    .with_effect_digest("sha256:not-the-ledger-digest".to_string()),
                )
            }
        }
        let ledger = ledger_with_write("out.txt");
        let mut reports = Vec::new();
        merge_attested_report(
            Some(&(Arc::new(WrongDigestAttestor) as Arc<dyn CompletionAttestor>)),
            &ledger,
            &mut reports,
        );
        assert_eq!(reports.len(), 1);
        let gate = decide_with_observations(&ledger, &reports, &[], false, &[]);
        assert!(matches!(
            gate,
            crate::harness_loop::CompletionGate::Incomplete { .. }
        ));
    }
}
