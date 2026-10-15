//! WC-BG1 — background execution as a Run-owned governed effect, on the
//! real model.
//!
//! One turn asks the model to (1) start a delayed-emitter background task
//! that stays alive after emitting, (2) do unrelated work (read the
//! README), (3) tail until the marker arrives, then echo the marker.
//! Mechanical oracles:
//! - the marker line landed as a `background.output` FACT on the one log;
//! - the final answer names the exact marker token (arrival-reactive);
//! - an `exited` fact records "terminated by request" — the model answers
//!   seconds into a 30-second command, so the ONLY way the task settled
//!   before `send` returned is the Run-owned settlement effect killing it
//!   (settlement precedes the turn's successful return);
//! - no process carrying the marker survives the turn.
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo +1.97.1 test -p a3s-code-core --test test_background_run_real_llm \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

mod support;

use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::{Agent, PlanningMode, SessionOptions};
use support::layer_c_model::load_pinned_layer_c_config;

#[tokio::test]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn the_model_runs_watches_and_settles_a_background_task() {
    let workspace = tempfile::tempdir().expect("workspace");
    let root = workspace.path().to_path_buf();
    std::fs::write(root.join("README.md"), "background live\n").expect("README");

    let mark = format!("BG1_LIVE_MARK_{}", uuid::Uuid::new_v4().simple());
    let mut config = load_pinned_layer_c_config();
    config.default_model = Some(support::layer_c_model::pinned_layer_c_model(&config));
    let agent = Agent::from_config(config).await.expect("agent");
    let session = agent
        .session_async(
            root.to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("wc-bg1-live")
                    .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
                    .with_planning(false)
                    .with_planning_mode(PlanningMode::Disabled)
                    .with_auto_delegation_enabled(false)
                    .with_manual_delegation_enabled(false)
                    .with_llm_api_timeout(180_000),
            ),
        )
        .await
        .expect("session");

    let answer = tokio::time::timeout(
        Duration::from_secs(240),
        session.send(
            &format!(
                "Use the background tool and the read tool, in this exact order:\n\
                 1. background: action=start, name=watch, command=sh -c 'sleep 2; echo {mark}; sleep 30'\n\
                 2. read: path=README.md\n\
                 3. background: action=tail, name=watch, wait_for={mark}, timeout_ms=15000\n\
                 When the tail reports the pattern as seen, reply with exactly {mark} and nothing else."
            ),
            None,
        ),
    )
    .await
    .expect("turn timed out")
    .expect("turn failed");
    println!("BG1 reply: {}", answer.text.trim());

    // Oracle 1 — the marker is a fact on the one log.
    let facts =
        a3s_code_core::fact_control::read_workspace_facts(&root, "wc-bg1-live").expect("facts");
    let marker_fact = facts
        .iter()
        .filter(|fact| fact.kind == "background.output")
        .find(|fact| {
            fact.payload
                .get("line")
                .and_then(|value| value.as_str())
                .is_some_and(|line| line.contains(&mark))
        });
    assert!(marker_fact.is_some(), "the marker must land as a fact");
    println!("BG1 marker fact seq: {:?}", marker_fact.unwrap().seq);

    // Oracle 2 — arrival-reactive: the model could only name the token it
    // received from the background stream.
    assert!(
        answer.text.contains(&mark),
        "the answer must name the exact marker token; got: {}",
        answer.text.trim()
    );

    // Oracle 3 — settlement killed the still-running task before send
    // returned: the model answered ~2s into a 30s command.
    let exited = facts
        .iter()
        .filter(|fact| fact.kind == "background.output")
        .find(|fact| fact.payload.get("event").and_then(|value| value.as_str()) == Some("exited"))
        .expect("an exited fact must record the settlement");
    let exit_line = exited
        .payload
        .get("line")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    assert!(
        exit_line.contains("terminated by request"),
        "settlement (not natural exit) must own the disposition; got: {exit_line}"
    );
    println!("BG1 exited line: {exit_line}");

    // Oracle 4 — nothing carrying the marker survives the turn.
    let pgrep = std::process::Command::new("pgrep")
        .arg("-f")
        .arg(&mark)
        .output()
        .expect("pgrep spawns");
    assert!(
        pgrep.stdout.is_empty(),
        "no process carrying the marker may survive the settled Run; pgrep saw: {}",
        String::from_utf8_lossy(&pgrep.stdout)
    );
    println!(
        "BG1 verdict: marker-fact=present arrival-reactive=answered settlement=killed orphans=none"
    );
}
