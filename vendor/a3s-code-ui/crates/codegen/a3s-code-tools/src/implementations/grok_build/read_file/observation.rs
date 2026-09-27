//! Ordinary GrokBuild read facts, recorded beside the typed output.

use std::path::Path;

use crate::implementations::grok_build::read_file::{
    READ_FILE_MAX_TOKENS, is_instruction_markdown, is_skill_markdown, max_output_bytes,
};
use crate::implementations::skills::types::SkillScope;
use crate::types::resources::SharedResources;
use crate::types::skill_discovery_tracker::SkillManager;
use crate::types::source_summary::{
    CapApplicability, CapDisposition, ReadDetail, ReadLimitSlot, ReadReason, ReadRole,
    ReadSelection, RegistryMatch, SourceSummarySlot, ToolOutputLimit, ToolSourceDetail,
    ToolSourceResult, ToolSourceSummary, UnknownReason, as_i64,
};

pub(super) struct ReadObservation {
    slot: Option<SourceSummarySlot>,
    summary: ToolSourceSummary,
    max_lines: Option<i64>,
}

impl ReadObservation {
    pub(super) fn start(slot: Option<&SourceSummarySlot>) -> Self {
        Self {
            slot: slot.cloned(),
            summary: ToolSourceSummary {
                result: ToolSourceResult::Unknown(UnknownReason::NotInstrumented),
                output_limit: ToolOutputLimit::Unobserved,
                detail: ToolSourceDetail::Read(ReadDetail::unknown()),
            },
            max_lines: None,
        }
    }

    pub(super) async fn classify_before_io(
        &mut self,
        resources: &SharedResources,
        path: &Path,
        max_lines: usize,
    ) {
        if self.slot.is_none() {
            return;
        }
        let max_bytes = max_output_bytes(resources).await;
        let (has_manager, scope) = {
            let resources = resources.lock().await;
            match resources.get::<SkillManager>() {
                Some(manager) => (true, manager.registered_scope(path)),
                None => (false, None),
            }
        };
        self.apply_classification(path, has_manager, scope, max_lines, max_bytes);
    }

    pub(super) fn note_memory(&mut self, in_memory: bool) {
        let Some(detail) = self.detail_mut() else {
            return;
        };
        if in_memory && matches!(detail.role, ReadRole::Ordinary | ReadRole::Unknown) {
            detail.role = ReadRole::Memory;
        }
    }

    pub(super) fn fail(&mut self, reason: Option<ReadReason>) {
        self.summary.result = ToolSourceResult::Failed(reason);
        self.finish();
    }

    pub(super) fn note_ignored(&mut self) {
        self.fail(Some(ReadReason::Ignored));
    }

    pub(super) fn note_not_found(&mut self) {
        self.fail(Some(ReadReason::NotFound));
    }

    pub(super) fn note_directory(&mut self) {
        self.fail(Some(ReadReason::Directory));
    }

    pub(super) fn note_denied(&mut self) {
        self.fail(Some(ReadReason::Denied));
    }

    pub(super) fn note_binary(&mut self) {
        self.fail(Some(ReadReason::Binary));
    }

    pub(super) fn note_io(&mut self) {
        self.fail(Some(ReadReason::Io));
    }

    pub(super) fn note_untyped_failure(&mut self) {
        self.fail(None);
    }

    // `source_bytes` is already the same buffer, recorded before the media branch.
    pub(super) fn note_media_success(&mut self) {
        self.mark_text_caps_not_applicable();
        if let Some(detail) = self.detail_mut() {
            detail.selection = ReadSelection::Unknown;
        }
        self.summary.result = ToolSourceResult::Succeeded;
        self.finish();
    }

    pub(super) fn note_early_content(&mut self, content: &crate::types::output::FileContent) {
        self.note_returned(content);
        self.summary.result = if content.content.is_empty() {
            ToolSourceResult::Empty
        } else {
            ToolSourceResult::Succeeded
        };
        self.finish();
    }

    /// Empty file returns before the token check. Applicable caps are within limit at zero.
    pub(super) fn note_empty_file(&mut self, offset: Option<i64>, limit: Option<usize>) {
        let configured_lines = self.max_lines;
        if let Some(detail) = self.detail_mut() {
            detail.selection = if offset.is_some() || limit.is_some() {
                ReadSelection::ModelWindow
            } else {
                ReadSelection::Full
            };
            detail.returned_lines = Some(0);
            detail.returned_bytes = Some(0);
            detail.lines.disposition = CapDisposition::WithinLimit;
            detail.lines.observed = Some(0);
            detail.lines.applicability = CapApplicability::Applies;
            if let Some(max_lines) = configured_lines {
                detail.lines.configured = Some(max_lines);
            }
            settle_empty_applicable(&mut detail.formatted_bytes);
            settle_empty_applicable(&mut detail.tokens);
        }
        self.summary.result = ToolSourceResult::Empty;
        self.finish();
    }

    pub(super) fn note_typed_output(&mut self, output: &crate::types::output::ReadFileOutput) {
        use crate::types::output::ReadFileOutput;
        match output {
            ReadFileOutput::FileContent(content) => self.note_early_content(content),
            ReadFileOutput::PdfPageImages(_) | ReadFileOutput::ImageContent(_) => {
                self.note_media_success()
            }
            ReadFileOutput::FileNotFound(_) => self.note_not_found(),
            ReadFileOutput::IsADirectory(_) => self.note_directory(),
            ReadFileOutput::PermissionDenied(_) => self.note_denied(),
            ReadFileOutput::FileTooLarge(_) => self.reject_tokens(),
            ReadFileOutput::FileReadError(_) | ReadFileOutput::ImageSizeError(_) => {
                self.note_untyped_failure()
            }
        }
    }

    pub(super) fn note_whole_read(&mut self, is_skill: bool) {
        if let Some(detail) = self.detail_mut() {
            detail.selection = if is_skill {
                ReadSelection::SkillFullRead
            } else {
                ReadSelection::Full
            };
            detail.lines.applicability = CapApplicability::Applies;
            detail.lines.disposition = CapDisposition::Exempt;
            if detail.formatted_bytes.applicability == CapApplicability::Applies {
                detail.formatted_bytes.disposition = CapDisposition::Exempt;
            }
        }
        self.accept_tokens();
    }

    pub(super) fn note_window(
        &mut self,
        model_requested: bool,
        model_limit: Option<usize>,
        remaining_lines: usize,
        max_lines: usize,
    ) {
        let applied = model_limit.unwrap_or(usize::MAX).min(max_lines);
        let clipped = applied == max_lines && remaining_lines > max_lines;
        if let Some(detail) = self.detail_mut() {
            detail.selection = if model_requested {
                ReadSelection::ModelWindow
            } else {
                ReadSelection::DefaultWindow
            };
            detail.lines.applicability = CapApplicability::Applies;
            detail.lines.configured = Some(as_i64(max_lines));
            detail.lines.observed = Some(as_i64(remaining_lines));
            detail.lines.disposition = if clipped {
                CapDisposition::Truncated
            } else {
                CapDisposition::WithinLimit
            };
        }
    }

    pub(super) fn note_byte_budget(&mut self, budget: usize, formatted_len: usize) {
        if let Some(detail) = self.detail_mut() {
            detail.formatted_bytes.applicability = CapApplicability::Applies;
            detail.formatted_bytes.configured = Some(as_i64(budget));
            detail.formatted_bytes.observed = Some(as_i64(formatted_len));
            detail.formatted_bytes.disposition = if formatted_len > budget {
                CapDisposition::Truncated
            } else {
                CapDisposition::WithinLimit
            };
        }
    }

    /// The typed `FileTooLarge` variant is the token refusal. It does not carry the estimate.
    pub(super) fn reject_tokens(&mut self) {
        self.settle_tokens(CapDisposition::Rejected);
        self.summary.result = ToolSourceResult::Failed(Some(ReadReason::TokenLimit));
        self.finish();
    }

    /// Missed the whole-read token cap, so this window counts as truncated.
    pub(super) fn note_token_fallback(&mut self) {
        self.settle_tokens(CapDisposition::Truncated);
    }

    pub(super) fn accept_tokens(&mut self) {
        // A fallback window or refusal already settled this slot.
        let already_hit = self
            .detail_mut()
            .is_some_and(|detail| detail.tokens.is_hit());
        if already_hit {
            return;
        }
        self.settle_tokens(CapDisposition::WithinLimit);
    }

    fn settle_tokens(&mut self, disposition: CapDisposition) {
        let Some(detail) = self.detail_mut() else {
            return;
        };
        detail.tokens.applicability = CapApplicability::Applies;
        detail.tokens.configured = Some(as_i64(READ_FILE_MAX_TOKENS));
        detail.tokens.observed = None;
        detail.tokens.disposition = disposition;
    }

    pub(super) fn note_source_bytes(&mut self, bytes: usize) {
        if let Some(detail) = self.detail_mut() {
            detail.source_bytes = Some(as_i64(bytes));
        }
    }

    pub(super) fn finish_success(&mut self, returned_lines: usize, returned_bytes: usize) {
        if let Some(detail) = self.detail_mut() {
            detail.returned_lines = Some(as_i64(returned_lines));
            detail.returned_bytes = Some(as_i64(returned_bytes));
        }
        self.accept_tokens();
        self.summary.result = if returned_bytes == 0 {
            ToolSourceResult::Empty
        } else {
            ToolSourceResult::Succeeded
        };
        self.finish();
    }

    fn apply_classification(
        &mut self,
        path: &Path,
        has_manager: bool,
        scope: Option<SkillScope>,
        max_lines: usize,
        max_bytes: Option<usize>,
    ) {
        let role = classify_role(path);
        let (skill_match, skill_scope) = match (has_manager, scope) {
            (false, _) => (RegistryMatch::Unknown, None),
            (true, Some(scope)) => (RegistryMatch::Registered, Some(scope)),
            (true, None) if matches!(role, ReadRole::SkillEntry | ReadRole::SkillSupport) => {
                (RegistryMatch::Unregistered, None)
            }
            (true, None) => (RegistryMatch::Unknown, None),
        };
        let text = matches!(
            role,
            ReadRole::SkillEntry | ReadRole::SkillSupport | ReadRole::Instruction
        );
        let max_lines = as_i64(max_lines);
        self.max_lines = Some(max_lines);
        if let Some(detail) = self.detail_mut() {
            detail.role = role;
            detail.skill_match = skill_match;
            detail.skill_scope = skill_scope;
            detail.lines.configured = Some(max_lines);
            detail.lines.applicability = if text {
                CapApplicability::Applies
            } else {
                CapApplicability::Unknown
            };
            detail.formatted_bytes = match max_bytes {
                Some(limit) => ReadLimitSlot {
                    applicability: if text {
                        CapApplicability::Applies
                    } else {
                        CapApplicability::Unknown
                    },
                    configured: Some(as_i64(limit)),
                    observed: None,
                    disposition: CapDisposition::Unobserved,
                },
                None => ReadLimitSlot {
                    applicability: CapApplicability::NotApplicable,
                    configured: None,
                    observed: None,
                    disposition: CapDisposition::Unobserved,
                },
            };
            detail.tokens.configured = Some(as_i64(READ_FILE_MAX_TOKENS));
            detail.tokens.applicability = if text {
                CapApplicability::Applies
            } else {
                CapApplicability::Unknown
            };
            detail.tokens.observed = None;
            detail.tokens.disposition = CapDisposition::Unobserved;
        }
        self.finish();
    }

    fn note_returned(&mut self, content: &crate::types::output::FileContent) {
        if let Some(detail) = self.detail_mut() {
            detail.returned_bytes = Some(as_i64(content.content.len()));
            detail.returned_lines = Some(as_i64(line_count(&content.raw_output)));
        }
    }

    fn mark_text_caps_not_applicable(&mut self) {
        let Some(detail) = self.detail_mut() else {
            return;
        };
        detail.lines.applicability = CapApplicability::NotApplicable;
        detail.lines.disposition = CapDisposition::Unobserved;
        detail.lines.observed = None;
        detail.formatted_bytes.applicability = CapApplicability::NotApplicable;
        detail.formatted_bytes.disposition = CapDisposition::Unobserved;
        detail.formatted_bytes.observed = None;
        detail.tokens.applicability = CapApplicability::NotApplicable;
        detail.tokens.disposition = CapDisposition::Unobserved;
        detail.tokens.observed = None;
    }

    fn finish(&mut self) {
        let limit = self.summary.read().map(ReadDetail::output_limit);
        if let Some(limit) = limit {
            self.summary.output_limit = limit;
        }
    }

    fn detail_mut(&mut self) -> Option<&mut ReadDetail> {
        match &mut self.summary.detail {
            ToolSourceDetail::Read(detail) => Some(detail),
            ToolSourceDetail::None => None,
        }
    }
}

impl Drop for ReadObservation {
    fn drop(&mut self) {
        if self.slot.is_none() {
            return;
        }
        if matches!(
            self.summary.result,
            ToolSourceResult::Unknown(UnknownReason::NotInstrumented)
        ) {
            self.summary.result = ToolSourceResult::Failed(None);
        }
        self.finish();
        let summary = self.summary.clone();
        if let Some(slot) = &self.slot {
            slot.record(summary);
        }
    }
}

fn settle_empty_applicable(slot: &mut ReadLimitSlot) {
    if slot.applicability == CapApplicability::Applies {
        slot.disposition = CapDisposition::WithinLimit;
        slot.observed = Some(0);
    }
}

/// Skill entry wins over instruction. Instruction wins over other skill markdown.
fn classify_role(path: &Path) -> ReadRole {
    if path.file_name().is_some_and(|name| name == "SKILL.md") {
        return ReadRole::SkillEntry;
    }
    if is_instruction_markdown(path) {
        return ReadRole::Instruction;
    }
    if is_skill_markdown(path) {
        return ReadRole::SkillSupport;
    }
    ReadRole::Ordinary
}

pub(super) fn line_count(raw: &str) -> usize {
    if raw.is_empty() {
        0
    } else {
        raw.lines().count()
    }
}
