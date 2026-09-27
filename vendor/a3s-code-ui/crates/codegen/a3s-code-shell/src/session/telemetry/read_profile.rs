//! Ordinary GrokBuild read projection. Other tools do not receive this profile.

use std::sync::OnceLock;

use a3s_code_telemetry::events::{
    CanonicalToolId, CapApplicability, CapDisposition, InvocationSource, ReadFileRole,
    ReadLimitKind, ReadProfile, ReadSelection, ReadSkillMatch, ReadSkillSource, ToolOutputLimit,
    ToolSourceReason, ToolSourceStatus,
};
use a3s_code_tools::implementations::grok_build::ReadFileTool;
use a3s_code_tools::implementations::skills::types::SkillScope;
use a3s_code_tools::types::source_summary::{
    ReadDetail, ReadLimitSlot, ReadReason, ToolSourceResult, ToolSourceSummary,
};
use a3s_code_tools::types::tool::ToolNamespace;
use a3s_code_tools::types::tool_call_origin::ToolCallOrigin;
use a3s_tool_runtime::Tool;

use super::tool_call::SourceProjection;

pub(crate) fn model_origin(
    invocation_id: &str,
    session_id: &str,
    turn: Option<i64>,
    requested_model: Option<&str>,
    tool_id: &str,
    tool_version: Option<&str>,
) -> ToolCallOrigin {
    ToolCallOrigin::model_call(
        invocation_id,
        session_id,
        turn,
        requested_model.map(str::to_owned),
        tool_id,
        tool_version.map(str::to_owned),
    )
}

#[cfg(test)]
pub(crate) fn direct_origin(
    source: a3s_code_tools::types::tool_call_origin::InvocationSource,
    invocation_id: &str,
    session_id: Option<&str>,
    turn: Option<i64>,
    requested_model: Option<&str>,
    tool_id: &str,
    tool_version: Option<&str>,
) -> ToolCallOrigin {
    ToolCallOrigin::direct(
        source,
        invocation_id,
        session_id.map(str::to_owned),
        turn,
        requested_model.map(str::to_owned),
        tool_id,
        tool_version.map(str::to_owned),
    )
}

pub(crate) fn invocation_source(origin: Option<&ToolCallOrigin>) -> Option<InvocationSource> {
    origin.map(|origin| match origin.source() {
        a3s_code_tools::types::tool_call_origin::InvocationSource::Model => InvocationSource::Model,
        a3s_code_tools::types::tool_call_origin::InvocationSource::UserDirect => {
            InvocationSource::UserDirect
        }
        a3s_code_tools::types::tool_call_origin::InvocationSource::System => {
            InvocationSource::System
        }
    })
}

pub(crate) fn is_grok_build_read(tool_id: &CanonicalToolId) -> bool {
    tool_id.as_str() == grok_build_read_id()
}

pub(crate) fn read_projection(
    tool_id: &CanonicalToolId,
    summary: Option<&ToolSourceSummary>,
) -> (
    Option<SourceProjection>,
    Option<ToolOutputLimit>,
    Option<ReadProfile>,
) {
    if !is_grok_build_read(tool_id) {
        return (None, None, None);
    }
    let Some(summary) = summary.filter(|summary| summary.read().is_some()) else {
        return (
            None,
            Some(ToolOutputLimit::Unobserved),
            Some(unknown_profile()),
        );
    };
    (
        Some(source_from_summary(summary)),
        Some(map_output_limit(summary.output_limit)),
        summary.read().map(profile_from_detail),
    )
}

pub(crate) fn record_read_fields(
    span: &tracing::Span,
    invocation_source: Option<InvocationSource>,
    output_limit: Option<ToolOutputLimit>,
    read: Option<&ReadProfile>,
) {
    if let Some(source) = invocation_source {
        span.record("invocation_source", source.as_ref());
    }
    if let Some(limit) = output_limit {
        span.record("output_limit", limit.as_ref());
    }
    let Some(read) = read else {
        return;
    };
    span.record("read_file_role", read.read_file_role.as_ref());
    span.record("read_skill_match", read.read_skill_match.as_ref());
    if let Some(source) = read.read_skill_source {
        span.record("read_skill_source", source.as_ref());
    }
    span.record("read_selection", read.read_selection.as_ref());
    span.record("read_limit_kind", read.read_limit_kind.as_ref());
    span.record(
        "read_lines_applicability",
        read.read_lines_applicability.as_ref(),
    );
    span.record(
        "read_lines_disposition",
        read.read_lines_disposition.as_ref(),
    );
    record_i64(span, "read_lines_limit", read.read_lines_limit);
    record_i64(span, "read_lines_observed", read.read_lines_observed);
    span.record(
        "read_bytes_applicability",
        read.read_bytes_applicability.as_ref(),
    );
    span.record(
        "read_bytes_disposition",
        read.read_bytes_disposition.as_ref(),
    );
    record_i64(span, "read_bytes_limit", read.read_bytes_limit);
    record_i64(span, "read_bytes_observed", read.read_bytes_observed);
    span.record(
        "read_tokens_applicability",
        read.read_tokens_applicability.as_ref(),
    );
    span.record(
        "read_tokens_disposition",
        read.read_tokens_disposition.as_ref(),
    );
    record_i64(span, "read_tokens_limit", read.read_tokens_limit);
    record_i64(span, "read_tokens_observed", read.read_tokens_observed);
    record_i64(span, "read_source_bytes", read.read_source_bytes);
    record_i64(span, "read_returned_lines", read.read_returned_lines);
    record_i64(span, "read_returned_bytes", read.read_returned_bytes);
}

fn record_i64(span: &tracing::Span, key: &'static str, value: Option<i64>) {
    if let Some(value) = value {
        span.record(key, value);
    }
}

fn grok_build_read_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| format!("{}:{}", ToolNamespace::GrokBuild, ReadFileTool.id()))
        .as_str()
}

fn source_from_summary(summary: &ToolSourceSummary) -> SourceProjection {
    match &summary.result {
        ToolSourceResult::Unknown(_) => SourceProjection {
            status: ToolSourceStatus::Unknown,
            reason: Some(ToolSourceReason::NotInstrumented),
        },
        ToolSourceResult::Succeeded => SourceProjection {
            status: ToolSourceStatus::Succeeded,
            reason: None,
        },
        ToolSourceResult::Empty => SourceProjection {
            status: ToolSourceStatus::Empty,
            reason: None,
        },
        ToolSourceResult::Failed(reason) => SourceProjection {
            status: ToolSourceStatus::Failed,
            reason: reason.map(map_reason),
        },
    }
}

fn map_reason(reason: ReadReason) -> ToolSourceReason {
    match reason {
        ReadReason::NotFound => ToolSourceReason::ReadNotFound,
        ReadReason::Directory => ToolSourceReason::ReadDirectory,
        ReadReason::Denied => ToolSourceReason::ReadDenied,
        ReadReason::Ignored => ToolSourceReason::ReadIgnored,
        ReadReason::Binary => ToolSourceReason::ReadBinary,
        ReadReason::TokenLimit => ToolSourceReason::ReadTokenLimit,
        ReadReason::Io => ToolSourceReason::ReadIo,
    }
}

fn map_output_limit(
    limit: a3s_code_tools::types::source_summary::ToolOutputLimit,
) -> ToolOutputLimit {
    match limit {
        a3s_code_tools::types::source_summary::ToolOutputLimit::Unobserved => {
            ToolOutputLimit::Unobserved
        }
        a3s_code_tools::types::source_summary::ToolOutputLimit::NotLimited => {
            ToolOutputLimit::NotLimited
        }
        a3s_code_tools::types::source_summary::ToolOutputLimit::Limited => ToolOutputLimit::Limited,
    }
}

fn profile_from_detail(detail: &ReadDetail) -> ReadProfile {
    let lines = map_slot(&detail.lines);
    let bytes = map_slot(&detail.formatted_bytes);
    let tokens = map_slot(&detail.tokens);
    ReadProfile {
        read_file_role: map_role(detail.role),
        read_skill_match: map_match(detail.skill_match),
        read_skill_source: detail.skill_scope.and_then(map_scope),
        read_selection: map_selection(detail.selection),
        read_source_bytes: detail.source_bytes,
        read_returned_lines: detail.returned_lines,
        read_returned_bytes: detail.returned_bytes,
        read_limit_kind: map_kind(detail.limit_kind()),
        read_lines_applicability: lines.0,
        read_lines_limit: lines.1,
        read_lines_observed: lines.2,
        read_lines_disposition: lines.3,
        read_bytes_applicability: bytes.0,
        read_bytes_limit: bytes.1,
        read_bytes_observed: bytes.2,
        read_bytes_disposition: bytes.3,
        read_tokens_applicability: tokens.0,
        read_tokens_limit: tokens.1,
        read_tokens_observed: tokens.2,
        read_tokens_disposition: tokens.3,
    }
}

fn map_slot(slot: &ReadLimitSlot) -> (CapApplicability, Option<i64>, Option<i64>, CapDisposition) {
    (
        map_applicability(slot.applicability),
        slot.configured,
        slot.observed,
        map_disposition(slot.disposition),
    )
}

fn unknown_profile() -> ReadProfile {
    let slot = map_slot(&ReadLimitSlot::unknown());
    ReadProfile {
        read_file_role: ReadFileRole::Unknown,
        read_skill_match: ReadSkillMatch::Unknown,
        read_skill_source: None,
        read_selection: ReadSelection::Unknown,
        read_source_bytes: None,
        read_returned_lines: None,
        read_returned_bytes: None,
        read_limit_kind: ReadLimitKind::Unknown,
        read_lines_applicability: slot.0,
        read_lines_limit: slot.1,
        read_lines_observed: slot.2,
        read_lines_disposition: slot.3,
        read_bytes_applicability: slot.0,
        read_bytes_limit: slot.1,
        read_bytes_observed: slot.2,
        read_bytes_disposition: slot.3,
        read_tokens_applicability: slot.0,
        read_tokens_limit: slot.1,
        read_tokens_observed: slot.2,
        read_tokens_disposition: slot.3,
    }
}

fn map_role(role: a3s_code_tools::types::source_summary::ReadRole) -> ReadFileRole {
    use a3s_code_tools::types::source_summary::ReadRole;
    match role {
        ReadRole::SkillEntry => ReadFileRole::SkillEntry,
        ReadRole::SkillSupport => ReadFileRole::SkillSupport,
        ReadRole::Instruction => ReadFileRole::Instruction,
        ReadRole::Memory => ReadFileRole::Memory,
        ReadRole::Ordinary => ReadFileRole::Ordinary,
        ReadRole::Unknown => ReadFileRole::Unknown,
    }
}

fn map_match(skill_match: a3s_code_tools::types::source_summary::RegistryMatch) -> ReadSkillMatch {
    use a3s_code_tools::types::source_summary::RegistryMatch;
    match skill_match {
        RegistryMatch::Registered => ReadSkillMatch::Registered,
        RegistryMatch::Unregistered => ReadSkillMatch::Unregistered,
        RegistryMatch::Unknown => ReadSkillMatch::Unknown,
    }
}

fn map_scope(scope: SkillScope) -> Option<ReadSkillSource> {
    Some(match scope {
        SkillScope::Local => ReadSkillSource::Local,
        SkillScope::Repo => ReadSkillSource::Repo,
        SkillScope::User => ReadSkillSource::User,
        SkillScope::Server => ReadSkillSource::Server,
        SkillScope::Bundled => ReadSkillSource::Bundled,
        SkillScope::Plugin => ReadSkillSource::Plugin,
    })
}

fn map_selection(selection: a3s_code_tools::types::source_summary::ReadSelection) -> ReadSelection {
    use a3s_code_tools::types::source_summary::ReadSelection as SourceSelection;
    match selection {
        SourceSelection::Full => ReadSelection::Full,
        SourceSelection::ModelWindow => ReadSelection::ModelWindow,
        SourceSelection::DefaultWindow => ReadSelection::DefaultWindow,
        SourceSelection::SkillFullRead => ReadSelection::SkillFullRead,
        SourceSelection::Unknown => ReadSelection::Unknown,
    }
}

fn map_kind(kind: a3s_code_tools::types::source_summary::ReadLimitKind) -> ReadLimitKind {
    use a3s_code_tools::types::source_summary::ReadLimitKind as SourceKind;
    match kind {
        SourceKind::None => ReadLimitKind::None,
        SourceKind::Lines => ReadLimitKind::Lines,
        SourceKind::Bytes => ReadLimitKind::Bytes,
        SourceKind::Tokens => ReadLimitKind::Tokens,
        SourceKind::Multiple => ReadLimitKind::Multiple,
        SourceKind::Unknown => ReadLimitKind::Unknown,
    }
}

fn map_applicability(
    applicability: a3s_code_tools::types::source_summary::CapApplicability,
) -> CapApplicability {
    use a3s_code_tools::types::source_summary::CapApplicability as SourceApplicability;
    match applicability {
        SourceApplicability::Applies => CapApplicability::Applies,
        SourceApplicability::NotApplicable => CapApplicability::NotApplicable,
        SourceApplicability::Unknown => CapApplicability::Unknown,
    }
}

fn map_disposition(
    disposition: a3s_code_tools::types::source_summary::CapDisposition,
) -> CapDisposition {
    use a3s_code_tools::types::source_summary::CapDisposition as SourceDisposition;
    match disposition {
        SourceDisposition::Unobserved => CapDisposition::Unobserved,
        SourceDisposition::WithinLimit => CapDisposition::WithinLimit,
        SourceDisposition::Truncated => CapDisposition::Truncated,
        SourceDisposition::Rejected => CapDisposition::Rejected,
        SourceDisposition::Exempt => CapDisposition::Exempt,
    }
}

#[cfg(test)]
#[path = "read_profile_tests.rs"]
mod read_profile_tests;
