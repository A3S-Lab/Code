//! Session modes an agent publishes, kept next to the plan flags every plan surface already reads.

use a3s_code_tools::types::SessionMode;
use agent_client_protocol as acp;

use crate::app::agent_view::AgentView;

impl AgentView {
    /// Stores the modes the agent published and turns Auto off. Ask replaces Auto for these agents.
    pub(crate) fn apply_session_modes(&mut self, modes: Option<acp::SessionModeState>) -> bool {
        let Some(modes) = modes else {
            return false;
        };
        self.session_mode = SessionMode::from_id(&modes.current_mode_id.0);
        self.session_mode_pending = None;
        self.plan_mode_active = self.session_mode.is_plan();
        self.available_modes = modes.available_modes;

        self.session.auto_mode = false;
        if self.deferred_permission_mode == Some("auto") {
            self.deferred_permission_mode = None;
        }

        true
    }

    /// Optimistic pick, else the confirmed mode. Plan exits through the plan flags must not
    /// still read as plan here.
    pub(crate) fn effective_session_mode(&self) -> SessionMode {
        if self.plan_mode_pending.unwrap_or(self.plan_mode_active) {
            return SessionMode::Plan;
        }
        let picked = self
            .session_mode_pending
            .as_ref()
            .unwrap_or(&self.session_mode);
        match picked {
            SessionMode::Ask => SessionMode::Ask,
            SessionMode::Default | SessionMode::Plan => SessionMode::Default,
        }
    }

    /// Prompt-row / peek flag for a published non-plan mode. Agent shows none.
    pub(crate) fn published_mode_label(&self) -> Option<&'static str> {
        match self.effective_session_mode() {
            SessionMode::Ask => Some(SessionMode::Ask.as_id()),
            SessionMode::Default | SessionMode::Plan => None,
        }
    }

    /// Same flag the prompt row uses when it is not in commenting / plan-approval chrome.
    pub(crate) fn prompt_row_mode_label(&self) -> Option<&'static str> {
        if self.plan_mode_pending.unwrap_or(self.plan_mode_active) {
            Some("plan")
        } else {
            self.published_mode_label()
        }
    }

    /// Optimistic Shift+Tab pick, pending the agent's `CurrentModeUpdate`.
    pub(crate) fn stage_session_mode(&mut self, mode: SessionMode) {
        self.stage_plan_mode(mode.is_plan());
        self.session_mode_pending = Some(mode);
    }
}
