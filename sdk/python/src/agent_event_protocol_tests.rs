use super::*;
use a3s_code_core::EventEnvelopeV1;
use serde_json::json;

#[test]
fn sdk_event_preserves_unknown_type_payload_and_metadata() {
    let projection = RustAgentEventProjectionV1::from(
        EventEnvelopeV1::new("future_event", json!({ "opaque": [1, 2, 3] }))
            .with_metadata(json!({ "correlation_id": "future-1" })),
    );

    let event = PyAgentEvent::from_projection(projection);

    assert_eq!(event.version, 1);
    assert_eq!(event.event_type, "future_event");
    assert_eq!(event.payload_json, r#"{"opaque":[1,2,3]}"#);
    assert_eq!(
        event.metadata_json.as_deref(),
        Some(r#"{"correlation_id":"future-1"}"#)
    );
    assert_eq!(event.data, Some(event.payload_json.clone()));

    pyo3::prepare_freethreaded_python();
    Python::with_gil(|py| {
        let bound = Bound::new(py, event).expect("Python AgentEvent should allocate");
        assert_eq!(
            bound
                .getattr("type")
                .expect("canonical type getter should exist")
                .extract::<String>()
                .expect("type should be a string"),
            "future_event"
        );
        let payload = bound
            .getattr("payload")
            .expect("payload getter should exist");
        assert_eq!(
            payload
                .get_item("opaque")
                .expect("opaque payload field should exist")
                .extract::<Vec<u8>>()
                .expect("opaque payload should remain a list"),
            vec![1, 2, 3]
        );
    });
}

#[test]
fn sdk_catalog_is_the_core_catalog() {
    assert_eq!(agent_event_types_v1(), AGENT_EVENT_TYPES_V1);
    assert_eq!(event_envelope_v1_version(), 1);
}

#[test]
fn core_error_exposes_a_stable_python_code_attribute() {
    pyo3::prepare_freethreaded_python();
    let error = py_code_error(a3s_code_core::CodeError::SessionBusy {
        session_id: "session-1".to_string(),
    });
    Python::with_gil(|py| {
        assert_eq!(
            error
                .value(py)
                .getattr("code")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "SESSION_BUSY"
        );
        assert!(error.value(py).to_string().contains("session-1"));
    });
}

#[tokio::test]
async fn terminal_event_waits_for_stream_lifecycle() {
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    tx.send(RustAgentEvent::End {
        text: "done".into(),
        usage: a3s_code_core::TokenUsage::default(),
        verification_summary: Box::new(RustVerificationSummary::from_reports(&[])),
        meta: None,
    })
    .await
    .expect("terminal event should enter the test stream");
    drop(tx);

    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let lifecycle = tokio::spawn(async move {
        let _ = release_rx.await;
    });
    let receive = tokio::spawn(recv_stream_event(
        Arc::new(Mutex::new(rx)),
        Arc::new(Mutex::new(Some(lifecycle))),
    ));
    tokio::task::yield_now().await;
    assert!(
        !receive.is_finished(),
        "terminal event must not outrun the core stream lifecycle"
    );
    let _ = release_tx.send(());
    assert!(matches!(
        receive.await.expect("receive task should join"),
        Some(RustAgentEvent::End { .. })
    ));
}
