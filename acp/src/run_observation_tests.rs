#![cfg(test)]

use super::*;
use serde_json::json;

#[test]
fn observations_require_explicit_protocol_and_correlated_prompt_identity() {
    for value in [
        json!({}),
        json!({"promptId":"p"}),
        json!({"promptId":"p","a3sExecutionObservation":2}),
        json!({"promptId":"","a3sExecutionObservation":1}),
        json!({"promptId":"wrong id","a3sExecutionObservation":1}),
        json!({"promptId":null,"a3sExecutionObservation":1}),
    ] {
        assert!(Binding::from_meta(value.as_object()).is_none());
    }
    let value = json!({"promptId":"native-uuid","a3sExecutionObservation":1});
    assert_eq!(
        Binding::from_meta(value.as_object()).unwrap().prompt_id,
        "native-uuid"
    );
}
