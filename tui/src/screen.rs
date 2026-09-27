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

use crate::agent::CodeAgentAdapter;
use crate::model::LaunchLayers;
use crate::scrollback::{HorizontalLayout, LayoutConfig};
use crate::slash::{
    find_command, matching_commands, parse_invocation, parse_slash, SlashCommand,
    MAX_VISIBLE_SUGGESTIONS, SLASH_COMMANDS,
};
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
    let mut agent = match CodeAgentAdapter::open(layers.clone()).await {
        Ok(agent) => Some(agent),
        Err(error) => {
            scrollback.push_assistant(&format!("agent: {error}"));
            None
        }
    };
    let model_id = agent
        .as_ref()
        .map(|agent| agent.model_id().to_string())
        .unwrap_or(model_id);

    loop {
        let menu = menu_for(prompt.text());
        if menu_index >= menu.len() {
            menu_index = 0;
        }
        if dirty {
            let permission = agent
                .as_ref()
                .map(|agent| agent.permission_mode().as_str())
                .unwrap_or("default");
            draw(
                &mut terminal,
                &model_id,
                permission,
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
                    Some("help") => scrollback.push_assistant(&help_text()),
                    Some("status") => {
                        let permission = agent
                            .as_ref()
                            .map(|a| a.permission_mode().as_str())
                            .unwrap_or("default");
                        let workspace = layers.workspace.display();
                        scrollback.push_assistant(&format!(
                            "workspace: {workspace}\nmodel: {model_id}\npermission: {permission}"
                        ));
                    }
                    Some("history") => {
                        let query = parse_invocation(&line)
                            .map(|invocation| invocation.args)
                            .unwrap_or("");
                        let hits = scrollback.history_matches(query, 12);
                        if hits.is_empty() {
                            scrollback.push_assistant("history: no matching prompts");
                        } else {
                            scrollback.push_assistant(&format!("history:\n{}", hits.join("\n")));
                        }
                    }
                    Some("export") => {
                        let arg = parse_invocation(&line)
                            .map(|invocation| invocation.args)
                            .unwrap_or("");
                        let path = if arg.is_empty() {
                            layers.workspace.join("a3s-session.md")
                        } else {
                            layers.workspace.join(arg)
                        };
                        match scrollback.export_markdown(&path) {
                            Ok(()) => {
                                scrollback.push_assistant(&format!("exported {}", path.display()))
                            }
                            Err(error) => scrollback.push_assistant(&error.to_string()),
                        }
                    }
                    Some("ctx") => {
                        let args = parse_invocation(&line)
                            .map(|invocation| invocation.args)
                            .unwrap_or("");
                        if args.is_empty() {
                            scrollback.push_assistant(crate::hubs::ctx_hub_help());
                        } else {
                            scrollback.push_user(&line);
                            if let Some(agent) = agent.as_mut() {
                                run_agent_turn(
                                    agent,
                                    &mut terminal,
                                    &model_id,
                                    &mut scrollback,
                                    &prompt,
                                    &line,
                                )
                                .await;
                            }
                        }
                    }
                    Some("use") => {
                        let args = parse_invocation(&line)
                            .map(|invocation| invocation.args)
                            .unwrap_or("");
                        if args.is_empty() {
                            scrollback.push_assistant(crate::hubs::use_hub_help());
                        } else {
                            scrollback.push_user(&line);
                            if let Some(agent) = agent.as_mut() {
                                run_agent_turn(
                                    agent,
                                    &mut terminal,
                                    &model_id,
                                    &mut scrollback,
                                    &prompt,
                                    &line,
                                )
                                .await;
                            }
                        }
                    }
                    Some("permissions") => {
                        if let Some(agent) = agent.as_mut() {
                            let mode = agent.cycle_permission_mode();
                            agent.cancel_session();
                            scrollback.push_assistant(&format!(
                                "permission: {} (Shift+Tab cycles)",
                                mode.as_str()
                            ));
                        }
                    }
                    Some("plan" | "ask") => {
                        if let Some(agent) = agent.as_mut() {
                            agent.set_permission_mode(crate::agent::PermissionMode::Plan);
                            agent.cancel_session();
                            scrollback.push_assistant("permission: plan");
                        }
                    }
                    Some("auto") => {
                        if let Some(agent) = agent.as_mut() {
                            agent.set_permission_mode(crate::agent::PermissionMode::Auto);
                            agent.cancel_session();
                            scrollback.push_assistant("permission: auto");
                        }
                    }
                    Some("yolo") => {
                        if let Some(agent) = agent.as_mut() {
                            agent.set_permission_mode(crate::agent::PermissionMode::Yolo);
                            agent.cancel_session();
                            scrollback.push_assistant("permission: yolo");
                        }
                    }
                    Some(name) => match find_command(name) {
                        Some(_) => {
                            scrollback.push_user(&line);
                            if let Some(agent) = agent.as_mut() {
                                run_agent_turn(
                                    agent,
                                    &mut terminal,
                                    &model_id,
                                    &mut scrollback,
                                    &prompt,
                                    &line,
                                )
                                .await;
                            }
                        }
                        None => scrollback.push_assistant("unknown command"),
                    },
                    None => {
                        scrollback.push_user(&line);
                        match agent.as_mut() {
                            Some(agent) => {
                                run_agent_turn(
                                    agent,
                                    &mut terminal,
                                    &model_id,
                                    &mut scrollback,
                                    &prompt,
                                    &line,
                                )
                                .await;
                            }
                            None => scrollback.push_assistant("agent unavailable"),
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
                if let Some(command) = menu.get(menu_index) {
                    prompt.set_text(&format!("/{}", command.name));
                }
            }
            KeyCode::BackTab => {
                if let Some(agent) = agent.as_mut() {
                    let mode = agent.cycle_permission_mode();
                    agent.cancel_session();
                    scrollback.push_assistant(&format!("permission: {}", mode.as_str()));
                }
            }
            _ => prompt.input(key),
        }
    }
    Ok(())
}

fn help_text() -> String {
    let mut lines = vec!["commands:".to_string()];
    for command in SLASH_COMMANDS.iter().filter(|c| c.name != "quit") {
        lines.push(format!("  /{} — {}", command.name, command.description));
    }
    lines.join("\n")
}

fn read_yes_no() -> bool {
    loop {
        if !event::poll(Duration::from_secs(60)).unwrap_or(false) {
            return false;
        }
        let Ok(Event::Key(key)) = event::read() else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('y' | 'Y') => return true,
            KeyCode::Char('n' | 'N') | KeyCode::Esc | KeyCode::Enter => return false,
            _ => {}
        }
    }
}

async fn run_agent_turn(
    agent: &mut CodeAgentAdapter,
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    model_id: &str,
    scrollback: &mut Scrollback,
    prompt: &TextArea,
    line: &str,
) {
    let permission = agent.permission_mode().as_str().to_string();
    let model_for_draw = model_id.to_string();
    if let Err(error) = agent
        .prompt_streaming(
            line,
            scrollback,
            |sb| {
                let _ = draw(terminal, &model_for_draw, &permission, sb, prompt, &[], 0);
            },
            |_| read_yes_no(),
        )
        .await
    {
        scrollback.push_assistant(&error.to_string());
    }
}

fn bare_enter(key: &KeyEvent) -> bool {
    !key.modifiers
        .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL)
}

fn menu_for(text: &str) -> Vec<&'static SlashCommand> {
    let trimmed = text.trim();
    if !trimmed.starts_with('/') || trimmed.contains(char::is_whitespace) {
        return Vec::new();
    }
    matching_commands(trimmed)
}

fn draw(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    model_id: &str,
    permission: &str,
    scrollback: &Scrollback,
    prompt: &TextArea,
    menu: &[&SlashCommand],
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
                Line::from(format!("model: {model_id} · permission: {permission}")),
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
                .map(|(index, command)| {
                    let style = if index == menu_index {
                        Style::default().fg(Color::Black).bg(Color::Cyan)
                    } else {
                        Style::default()
                    };
                    Line::from(Span::styled(
                        format!("/{}  {}", command.name, command.description),
                        style,
                    ))
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
