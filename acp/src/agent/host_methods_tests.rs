use super::HostMethod;

#[test]
fn every_host_method_parses_back_from_its_name() {
    for method in HostMethod::ALL {
        let wire = match method {
            HostMethod::GoalCommand => "a3s/goal/state",
            HostMethod::Review => "a3s/review/target",
            other => other.as_str(),
        };
        assert_eq!(HostMethod::parse(wire), Some(method), "{wire}");
    }
}

#[test]
fn protocol_and_unknown_methods_are_outside_the_host_set() {
    for wire in [
        "session/prompt",
        "a3s/goal",
        "a3s/goal/unknown",
        "a3s/review",
        "x.ai/session",
        "x.ai/session/infos",
        "x.ai/permission_posture/set",
        "_meta/unknown",
        "",
    ] {
        assert_eq!(HostMethod::parse(wire), None, "{wire}");
    }
    assert_eq!(
        HostMethod::parse("x.ai/compact_conversation/start"),
        Some(HostMethod::CompactConversation)
    );
    assert_eq!(
        HostMethod::parse("x.ai/interject/cancel"),
        Some(HostMethod::Interject)
    );
}
