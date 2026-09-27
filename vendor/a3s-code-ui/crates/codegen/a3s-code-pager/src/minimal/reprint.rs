//! Decides when minimal mode reprints its history after a width or compact-layout change.
//!
//! A height change can flip the auto-compact layout (`views::agent::effective_compact`). Printed history keeps the old layout.
//! `a3s-code-pager-minimal` does the printing.

use std::time::{Duration, Instant};

use crate::app::app_view::AppView;

/// A dragged window sends a burst of resize events. The reprint waits until they stop for this long.
pub const REPRINT_DEBOUNCE: Duration = Duration::from_millis(120);

/// What a minimal frame should do about the printed history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReprintDecision {
    /// The history is already printed in this layout.
    Keep,
    /// The layout changed less than [`REPRINT_DEBOUNCE`] ago.
    Wait,
    /// Reprint, then call [`mark_minimal_history_printed`].
    Reprint,
}

/// The wrap width and compact flag committed history is rendered against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PrintedLayout {
    width: u16,
    compact: bool,
}

/// What native scrollback holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrintedHistory {
    /// Every row was printed in this layout.
    Uniform(PrintedLayout),
    /// Some rows were printed in another layout. Returning to an earlier layout does not repair them.
    Mixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingReprint {
    layout: PrintedLayout,
    due_at: Instant,
}

/// Held on `AppView::minimal_state`.
#[derive(Debug, Default)]
pub(crate) struct ReprintState {
    /// `None` until the first minimal frame or the first printed rows.
    printed: Option<PrintedHistory>,
    pending: Option<PendingReprint>,
}

impl ReprintState {
    pub(crate) fn is_waiting(&self) -> bool {
        self.pending.is_some()
    }

    fn observe(&mut self, layout: PrintedLayout, now: Instant) -> ReprintDecision {
        match self.printed {
            None => {
                self.printed = Some(PrintedHistory::Uniform(layout));
                return ReprintDecision::Keep;
            }
            Some(PrintedHistory::Uniform(printed)) if printed == layout => {
                self.pending = None;
                return ReprintDecision::Keep;
            }
            Some(_) => {}
        }
        match self.pending {
            Some(pending) if pending.layout == layout && now >= pending.due_at => {
                ReprintDecision::Reprint
            }
            Some(pending) if pending.layout == layout => ReprintDecision::Wait,
            _ => {
                self.pending = Some(PendingReprint {
                    layout,
                    due_at: now + REPRINT_DEBOUNCE,
                });
                ReprintDecision::Wait
            }
        }
    }

    fn record_rows(&mut self, layout: PrintedLayout) {
        match self.printed {
            None => self.printed = Some(PrintedHistory::Uniform(layout)),
            Some(PrintedHistory::Uniform(printed)) if printed != layout => {
                self.printed = Some(PrintedHistory::Mixed);
            }
            Some(_) => {}
        }
    }

    fn mark_printed(&mut self, layout: PrintedLayout) {
        self.printed = Some(PrintedHistory::Uniform(layout));
        self.pending = None;
    }
}

fn current_layout(app: &AppView, width: u16) -> PrintedLayout {
    PrintedLayout {
        width,
        compact: app.appearance.prompt.compact,
    }
}

/// Record this frame's layout (`width` plus the current compact flag) and decide whether the history must be reprinted.
/// `Reprint` repeats every frame until [`mark_minimal_history_printed`] is called.
pub fn observe_minimal_layout(app: &mut AppView, width: u16, now: Instant) -> ReprintDecision {
    let layout = current_layout(app, width);
    app.minimal_state.reprint.observe(layout, now)
}

/// Every history writer except the reprint calls this after printing rows at `width` in the current compact layout.
/// Rows in another layout than the printed history make the next reprint unconditional.
pub fn record_minimal_rows_printed(app: &mut AppView, width: u16) {
    let layout = current_layout(app, width);
    app.minimal_state.reprint.record_rows(layout);
}

/// The whole history, welcome card included, is now in native scrollback at `width` in the current compact layout.
pub fn mark_minimal_history_printed(app: &mut AppView, width: u16) {
    let layout = current_layout(app, width);
    app.minimal_state.reprint.mark_printed(layout);
    app.minimal_state.welcome_pending = false;
}

#[cfg(test)]
#[path = "reprint_tests.rs"]
mod tests;
