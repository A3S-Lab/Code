use super::common::*;

const TIMEOUT: Duration = Duration::from_secs(20);

fn open_settings(harness: &mut PtyHarness) {
    harness.inject_keys(keys::F2).expect("open settings");
    harness
        .wait_for_text("Appearance", TIMEOUT)
        .expect("settings shown");

    harness.inject_keys(b"/").expect("search settings");
    harness
        .wait_for_text_absent("/ to search", TIMEOUT)
        .expect("search focused");
    harness
        .inject_keys(b"dashboard preview")
        .expect("search for preview");
    harness
        .wait_for_text("dashboard preview", TIMEOUT)
        .expect("search query rendered");
    harness
        .wait_for_text_absent("Compact mode", TIMEOUT)
        .expect("unrelated settings filtered");
    harness
        .inject_keys(keys::ENTER)
        .expect("commit settings search");
}

fn wait_setting(harness: &mut PtyHarness, enabled: bool) {
    let value = if enabled { "on" } else { "off" };
    harness
        .wait_until("preview setting value", TIMEOUT, |harness| {
            harness.screen_contents().lines().any(|line| {
                line.contains("Dashboard preview")
                    && line.split_whitespace().any(|word| word == value)
            })
        })
        .expect("preview setting updated");
}

fn open_dashboard(harness: &mut PtyHarness) {
    harness.inject_keys(CTRL_BACKSLASH).expect("open dashboard");
    harness
        .wait_for_text("+ New Agent", TIMEOUT)
        .expect("dashboard shown");
    harness
        .wait_until("session loaded before navigation", RESUME_TIMEOUT, |h| {
            let screen = h.screen_contents();
            screen.contains("1 idle")
                && screen.contains(MOCK_RESPONSE_SENTINEL)
                && !screen.contains("Loading…")
        })
        .expect("session replay completed");

    harness.inject_keys(keys::DOWN).expect("select section");
    harness
        .wait_for_text("Enter:collapse", TIMEOUT)
        .expect("section selected");
    harness.inject_keys(keys::DOWN).expect("select session");
    harness
        .wait_until("expected session selected", TIMEOUT, |h| {
            h.screen_contents()
                .lines()
                .any(|line| line.contains('▏') && line.contains(MOCK_RESPONSE_SENTINEL))
        })
        .expect("session selected");
}

fn draft_row(harness: &mut PtyHarness) -> usize {
    harness.inject_keys(b"PREVIEWDRAFT").expect("type draft");
    harness
        .wait_for_text("PREVIEWDRAFT", TIMEOUT)
        .expect("draft rendered");
    let screen = harness.screen_contents();
    let border_row = find_draft_prompt_border_row(&screen);

    harness.inject_keys(b"\x15").expect("clear draft");
    harness
        .wait_for_text_absent("PREVIEWDRAFT", TIMEOUT)
        .expect("draft cleared");
    border_row
}

fn find_draft_prompt_border_row(screen: &str) -> usize {
    screen
        .lines()
        .enumerate()
        .take_while(|(_, line)| !line.contains("PREVIEWDRAFT"))
        .filter(|(_, line)| line.contains('╭'))
        .map(|(row, _)| row)
        .last()
        .expect("prompt top border")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; requires a rebuilt pager binary"]
async fn dashboard_preview_setting_persists_and_restores() {
    for workspace in ["0", "1"] {
        verify_preview_preference(workspace).await;
    }
}

async fn verify_preview_preference(workspace: &str) {
    let content = ContentController::start().await.expect("mock server");
    content.set_response(format!(
        "{MOCK_RESPONSE_SENTINEL} dashboard preview response."
    ));

    let binary = pager_binary().expect("pager binary");
    let mut harness = PtyHarness::spawn_with_content_env(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &["--no-leader"],
        &[("GROK_WORKSPACE_DASHBOARD", workspace)],
    )
    .expect("start pager");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome");

    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("send prompt");
    harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, TIMEOUT)
        .expect("response");
    harness.wait_for_turn_idle(TIMEOUT).expect("idle session");

    open_settings(&mut harness);
    wait_setting(&mut harness, true);

    harness.inject_keys(b" ").expect("disable preview");
    wait_setting(&mut harness, false);
    harness.inject_keys(keys::F2).expect("close settings");
    harness
        .wait_for_text("Dashboard preview: off", TIMEOUT)
        .expect("preview disable saved");

    open_dashboard(&mut harness);
    let without_preview = draft_row(&mut harness);

    harness
        .inject_keys(keys::ENTER)
        .expect("open selected session");
    harness
        .wait_for_text_absent("+ New Agent", TIMEOUT)
        .expect("session opened");

    open_settings(&mut harness);
    wait_setting(&mut harness, false);
    harness.inject_keys(keys::F2).expect("close settings");
    harness.quit().expect("stop first pager");

    let config: toml::Value = toml::from_str(
        &std::fs::read_to_string(content.sandbox().grok_home().join("config.toml"))
            .expect("saved config"),
    )
    .expect("valid saved config");

    assert_eq!(
        Some(false),
        config
            .get("ui")
            .and_then(|ui| ui.get("dashboard_preview"))
            .and_then(toml::Value::as_bool)
    );

    let mut harness = PtyHarness::spawn_with_content_env(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &["--no-leader", "-c"],
        &[("GROK_WORKSPACE_DASHBOARD", workspace)],
    )
    .expect("restart pager");
    harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, RESUME_TIMEOUT)
        .expect("resumed session");

    open_settings(&mut harness);
    wait_setting(&mut harness, false);

    harness.inject_keys(b" ").expect("enable preview");
    wait_setting(&mut harness, true);
    harness.inject_keys(keys::F2).expect("close settings");
    harness
        .wait_for_text("Dashboard preview: on", TIMEOUT)
        .expect("preview enable saved");

    open_dashboard(&mut harness);
    harness
        .wait_until("response inside preview box", TIMEOUT, |h| {
            h.screen_contents()
                .lines()
                .skip_while(|line| !line.contains('╭'))
                .any(|line| line.contains(MOCK_RESPONSE_SENTINEL))
        })
        .expect("preview visibly open");
    let with_preview = draft_row(&mut harness);

    assert_ne!(
        without_preview,
        with_preview,
        "preview must change the compose layout (workspace={workspace})\n{}",
        harness.screen_contents()
    );

    harness
        .inject_keys(keys::ENTER)
        .expect("open session with preview enabled");
    harness
        .wait_for_text_absent("+ New Agent", TIMEOUT)
        .expect("session opened");
    harness.quit().expect("stop second pager");
}
