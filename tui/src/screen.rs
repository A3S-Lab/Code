//! Full-screen loop: scrollback, prompt dock, and slash menu.
//!
//! The prompt is the vendored text area. Scrollback columns use the vendored
//! layout. Slash entry uses the vendored parser and fuzzy matcher. Enter on a
//! normal line calls [`crate::submit_configured_turn`].

use std::io::{stdout, IsTerminal};
use std::path::Path;
use std::time::Duration;

use a3s_prompt_textarea::TextArea;
use crossterm::cursor::Show;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::terminal::{Clear, ClearType};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Terminal;

use crate::model::LaunchLayers;
use crate::scrollback::{HorizontalLayout, LayoutConfig};
use crate::session::submit_configured_turn;
use crate::slash::{matching_commands, parse_slash, MAX_VISIBLE_SUGGESTIONS};
use crate::transcript::Scrollback;
use crate::{merge_launch_layers, PRODUCT_NAME};

/// Paint one frame and read keys until `/exit`.
///
/// `on_first_frame` runs after the first frame is flushed so startup work
/// that waits on the terminal stays behind that paint.
pub async fn run_fullscreen(
    workspace: &Path,
    explicit_config: Option<&Path>,
    home: Option<&Path>,
    fallback_model: &str,
    mut on_first_frame: Option<Box<dyn FnOnce() + Send>>,
) -> anyhow::Result<()> {
    let layers = LaunchLayers {
        workspace: workspace.to_path_buf(),
        explicit_config: explicit_config.map(Path::to_path_buf),
        home: home.map(Path::to_path_buf),
    };
    let model_id = merge_launch_layers(&layers)
        .map(|merged| merged.model_id)
        .unwrap_or_else(|_| fallback_model.to_string());
    if !stdout().is_terminal() {
        println!("{PRODUCT_NAME}");
        println!("model: {model_id}");
        println!("Loading workspace");
        if let Some(callback) = on_first_frame.take() {
            callback();
        }
        return Ok(());
    }

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, Clear(ClearType::All))?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let _restore = TerminalRestore;
    let mut scrollback = Scrollback::default();
    scrollback.push_assistant("Loading workspace");
    let mut prompt = TextArea::new();
    let mut menu_index = 0usize;
    let mut painted = false;
    let mut dirty = true;

    loop {
        let menu = menu_for(prompt.text());
        if menu_index >= menu.len() {
            menu_index = 0;
        }
        if dirty {
            draw(
                &mut terminal,
                &model_id,
                &scrollback,
                &prompt,
                &menu,
                menu_index,
            )?;
            dirty = false;
            if !painted {
                painted = true;
                if let Some(callback) = on_first_frame.take() {
                    callback();
                }
            }
        }
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        let event = event::read()?;
        if matches!(event, Event::Resize(_, _)) {
            dirty = true;
            continue;
        }
        let Event::Key(key) = event else {
            continue;
        };
        dirty = true;
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Enter if bare_enter(&key) => {
                let line = prompt.text().trim().to_string();
                prompt.set_text("");
                menu_index = 0;
                if line.is_empty() {
                    continue;
                }
                match parse_slash(&line) {
                    Some("exit" | "quit") => break,
                    Some("clear") => scrollback.clear(),
                    Some("model") => scrollback.push_assistant(&format!("model: {model_id}")),
                    Some("help") => {
                        scrollback.push_assistant("commands: /help /model /clear /exit")
                    }
                    Some(_) => scrollback.push_assistant("unknown command"),
                    None => {
                        scrollback.push_user(&line);
                        match submit_configured_turn(&layers, None, &line).await {
                            Ok(turn) => scrollback.push_assistant(&turn.text),
                            Err(error) => scrollback.push_assistant(&error.to_string()),
                        }
                    }
                }
            }
            KeyCode::Esc => break,
            KeyCode::Up if !menu.is_empty() => {
                menu_index = menu_index.saturating_sub(1);
            }
            KeyCode::Down if !menu.is_empty() => {
                if menu_index + 1 < menu.len() {
                    menu_index += 1;
                }
            }
            KeyCode::Tab if !menu.is_empty() => {
                if let Some(name) = menu.get(menu_index) {
                    prompt.set_text(&format!("/{name}"));
                }
            }
            _ => prompt.input(key),
        }
    }
    Ok(())
}

fn bare_enter(key: &KeyEvent) -> bool {
    !key.modifiers
        .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL)
}

fn menu_for(text: &str) -> Vec<&'static str> {
    let trimmed = text.trim();
    if !trimmed.starts_with('/') || trimmed.contains(char::is_whitespace) {
        return Vec::new();
    }
    matching_commands(trimmed)
}

fn draw(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    model_id: &str,
    scrollback: &Scrollback,
    prompt: &TextArea,
    menu: &[&str],
    menu_index: usize,
) -> anyhow::Result<()> {
    terminal.draw(|frame| {
        let area = frame.area();
        let menu_rows = u16::try_from(menu.len().min(MAX_VISIBLE_SUGGESTIONS)).unwrap_or(0);
        let [header, body, suggestions, dock] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(menu_rows),
            Constraint::Length(1),
        ])
        .areas(area);

        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(PRODUCT_NAME, Style::default().fg(Color::Cyan))),
                Line::from(format!("model: {model_id}")),
            ]),
            header,
        );

        let columns = HorizontalLayout::new(body, &LayoutConfig::default());
        let visible = scrollback.window(columns.content.height as usize);
        let lines: Vec<Line> = visible.iter().cloned().map(Line::from).collect();
        frame.render_widget(Paragraph::new(lines), columns.content);

        if menu_rows > 0 {
            let rows: Vec<Line> = menu
                .iter()
                .take(MAX_VISIBLE_SUGGESTIONS)
                .enumerate()
                .map(|(index, name)| {
                    let style = if index == menu_index {
                        Style::default().fg(Color::Black).bg(Color::Cyan)
                    } else {
                        Style::default()
                    };
                    Line::from(Span::styled(format!("/{name}"), style))
                })
                .collect();
            frame.render_widget(Paragraph::new(rows), suggestions);
        }

        let [gutter, input] =
            Layout::horizontal([Constraint::Length(2), Constraint::Min(1)]).areas(dock);
        frame.render_widget(Paragraph::new("❯"), gutter);
        frame.render_widget_ref(prompt, input);
        // cursor_pos already includes the widget area origin.
        if let Some(position) = prompt.cursor_pos(input) {
            frame.set_cursor_position(position);
        }
    })?;
    Ok(())
}

struct TerminalRestore;

impl Drop for TerminalRestore {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen, Show, Clear(ClearType::All));
    }
}
