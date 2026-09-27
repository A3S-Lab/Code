//! Reprints the session history at the terminal's new width after a resize.
//!
//! A reflowing terminal re-wraps printed rows and splits committed text mid-word. A height change that flips the auto-compact layout also reprints.
//! After [`a3s_code_pager::minimal_reprint::REPRINT_DEBOUNCE`], minimal clears the screen and scrollback and prints the committed entries again.
//!
//! The clear also drops terminal output from before a3s started and from earlier `/new` sessions.
//! Only the newest [`REPRINT_MAX_ROWS`] rows are reprinted. Older blocks stay reachable through `/transcript`.

use std::io;
use std::time::Instant;

use crossterm::QueueableCommand;
use crossterm::cursor::MoveTo;
use crossterm::style::{Attribute, SetAttribute};
use crossterm::terminal::{Clear, ClearType};
use ratatui::text::Span;

use a3s_code_pager::app::PagerTerminal;
use a3s_code_pager::app::app_view::{ActiveView, AppView};
use a3s_code_pager::minimal_api;
use a3s_code_pager::minimal_reprint::{self, ReprintDecision};
use a3s_code_pager::render::Renderable;
use a3s_code_pager::theme::Theme;

use crate::commit::{
    COMMITTED_TICK, committed_appearance, insert_committed, minimal_commit_display_mode,
    minimal_renderer,
};

/// Row budget for one reprint, counted from the newest committed block backwards.
pub(crate) const REPRINT_MAX_ROWS: u16 = 4000;

/// Reprint the history once a width or compact-layout change has held for the debounce. Call it before this frame's commits.
pub fn maybe_reprint(app: &mut AppView, terminal: &mut PagerTerminal) {
    let width = terminal.viewport_area().width;
    // Below the card's width nothing reprints. Rows printed meanwhile still mark the history mixed
    if width < crate::welcome::MIN_CARD_WIDTH {
        return;
    }
    if minimal_reprint::observe_minimal_layout(app, width, Instant::now())
        != ReprintDecision::Reprint
    {
        return;
    }
    // A reprint under a band-owning modal would scroll the popup away
    if let ActiveView::Agent(id) = &app.active_view
        && app
            .agents
            .get(id)
            .is_some_and(crate::overlay::is_live_region_modal_active)
    {
        return;
    }
    match reprint_history(app, terminal, width) {
        Ok(()) => minimal_reprint::mark_minimal_history_printed(app, width),
        Err(error) => {
            tracing::warn!(%error, width, "minimal: history reprint failed; retrying next frame")
        }
    }
}

fn reprint_history(app: &AppView, terminal: &mut PagerTerminal, width: u16) -> io::Result<()> {
    let backend = terminal.backend_mut();
    backend.queue(SetAttribute(Attribute::Reset))?;
    backend.queue(MoveTo(0, 0))?;
    backend.queue(Clear(ClearType::All))?;
    backend.queue(Clear(ClearType::Purge))?;
    backend.queue(MoveTo(0, 0))?;
    crate::welcome::print_welcome_card(app, terminal)?;
    reprint_committed(app, terminal, width)
}

/// Re-emit the active agent's committed entries oldest first, within [`REPRINT_MAX_ROWS`].
/// Each keeps the display mode it was printed in. A block the user expanded with Ctrl+E prints in full.
fn reprint_committed(app: &AppView, terminal: &mut PagerTerminal, width: u16) -> io::Result<()> {
    let ActiveView::Agent(id) = &app.active_view else {
        return Ok(());
    };
    let Some(agent) = app.agents.get(id) else {
        return Ok(());
    };
    let appearance = committed_appearance(&app.appearance);
    let max_rows = appearance.minimal_max_commit_rows;
    let theme = Theme::current();
    let footer_style = theme.dim();
    let cwd = agent.session.cwd.as_path();
    let sb = &agent.scrollback;

    let committed: Vec<(usize, u16)> = (0..sb.len())
        .filter_map(|i| {
            let entry = sb.get(i).filter(|e| minimal_api::is_committed(sb, e))?;
            let is_commit_mode =
                entry.display_mode() == minimal_commit_display_mode(&entry.block, &appearance);
            Some((i, if is_commit_mode { max_rows } else { 0 }))
        })
        .collect();

    let mut budget = REPRINT_MAX_ROWS;
    let mut first = committed.len();
    for (k, &(i, cap)) in committed.iter().enumerate().rev() {
        let Some(entry) = sb.get(i) else { continue };
        let renderer = minimal_renderer(entry, &theme, appearance.clone(), cwd, COMMITTED_TICK);
        let full = renderer.desired_height(width);
        let rows = if cap > 0 { full.min(cap) } else { full };
        if rows > budget {
            break;
        }
        budget -= rows;
        first = k;
    }

    if first > 0 {
        let note = format!("\u{2026} {first} earlier blocks \u{00b7} /transcript to view");
        terminal.insert_before(1, |buf| {
            buf.set_span(0, 0, &Span::styled(note, footer_style), width);
        })?;
    }
    for &(i, cap) in committed.get(first..).unwrap_or_default() {
        let Some(entry) = sb.get(i) else { continue };
        let renderer = minimal_renderer(entry, &theme, appearance.clone(), cwd, COMMITTED_TICK);
        insert_committed(terminal, renderer, width, cap, footer_style)?;
    }
    Ok(())
}
