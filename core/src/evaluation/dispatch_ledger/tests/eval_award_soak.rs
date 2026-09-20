//! S-EV-01: a restart between claim and award still leaves one receipt.

use super::{
    identity, receipt, EvaluationDispatchClaimOutcome, EvaluationDispatchLedger,
    EvaluationDispatchLedgerError, FileEvaluationDispatchLedger,
};

const CLAIMS: usize = 20;
const REQUEST_DIGEST: &str =
    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

#[tokio::test]
#[ignore = "S-EV-01 soak: 20 dispatch claims restarted before award"]
async fn soak_restarted_dispatch_awards_once() {
    let root = tempfile::tempdir().unwrap();
    let canonical = identity("a3s.test.dispatch-soak");
    let awarded = receipt(canonical.clone());
    let mut other = awarded.clone();
    other.result_digest =
        Some("sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".to_string());

    for index in 0..CLAIMS {
        let dispatch_id = format!("dispatch-{index}");
        let ledger = FileEvaluationDispatchLedger::open(root.path())
            .await
            .expect("open ledger");
        assert!(matches!(
            ledger
                .claim_with_identity(
                    &dispatch_id,
                    REQUEST_DIGEST,
                    &canonical,
                    "owner-1",
                    1_000,
                    60_000,
                )
                .await
                .expect("claim"),
            EvaluationDispatchClaimOutcome::Claimed { attempt: 1 }
        ));
        drop(ledger);

        let ledger = FileEvaluationDispatchLedger::open(root.path())
            .await
            .expect("reopen before award");
        ledger
            .complete_with_receipt(
                &dispatch_id,
                REQUEST_DIGEST,
                &canonical,
                "owner-1",
                &awarded,
                2_000,
            )
            .await
            .expect("first award");
        drop(ledger);

        let ledger = FileEvaluationDispatchLedger::open(root.path())
            .await
            .expect("reopen after award");
        assert_eq!(
            ledger
                .completed_receipt(&dispatch_id)
                .await
                .expect("read receipt"),
            Some(awarded.clone())
        );
        assert!(matches!(
            ledger
                .complete_with_receipt(
                    &dispatch_id,
                    REQUEST_DIGEST,
                    &canonical,
                    "owner-1",
                    &other,
                    3_000,
                )
                .await,
            Err(EvaluationDispatchLedgerError::Conflict)
        ));
        assert!(matches!(
            ledger
                .claim_with_identity(
                    &dispatch_id,
                    REQUEST_DIGEST,
                    &canonical,
                    "owner-2",
                    4_000,
                    60_000,
                )
                .await
                .expect("second claim"),
            EvaluationDispatchClaimOutcome::Completed
        ));
    }
}
