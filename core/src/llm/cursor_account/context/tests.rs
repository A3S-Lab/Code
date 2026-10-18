#![cfg(test)]

use super::*;

#[test]
fn context_requests_receive_only_the_static_empty_host_context() {
    let request = br#"{"execServerMessage":{"execId":"context-1","requestContextArgs":{"workspaceId":"secret-workspace"}}}"#;
    let response = reply(request).unwrap().unwrap();
    let value: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(value["execClientMessage"]["id"], 0);
    assert_eq!(value["execClientMessage"]["execId"], "context-1");
    assert_eq!(
        value["execClientMessage"]["requestContextResult"]["success"]["requestContext"],
        empty()
    );
    assert!(!String::from_utf8(response)
        .unwrap()
        .contains("secret-workspace"));
}

#[test]
fn context_replies_do_not_admit_execution_or_malformed_requests() {
    let oversized =
        json!({"execServerMessage":{"execId":"x".repeat(1025),"requestContextArgs":{}}});
    assert!(reply(&serde_json::to_vec(&oversized).unwrap()).is_err());
    assert!(
        reply(br#"{"execServerMessage":{"shellArgs":{"command":"never execute"}}}"#)
            .unwrap()
            .is_none()
    );
    for payload in [
        br#"{"execServerMessage":{"requestContextArgs":{},"shellArgs":{}}}"#.as_slice(),
        br#"{"execServerMessage":{"id":"secret","requestContextArgs":{}}}"#,
        br#"{"execServerMessage":{"requestContextArgs":null}}"#,
    ] {
        assert!(reply(payload).is_err());
    }
}
