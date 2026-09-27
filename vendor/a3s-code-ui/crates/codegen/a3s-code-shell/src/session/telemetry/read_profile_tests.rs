use super::{direct_origin, read_projection};
use a3s_code_telemetry::events::{ReadLimitKind, ReadSkillMatch};
use a3s_code_tools::implementations::grok_build::ReadFileTool;
use a3s_code_tools::types::source_summary::{
    CapApplicability, CapDisposition, ReadDetail, ReadLimitSlot, ReadRole, ToolSourceDetail,
    ToolSourceResult, ToolSourceSummary,
};
use a3s_code_tools::types::tool::ToolNamespace;
use a3s_code_tools::types::tool_call_origin::InvocationSource;
use a3s_tool_runtime::Tool;

#[test]
fn system_origin_leaves_the_model_absent_unless_it_is_known() {
    let absent = direct_origin(
        InvocationSource::System,
        "call-1",
        None,
        None,
        None,
        "opaque",
        None,
    );
    assert!(absent.requested_model().is_none());
    let known = direct_origin(
        InvocationSource::UserDirect,
        "call-2",
        Some("session"),
        Some(4),
        Some("grok-4.6"),
        "GrokBuild:read_file",
        None,
    );
    assert_eq!(known.requested_model(), Some("grok-4.6"));
    assert_eq!(known.source(), InvocationSource::UserDirect);
}

#[test]
fn two_caps_project_to_multiple() {
    let mut detail = ReadDetail::unknown();
    detail.role = ReadRole::Ordinary;
    detail.skill_match = a3s_code_tools::types::source_summary::RegistryMatch::Unknown;
    detail.lines = ReadLimitSlot {
        applicability: CapApplicability::Applies,
        configured: Some(2),
        observed: Some(6),
        disposition: CapDisposition::Truncated,
    };
    detail.formatted_bytes = ReadLimitSlot {
        applicability: CapApplicability::Applies,
        configured: Some(15),
        observed: Some(40),
        disposition: CapDisposition::Truncated,
    };
    detail.tokens = ReadLimitSlot {
        applicability: CapApplicability::Applies,
        configured: Some(25_000),
        observed: None,
        disposition: CapDisposition::WithinLimit,
    };
    let summary = ToolSourceSummary {
        result: ToolSourceResult::Succeeded,
        output_limit: detail.output_limit(),
        detail: ToolSourceDetail::Read(detail),
    };
    let tool_id = a3s_code_telemetry::events::CanonicalToolId::from_qualified(&format!(
        "{}:{}",
        ToolNamespace::GrokBuild,
        ReadFileTool.id()
    ))
    .expect("registered read id");
    let (_, _, read) = read_projection(&tool_id, Some(&summary));
    let read = read.expect("read profile");
    assert_eq!(read.read_limit_kind, ReadLimitKind::Multiple);
    assert_eq!(read.read_skill_match, ReadSkillMatch::Unknown);
    assert!(read.read_tokens_observed.is_none());
}
