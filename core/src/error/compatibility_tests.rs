#![cfg(test)]

use super::CodeError;

#[test]
fn original_cancelled_run_has_typed_identity_and_runtime_code() {
    let error = CodeError::run_cancelled("original-run".into());
    assert_eq!(error.cancelled_run_id(), Some("original-run"));
    assert_eq!(error.runtime_code(), "RUN_CANCELLED");
    assert_eq!(error.code(), "INTERNAL_ERROR");
    assert_eq!(error.to_string(), "Run 'original-run' was cancelled");
}

#[test]
fn matching_diagnostic_does_not_classify_as_run_cancellation() {
    let error = CodeError::Internal(anyhow::anyhow!("Run 'original-run' was cancelled"));
    assert_eq!(error.cancelled_run_id(), None);
    assert_eq!(error.runtime_code(), "INTERNAL_ERROR");
    assert_eq!(error.code(), "INTERNAL_ERROR");
}

#[test]
fn runtime_classification_preserves_existing_categories() {
    let error = CodeError::SessionBusy {
        session_id: "session".into(),
    };
    assert_eq!(error.cancelled_run_id(), None);
    assert_eq!(error.runtime_code(), error.code());
}
