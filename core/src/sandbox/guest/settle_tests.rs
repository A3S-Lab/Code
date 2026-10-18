#![cfg(test)]

use super::{
    observe_json, settle, CommandSettlement, GuestObservation, HostStop, GUEST_COMMAND_SCHEMA,
};

#[test]
fn legacy_worker_status_cannot_be_accepted_as_payload_exit() {
    for exit_code in [0, 7, 127] {
        let legacy = serde_json::json!({
            "schema_version": "a3s.oci.guest-command.v1",
            "settlement": "settled",
            "exit_code": exit_code,
            "vm_entered": true,
        });
        assert_eq!(
            observe_json(HostStop::None, &serde_json::to_vec(&legacy).unwrap()),
            GuestObservation::MissingStatus,
        );
    }
}

#[test]
fn settlement_matches_the_observation_and_never_promotes_an_unknown_exit() {
    assert_eq!(
        settle(GuestObservation::ProbeUnavailable),
        CommandSettlement::Refused
    );
    assert_eq!(
        settle(GuestObservation::Exited(0)),
        CommandSettlement::Settled {
            exit_code: 0,
            timed_out: false
        }
    );
    assert_eq!(
        settle(GuestObservation::Exited(7)),
        CommandSettlement::Settled {
            exit_code: 7,
            timed_out: false
        }
    );
    assert_eq!(
        settle(GuestObservation::TimedOut),
        CommandSettlement::Settled {
            exit_code: 124,
            timed_out: true
        }
    );
    assert_eq!(
        settle(GuestObservation::Signaled),
        CommandSettlement::Indeterminate
    );
    assert_eq!(
        settle(GuestObservation::MissingStatus),
        CommandSettlement::Indeterminate
    );
}

#[test]
fn a_settled_report_without_vm_entry_is_not_success() {
    let entered = format!(
        r#"{{"schema_version":"{GUEST_COMMAND_SCHEMA}","settlement":"settled","exit_code":0,"vm_entered":true}}"#
    );
    assert_eq!(
        observe_json(HostStop::None, entered.as_bytes()),
        GuestObservation::Exited(0)
    );

    let not_entered = format!(
        r#"{{"schema_version":"{GUEST_COMMAND_SCHEMA}","settlement":"settled","exit_code":0,"vm_entered":false}}"#
    );
    assert_eq!(
        observe_json(HostStop::None, not_entered.as_bytes()),
        GuestObservation::MissingStatus
    );
    assert_eq!(
        settle(observe_json(HostStop::None, not_entered.as_bytes())),
        CommandSettlement::Indeterminate
    );

    let refused = format!(
        r#"{{"schema_version":"{GUEST_COMMAND_SCHEMA}","settlement":"refused","vm_entered":false}}"#
    );
    assert_eq!(
        observe_json(HostStop::None, refused.as_bytes()),
        GuestObservation::ProbeUnavailable
    );

    let shutdown = observe_json(HostStop::Shutdown, entered.as_bytes());
    assert_eq!(shutdown, GuestObservation::Signaled);
    assert_eq!(settle(shutdown), CommandSettlement::Indeterminate);

    assert_eq!(
        observe_json(HostStop::Timeout, entered.as_bytes()),
        GuestObservation::TimedOut
    );
    assert_eq!(
        observe_json(HostStop::None, b""),
        GuestObservation::MissingStatus
    );
    assert_eq!(
        observe_json(
            HostStop::None,
            br#"{"schema_version":"other","settlement":"settled","exit_code":0,"vm_entered":true}"#
        ),
        GuestObservation::MissingStatus
    );
}
