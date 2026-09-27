//! Per-attempt source facts. The slot emits nothing and holds no task.

use std::sync::Arc;

use crate::implementations::skills::types::SkillScope;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownReason {
    NotInstrumented,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadReason {
    NotFound,
    Directory,
    Denied,
    Ignored,
    Binary,
    TokenLimit,
    Io,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolSourceResult {
    Unknown(UnknownReason),
    Succeeded,
    Empty,
    Failed(Option<ReadReason>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolOutputLimit {
    Unobserved,
    NotLimited,
    Limited,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadRole {
    SkillEntry,
    SkillSupport,
    Instruction,
    Memory,
    Ordinary,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistryMatch {
    Registered,
    Unregistered,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadSelection {
    Full,
    ModelWindow,
    DefaultWindow,
    SkillFullRead,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapApplicability {
    Applies,
    NotApplicable,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapDisposition {
    Unobserved,
    WithinLimit,
    Truncated,
    Rejected,
    Exempt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadLimitKind {
    None,
    Lines,
    Bytes,
    Tokens,
    Multiple,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadLimitSlot {
    pub applicability: CapApplicability,
    pub configured: Option<i64>,
    pub observed: Option<i64>,
    pub disposition: CapDisposition,
}

impl ReadLimitSlot {
    pub fn unknown() -> Self {
        Self {
            applicability: CapApplicability::Unknown,
            configured: None,
            observed: None,
            disposition: CapDisposition::Unobserved,
        }
    }

    pub fn is_hit(&self) -> bool {
        matches!(
            self.disposition,
            CapDisposition::Truncated | CapDisposition::Rejected
        )
    }

    fn is_unevaluated(&self) -> bool {
        self.applicability == CapApplicability::Applies
            && self.disposition == CapDisposition::Unobserved
    }

    fn is_unknown_unobserved(&self) -> bool {
        self.applicability == CapApplicability::Unknown
            && self.disposition == CapDisposition::Unobserved
    }

    fn has_evaluated_disposition(&self) -> bool {
        self.disposition != CapDisposition::Unobserved
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadDetail {
    pub role: ReadRole,
    pub skill_match: RegistryMatch,
    pub skill_scope: Option<SkillScope>,
    pub selection: ReadSelection,
    pub source_bytes: Option<i64>,
    pub returned_lines: Option<i64>,
    pub returned_bytes: Option<i64>,
    pub lines: ReadLimitSlot,
    pub formatted_bytes: ReadLimitSlot,
    pub tokens: ReadLimitSlot,
}

impl ReadDetail {
    pub fn unknown() -> Self {
        Self {
            role: ReadRole::Unknown,
            skill_match: RegistryMatch::Unknown,
            skill_scope: None,
            selection: ReadSelection::Unknown,
            source_bytes: None,
            returned_lines: None,
            returned_bytes: None,
            lines: ReadLimitSlot::unknown(),
            formatted_bytes: ReadLimitSlot::unknown(),
            tokens: ReadLimitSlot::unknown(),
        }
    }

    /// `multiple` when two caps hit. One hit plus an unevaluated applicable cap stays unknown.
    /// Unknown and unobserved, with no evaluated disposition, is unknown rather than none.
    pub fn limit_kind(&self) -> ReadLimitKind {
        let mut hits = 0usize;
        let mut only = None;
        for (hit, kind) in [
            (self.lines.is_hit(), ReadLimitKind::Lines),
            (self.formatted_bytes.is_hit(), ReadLimitKind::Bytes),
            (self.tokens.is_hit(), ReadLimitKind::Tokens),
        ] {
            if hit {
                hits += 1;
                only = Some(kind);
            }
        }
        let unevaluated = self.lines.is_unevaluated()
            || self.formatted_bytes.is_unevaluated()
            || self.tokens.is_unevaluated();
        let unknown_unobserved = self.lines.is_unknown_unobserved()
            || self.formatted_bytes.is_unknown_unobserved()
            || self.tokens.is_unknown_unobserved();
        let evaluated = self.lines.has_evaluated_disposition()
            || self.formatted_bytes.has_evaluated_disposition()
            || self.tokens.has_evaluated_disposition();
        match hits {
            0 if unevaluated || (unknown_unobserved && !evaluated) => ReadLimitKind::Unknown,
            0 => ReadLimitKind::None,
            1 if unevaluated => ReadLimitKind::Unknown,
            1 => only.unwrap_or(ReadLimitKind::Unknown),
            _ => ReadLimitKind::Multiple,
        }
    }

    pub fn output_limit(&self) -> ToolOutputLimit {
        match self.limit_kind() {
            ReadLimitKind::None => ToolOutputLimit::NotLimited,
            ReadLimitKind::Unknown => ToolOutputLimit::Unobserved,
            ReadLimitKind::Lines
            | ReadLimitKind::Bytes
            | ReadLimitKind::Tokens
            | ReadLimitKind::Multiple => ToolOutputLimit::Limited,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolSourceDetail {
    None,
    Read(ReadDetail),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolSourceSummary {
    pub result: ToolSourceResult,
    pub output_limit: ToolOutputLimit,
    pub detail: ToolSourceDetail,
}

impl ToolSourceSummary {
    pub fn uninstrumented() -> Self {
        Self {
            result: ToolSourceResult::Unknown(UnknownReason::NotInstrumented),
            output_limit: ToolOutputLimit::Unobserved,
            detail: ToolSourceDetail::None,
        }
    }

    pub fn read(&self) -> Option<&ReadDetail> {
        match &self.detail {
            ToolSourceDetail::Read(detail) => Some(detail),
            ToolSourceDetail::None => None,
        }
    }
}

/// Cloneable attempt-local handle. `record` replaces the value and emits nothing.
#[derive(Clone, Debug)]
pub struct SourceSummarySlot {
    inner: Arc<parking_lot::Mutex<ToolSourceSummary>>,
}

impl SourceSummarySlot {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(parking_lot::Mutex::new(ToolSourceSummary::uninstrumented())),
        }
    }

    pub fn record(&self, summary: ToolSourceSummary) {
        *self.inner.lock() = summary;
    }

    pub fn snapshot(&self) -> ToolSourceSummary {
        self.inner.lock().clone()
    }
}

impl Default for SourceSummarySlot {
    fn default() -> Self {
        Self::new()
    }
}

/// Copies the attempt slot and origin. Does not copy host resources or inner dispatch.
pub fn copy_call_facts(
    from: &a3s_tool_runtime::ToolCallContext,
    to: &mut a3s_tool_runtime::ToolCallContext,
) {
    if let Some(slot) = from.get::<SourceSummarySlot>() {
        to.extensions.insert((*slot).clone());
    }
    if let Some(origin) = from.get::<crate::types::tool_call_origin::ToolCallOrigin>() {
        to.extensions.insert((*origin).clone());
    }
}

pub fn as_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
