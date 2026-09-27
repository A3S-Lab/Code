use crate::types::source_summary::{
    CapApplicability, CapDisposition, ReadDetail, ReadLimitKind, ReadLimitSlot, ReadReason,
    SourceSummarySlot, ToolOutputLimit, ToolSourceDetail, ToolSourceResult, ToolSourceSummary,
    copy_call_facts,
};
use crate::types::tool_call_origin::{InvocationSource, ToolCallOrigin};

#[test]
fn copied_slot_sees_a_later_record() {
    let slot = SourceSummarySlot::new();
    let mut parent = a3s_tool_runtime::ToolCallContext::default();
    parent.insert(slot.clone());
    let mut child = a3s_tool_runtime::ToolCallContext::default();
    copy_call_facts(&parent, &mut child);
    let mut detail = ReadDetail::unknown();
    detail.tokens = ReadLimitSlot {
        applicability: CapApplicability::Applies,
        configured: Some(25_000),
        observed: None,
        disposition: CapDisposition::Rejected,
    };
    detail.lines.disposition = CapDisposition::Truncated;
    detail.lines.applicability = CapApplicability::Applies;
    detail.formatted_bytes.disposition = CapDisposition::Truncated;
    detail.formatted_bytes.applicability = CapApplicability::Applies;
    slot.record(ToolSourceSummary {
        result: ToolSourceResult::Failed(Some(ReadReason::TokenLimit)),
        output_limit: detail.output_limit(),
        detail: ToolSourceDetail::Read(detail),
    });
    let copied = child.get::<SourceSummarySlot>().expect("slot copied");
    let summary = copied.snapshot();
    let read = summary.read().expect("read detail");
    assert_eq!(read.limit_kind(), ReadLimitKind::Multiple);
    assert!(read.tokens.observed.is_none());
}

#[test]
fn unknown_unobserved_without_an_evaluated_disposition_is_unknown() {
    let unread = ReadDetail::unknown();
    assert_eq!(unread.limit_kind(), ReadLimitKind::Unknown);
    assert_eq!(unread.output_limit(), ToolOutputLimit::Unobserved);

    let mut ordinary = ReadDetail::unknown();
    ordinary.formatted_bytes.applicability = CapApplicability::NotApplicable;
    assert_eq!(ordinary.limit_kind(), ReadLimitKind::Unknown);

    let mut skill = ReadDetail::unknown();
    skill.lines.applicability = CapApplicability::Applies;
    skill.tokens.applicability = CapApplicability::Applies;
    skill.formatted_bytes.applicability = CapApplicability::NotApplicable;
    assert_eq!(skill.limit_kind(), ReadLimitKind::Unknown);

    let mut empty = ReadDetail::unknown();
    empty.lines.applicability = CapApplicability::Applies;
    empty.lines.disposition = CapDisposition::WithinLimit;
    empty.lines.observed = Some(0);
    assert_eq!(empty.limit_kind(), ReadLimitKind::None);
    assert_eq!(empty.output_limit(), ToolOutputLimit::NotLimited);

    let mut media = ReadDetail::unknown();
    for slot in [
        &mut media.lines,
        &mut media.formatted_bytes,
        &mut media.tokens,
    ] {
        slot.applicability = CapApplicability::NotApplicable;
    }
    assert_eq!(media.limit_kind(), ReadLimitKind::None);
    assert_eq!(media.output_limit(), ToolOutputLimit::NotLimited);

    let mut refused = ReadDetail::unknown();
    refused.tokens = ReadLimitSlot {
        applicability: CapApplicability::Applies,
        configured: Some(25_000),
        observed: None,
        disposition: CapDisposition::Rejected,
    };
    assert_eq!(refused.limit_kind(), ReadLimitKind::Tokens);
}

#[test]
fn direct_origin_omits_the_model_unless_the_caller_knows_one() {
    let absent = ToolCallOrigin::direct(
        InvocationSource::System,
        "call-1",
        None,
        None,
        None,
        "opaque",
        None,
    );
    assert!(absent.requested_model().is_none());
    assert_eq!(absent.source(), InvocationSource::System);
    let known = ToolCallOrigin::direct(
        InvocationSource::UserDirect,
        "call-2",
        None,
        None,
        None,
        "opaque",
        None,
    )
    .with_known_model("grok-4.6");
    assert_eq!(known.requested_model(), Some("grok-4.6"));
    let blank =
        ToolCallOrigin::model_call("call-3", "session", None, Some(String::new()), "id", None);
    assert!(blank.requested_model().is_none());
}
