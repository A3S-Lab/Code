#![cfg(test)]

use super::*;

#[test]
fn acknowledges_and_replays_only_in_memory_blobs() {
    let mut store = Store::default();
    let set = store
        .reply(
            br#"{"kvServerMessage":{"id":7,"setBlobArgs":{"blobId":"a","blobData":"YmxvYg=="}}}"#,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&set).unwrap(),
        json!({"kvClientMessage":{"id":7,"setBlobResult":{}}})
    );
    let get = store
        .reply(br#"{"kvServerMessage":{"id":8,"getBlobArgs":{"blobId":"a"}}}"#)
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&get).unwrap(),
        json!({"kvClientMessage":{"id":8,"getBlobResult":{"blobData":"YmxvYg=="}}})
    );
    assert!(Store::default()
        .reply(br#"{"kvServerMessage":{"id":8,"getBlobArgs":{"blobId":"a"}}}"#)
        .is_err());
}

#[test]
fn omitted_protobuf_request_id_means_zero() {
    let reply = Store::default()
        .reply(br#"{"kvServerMessage":{"setBlobArgs":{"blobId":"a","blobData":"YQ=="}}}"#)
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&reply).unwrap(),
        json!({"kvClientMessage":{"id":0,"setBlobResult":{}}})
    );
}

#[test]
fn refuses_unknown_ambiguous_unbounded_or_malformed_requests() {
    let mut store = Store::default();
    for request in [
        json!({"id":"secret"}),
        json!({"id":u64::MAX,"getBlobArgs":{}}),
        json!({"id":1,"setBlobArgs":{},"getBlobArgs":{}}),
        json!({"id":1,"setBlobArgs":{"blobId":"a","blobData":"x".repeat(MAX_BLOB_BYTES)}}),
    ] {
        assert!(store
            .reply(&serde_json::to_vec(&json!({"kvServerMessage":request})).unwrap())
            .is_err());
    }
    assert!(store
        .reply(br#"{"execServerMessage":{"shell":{"command":"do not execute"}}}"#)
        .unwrap()
        .is_none());
}
