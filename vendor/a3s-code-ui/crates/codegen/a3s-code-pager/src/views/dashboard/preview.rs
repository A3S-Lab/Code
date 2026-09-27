//! The preview temporarily uses the selected agent's scrollback viewport.

use std::path::PathBuf;

use indexmap::IndexMap;
use ratatui::layout::Rect;

use crate::app::agent::AgentId;
use crate::app::agent_view::AgentView;
use crate::scrollback::state::ScrollbackState;
use crate::views::dashboard::layout::{self, DashboardLayout};
use crate::views::dashboard::peek::{self, PeekFields, PeekPanelState};
use crate::views::dashboard::peek_tail;
use crate::views::dashboard::state::{
    DashboardRowId, DashboardState, scrollback_available_for_row, scrollback_mut_for_row,
};
use crate::views::prompt_widget::PromptWidget;

struct MeasureLiveTailContent<'a> {
    area: Rect,
    dispatch: Rect,
    reply: &'a PromptWidget,
    scrollback: Option<&'a mut ScrollbackState>,
}

impl DashboardState {
    pub(crate) fn set_preview_enabled(
        &mut self,
        enabled: bool,
        agents: &mut IndexMap<AgentId, AgentView>,
    ) {
        self.preview_enabled = enabled;
        if !enabled {
            self.set_peek(None);
            self.set_peek_reply_target_cwd(None);
            self.file_search_dropdown_items_area = None;
            self.restore_peek_viewport(agents);
        }
    }

    pub(crate) fn layout_with_preview(
        &mut self,
        area: Rect,
        agents: &mut IndexMap<AgentId, AgentView>,
    ) -> DashboardLayout {
        let mut result = layout::compute_layout(area, false);
        if !self.preview_enabled {
            return result;
        }

        if !self.search_mode {
            result = self.layout_for_selected_preview(area, result, agents);
        }
        self.ensure_peek_viewport(agents);
        result
    }

    fn layout_for_selected_preview(
        &mut self,
        area: Rect,
        mut result: DashboardLayout,
        agents: &mut IndexMap<AgentId, AgentView>,
    ) -> DashboardLayout {
        let Some(selected) = self.selected.clone() else {
            self.set_peek_reply_target_cwd(None);
            self.set_peek(None);
            return result;
        };
        let Some(fields) = peek::compute_peek_fields(&selected, agents) else {
            self.set_peek_reply_target_cwd(None);
            self.set_peek(None);
            return result;
        };

        let question = fields.question.is_some();
        let peek_min = if question {
            layout::PEEK_MIN_BOX_QUESTION
        } else {
            layout::PEEK_MIN_BOX_LIVE_TAIL
        };
        let content_rows = if question {
            1 + fields.options.len().min(9) as u16
        } else {
            Self::measure_live_tail_content_rows(MeasureLiveTailContent {
                area,
                dispatch: result.dispatch,
                reply: &self.peek_reply,
                scrollback: scrollback_mut_for_row(&selected, agents),
            })
        };

        let allocation = layout::allocate_peek(
            area.height,
            layout::chrome_overhead(area),
            content_rows,
            peek_min,
        );
        if !allocation.show_peek {
            self.set_peek_reply_target_cwd(None);
            self.set_peek(None);
            return result;
        }

        self.refresh_peek_for_selection(selected, fields, agents);
        result = layout::compute_layout_with_peek_box(area, allocation.peek_box_h);
        result
    }

    fn measure_live_tail_content_rows(input: MeasureLiveTailContent<'_>) -> u16 {
        let MeasureLiveTailContent {
            area,
            dispatch,
            reply,
            scrollback,
        } = input;
        let reply_text_w = dispatch.width.saturating_sub(6);
        let reply_rows = peek::reply_row_count(reply, reply_text_w, peek::MAX_REPLY_ROWS);
        let max_content = layout::max_peek_content_rows(area);
        let middle_w = dispatch.width.saturating_sub(4);
        let (body_measured, pin_user) = scrollback
            .map(|scrollback| {
                (
                    peek_tail::densified_body_line_count(scrollback, middle_w),
                    peek_tail::scrollback_has_last_user(scrollback),
                )
            })
            .unwrap_or((0, false));

        layout::peek_live_tail_desired_content(max_content, reply_rows, body_measured, pin_user)
            .content_rows
    }

    fn refresh_peek_for_selection(
        &mut self,
        selected: DashboardRowId,
        fields: PeekFields,
        agents: &IndexMap<AgentId, AgentView>,
    ) {
        self.set_peek_reply_target_cwd(Self::peeked_agent_cwd(&selected, agents));
        let badge = peek::peek_model_and_mode(&selected, agents);
        match self.peek.as_mut() {
            Some(panel) => {
                if panel.apply_fields(selected, fields) {
                    self.clear_peek_reply();
                }
            }
            None => self.set_peek(Some(PeekPanelState::new(selected, fields))),
        }

        if let Some(panel) = self.peek.as_mut() {
            panel.model_name = badge.model;
            panel.auto_approve = badge.yolo;
            panel.auto = badge.auto;
            panel.mode_label = badge.mode_label;
        }
    }

    fn peeked_agent_cwd(
        selected: &DashboardRowId,
        agents: &IndexMap<AgentId, AgentView>,
    ) -> Option<PathBuf> {
        let reply_agent = match selected {
            DashboardRowId::TopLevel(id) => Some(*id),
            DashboardRowId::Roster { .. } | DashboardRowId::Workspace { .. } => None,
        };
        reply_agent.and_then(|id| agents.get(&id).map(|agent| agent.session.cwd.clone()))
    }

    fn ensure_peek_viewport(&mut self, agents: &mut IndexMap<AgentId, AgentView>) {
        if self.attached_agent.is_some() {
            return;
        }

        let Some(row) = self.peek.as_ref().map(|panel| panel.row.clone()) else {
            self.restore_peek_viewport(agents);
            return;
        };
        if self
            .peek_viewport
            .as_ref()
            .is_some_and(|lease| lease.row == row)
        {
            return;
        }

        if scrollback_available_for_row(&row, agents) {
            self.begin_peek_viewport(row, agents);
        } else {
            self.restore_peek_viewport(agents);
        }
    }
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
