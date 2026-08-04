use a3s_code_core::{
    AgentEvent, AgentProtocolCommandActionV1, AgentProtocolCommandReceiptV1,
    AgentProtocolCommandV1, AgentProtocolEventPageV1, AgentProtocolEventRecordV1,
    AgentProtocolRunCancelV1, AgentProtocolRunIdentityV1, AgentProtocolRunRecoverV1,
    AgentProtocolRunStartV1, AgentProtocolRunStateV1, EventEnvelopeV1, InMemoryRunStore,
    AGENT_PROTOCOL_V1,
};
use serde_json::json;

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn identity(run_id: &str) -> AgentProtocolRunIdentityV1 {
    AgentProtocolRunIdentityV1 {
        schema: AgentProtocolRunIdentityV1::SCHEMA.into(),
        protocol: AGENT_PROTOCOL_V1.into(),
        agent_release_identity: digest('a'),
        session_id: "conversation-018f4f86".into(),
        run_id: run_id.into(),
    }
}

fn start_command() -> AgentProtocolCommandV1 {
    AgentProtocolCommandV1::Start {
        request: AgentProtocolRunStartV1 {
            schema: AgentProtocolRunStartV1::SCHEMA.into(),
            request_id: "execution-018f4f86:start".into(),
            identity: identity("run-execution-018f4f86-attempt-1"),
            prompt: "Explain the failing test and fix it.".into(),
        },
    }
}

fn event_record(
    identity: &AgentProtocolRunIdentityV1,
    sequence: u64,
    timestamp_ms: u64,
    event_type: &str,
    payload: serde_json::Value,
) -> AgentProtocolEventRecordV1 {
    AgentProtocolEventRecordV1 {
        sequence,
        occurred_at_ms: timestamp_ms,
        event: EventEnvelopeV1::new(event_type, payload).with_metadata(json!({
            "run_id": identity.run_id,
            "session_id": identity.session_id,
            "sequence": sequence,
            "timestamp_ms": timestamp_ms,
        })),
    }
}

#[test]
fn start_command_uses_the_declared_a3s_code_agent_protocol() {
    let command = start_command();
    command.validate().expect("valid start command");

    assert_eq!(command.action(), AgentProtocolCommandActionV1::Start);
    assert_eq!(command.identity().protocol, AGENT_PROTOCOL_V1);
    assert!(command
        .digest()
        .expect("command digest")
        .starts_with("sha256:"));

    let encoded = serde_json::to_value(&command).expect("encode command");
    assert_eq!(encoded["action"], "start");
    assert_eq!(
        encoded["request"]["identity"]["protocol"],
        AGENT_PROTOCOL_V1
    );
    let decoded: AgentProtocolCommandV1 = serde_json::from_value(encoded).expect("decode command");
    assert_eq!(decoded, command);
}

#[test]
fn command_digest_binds_the_exact_code_input() {
    let command = start_command();
    let original = command.digest().expect("original digest");
    let mut changed = command;
    let AgentProtocolCommandV1::Start { request } = &mut changed else {
        panic!("expected start command");
    };
    request.prompt.push_str(" Do not run tests.");

    assert_ne!(changed.digest().expect("changed digest"), original);
}

#[test]
fn cancellation_and_recovery_reuse_code_session_and_run_identity() {
    let current = identity("run-execution-018f4f86-attempt-1");
    let cancel = AgentProtocolCommandV1::Cancel {
        request: AgentProtocolRunCancelV1 {
            schema: AgentProtocolRunCancelV1::SCHEMA.into(),
            request_id: "execution-018f4f86:cancel:1".into(),
            identity: current.clone(),
            reason: "user_requested".into(),
        },
    };
    cancel.validate().expect("valid cancellation");
    assert_eq!(cancel.action(), AgentProtocolCommandActionV1::Cancel);

    let recover = AgentProtocolCommandV1::Recover {
        request: AgentProtocolRunRecoverV1 {
            schema: AgentProtocolRunRecoverV1::SCHEMA.into(),
            request_id: "execution-018f4f86:recover:2".into(),
            identity: identity("run-execution-018f4f86-attempt-2"),
            checkpoint_run_id: current.run_id,
        },
    };
    recover.validate().expect("valid recovery");
    assert_eq!(recover.action(), AgentProtocolCommandActionV1::Recover);
}

#[test]
fn event_pages_carry_code_event_envelopes_without_a_second_event_model() {
    let identity = identity("run-execution-018f4f86-attempt-1");
    let page = AgentProtocolEventPageV1 {
        schema: AgentProtocolEventPageV1::SCHEMA.into(),
        identity: identity.clone(),
        after_event_sequence: None,
        first_available_sequence: Some(0),
        latest_sequence_exclusive: 2,
        next_after_event_sequence: Some(1),
        state: AgentProtocolRunStateV1::Executing,
        observed_at_ms: 1_723_000_000_002,
        retention_gap: false,
        has_more: false,
        events: vec![
            event_record(
                &identity,
                0,
                1_723_000_000_000,
                "agent_start",
                json!({"prompt": "Explain the failure"}),
            ),
            event_record(
                &identity,
                1,
                1_723_000_000_001,
                "future_code_event",
                json!({"opaque": [1, 2, 3]}),
            ),
        ],
    };

    page.validate().expect("valid Code event page");
    assert_eq!(page.first_sequence(), Some(0));
    assert_eq!(page.last_sequence(), Some(1));
    assert_eq!(page.events[1].event.event_type, "future_code_event");
    assert!(page.digest().expect("page digest").starts_with("sha256:"));

    let mut gap = page.clone();
    gap.events[1].sequence = 3;
    assert!(gap.validate().is_err());

    let mut mismatched_metadata = page;
    mismatched_metadata.events[0].event.metadata = Some(json!({
        "run_id": "another-run",
        "session_id": identity.session_id,
        "sequence": 0,
        "timestamp_ms": 1_723_000_000_000_u64,
    }));
    assert!(mismatched_metadata.validate().is_err());
}

#[tokio::test]
async fn event_pages_project_the_authoritative_code_run_store() {
    let identity = identity("run-execution-018f4f86-attempt-1");
    let store = InMemoryRunStore::new();
    store
        .create_run_with_id(
            identity.run_id.clone(),
            &identity.session_id,
            "Explain the failure",
        )
        .await;
    store
        .record_event(
            &identity.run_id,
            AgentEvent::Start {
                prompt: "Explain the failure".into(),
            },
        )
        .await;
    store
        .record_event(
            &identity.run_id,
            AgentEvent::TextDelta {
                text: "The assertion is stale.".into(),
            },
        )
        .await;

    let page = store
        .event_page(&identity.run_id, None, 64)
        .await
        .expect("known run page");
    let snapshot = store
        .snapshot(&identity.run_id)
        .await
        .expect("known run snapshot");
    let projected = AgentProtocolEventPageV1::from_run_page(
        identity.clone(),
        snapshot.status,
        page.events.last().unwrap().timestamp_ms,
        None,
        &page,
    )
    .expect("projected protocol page");

    assert_eq!(projected.identity, identity);
    assert_eq!(projected.state, AgentProtocolRunStateV1::Executing);
    assert_eq!(projected.first_sequence(), Some(0));
    assert_eq!(projected.last_sequence(), Some(1));
    assert_eq!(projected.events[0].event.event_type, "agent_start");
    assert_eq!(projected.events[1].event.event_type, "text_delta");
    assert_eq!(
        projected.events[1].event.payload["text"],
        "The assertion is stale."
    );
}

#[test]
fn command_receipts_settle_only_the_exact_code_command() {
    let command = start_command();
    let receipt = AgentProtocolCommandReceiptV1 {
        schema: AgentProtocolCommandReceiptV1::SCHEMA.into(),
        action: AgentProtocolCommandActionV1::Start,
        request_id: command.request_id().into(),
        identity: command.identity().clone(),
        command_digest: command.digest().expect("command digest"),
        state: AgentProtocolRunStateV1::Executing,
        latest_event_sequence_exclusive: 0,
        observed_at_ms: 1_723_000_000_000,
        replayed: false,
    };
    receipt.validate_for(&command).expect("matching receipt");

    let mut wrong = receipt;
    wrong.identity.run_id = "run-other".into();
    assert!(wrong.validate_for(&command).is_err());
}

#[test]
fn the_protocol_is_closed_bounded_and_send_sync() {
    let unknown = json!({
        "action": "cancel",
        "request": {
            "schema": AgentProtocolRunCancelV1::SCHEMA,
            "request_id": "request-1",
            "identity": serde_json::to_value(identity("run-1")).unwrap(),
            "reason": "user_requested",
            "shell": "do not admit me"
        }
    });
    assert!(serde_json::from_value::<AgentProtocolCommandV1>(unknown).is_err());

    let mut oversized = start_command();
    let AgentProtocolCommandV1::Start { request } = &mut oversized else {
        panic!("expected start command");
    };
    request.prompt = "x".repeat(64 * 1024 + 1);
    assert!(oversized.validate().is_err());

    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AgentProtocolCommandV1>();
    assert_send_sync::<AgentProtocolCommandReceiptV1>();
    assert_send_sync::<AgentProtocolEventPageV1>();
}
