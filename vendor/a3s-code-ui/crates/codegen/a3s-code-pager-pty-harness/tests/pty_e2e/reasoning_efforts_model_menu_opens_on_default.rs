// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// `/model <name> ` opens the effort sub-menu on the model's default effort, so a bare Enter picks that level.
/// The current model is `a3s-4.5` so the session's live effort (`xhigh`) cannot mask `a3s-4.6`'s catalog default.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn reasoning_efforts_model_menu_opens_on_default() {
    let content = ContentController::start_with_models(vec![
        MockModel::new("grok-4.5")
            .with_api_backend("responses")
            .with_supports_reasoning_effort(true)
            .with_reasoning_effort("xhigh"),
        MockModel::new("grok-4.6")
            .with_api_backend("responses")
            .with_supports_reasoning_effort(true)
            .with_reasoning_effort("high"),
    ])
    .await
    .expect("start content");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} first turn."));

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness =
        PtyHarness::spawn_with_content(&binary, DEFAULT_ROWS, DEFAULT_COLS, &content, &[])
            .expect("spawn pager");

    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");

    // Establish a session so the session-scoped `/model` command is available.
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
        .expect("first turn rendered");

    // The trailing space chains into the effort sub-menu.
    inject_keys_paced(&mut harness, b"/model grok-4.6 ");
    harness
        .wait_for_text("Heavy reasoning", Duration::from_secs(10))
        .expect("effort sub-menu rendered");
    harness
        .inject_keys(b"\r")
        .expect("accept highlighted effort");
    harness
        .wait_for_text(
            "Switched to grok-4.6 (high effort)",
            Duration::from_secs(10),
        )
        .expect("switch landed on the default effort");

    // Distinct from `PROMPT` and from any system-prompt wording, so the body lookup below cannot match the first turn.
    let second_prompt = "SECONDTURNPROMPT";
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} second turn."));
    harness
        .inject_keys(format!("{second_prompt}\r").as_bytes())
        .expect("submit second prompt");
    harness
        .wait_for_text("second turn", Duration::from_secs(30))
        .expect("second turn rendered");

    // Auxiliary requests (turn summary) can land after the turn, so find the body by its prompt text, not by position.
    let bodies = content.request_bodies();
    let Some(second_turn) = bodies
        .iter()
        .find(|b| b.to_string().contains(second_prompt))
    else {
        panic!("no request body carries the second prompt\nbodies: {bodies:#?}");
    };
    assert_eq!(
        Some("grok-4.6"),
        second_turn.pointer("/model").and_then(|v| v.as_str()),
        "second turn must run on the switched model\nbody: {second_turn:#?}"
    );
    assert_eq!(
        Some("high"),
        second_turn
            .pointer("/reasoning/effort")
            .and_then(|v| v.as_str()),
        "Enter on the freshly opened sub-menu must send the default effort\nbody: {second_turn:#?}"
    );

    harness.quit().expect("clean quit");
}
