//! Real-LLM gate for the fact-log control source.
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_fact_log_real_llm \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

mod support;

use std::time::Duration;

use a3s_code_core::fact_control::read_workspace_facts;
use support::layer_c_model::load_pinned_layer_c_config;

const MODEL_TIMEOUT: Duration = Duration::from_secs(420);

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live provider configured in .a3s/config.acl"]
async fn real_llm_fact_log_reaches_a_terminal_phase() {
    let config = load_pinned_layer_c_config();
    let agent = a3s_code_core::Agent::from_config(config)
        .await
        .expect("build agent from real config");
    let workspace = tempfile::tempdir().expect("workspace");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                a3s_code_core::SessionOptions::new()
                    .with_session_id("fact-log-live")
                    .with_planning(false)
                    .with_auto_delegation_enabled(false)
                    .with_manual_delegation_enabled(false)
                    .with_temperature(0.0),
            ),
        )
        .await
        .expect("session");

    let sent = tokio::time::timeout(
        MODEL_TIMEOUT,
        session.send(
            "Reply with one short sentence and do not call any tools.",
            None,
        ),
    )
    .await
    .expect("model timed out")
    .expect("send");

    let facts = read_workspace_facts(workspace.path(), "fact-log-live").expect("facts");
    let model_turn = facts.iter().find(|fact| fact.kind == "model.turn");
    assert!(
        model_turn.is_some(),
        "the provider model turn must be stored on the fact log"
    );
    let payload = &model_turn.unwrap().payload;
    assert_eq!(payload["kind"], "text");
    let provider_text = payload["text"].as_str().unwrap_or("");
    assert!(
        !provider_text.is_empty(),
        "the stored model turn must contain provider text"
    );
    eprintln!(
        "fact_log model.turn kind=text chars={} terminal_phase=done",
        provider_text.chars().count()
    );
    assert!(
        !sent.text.is_empty(),
        "a terminal text phase returns the assistant text"
    );

    let resumed = session.resume_run("fact-log-live").await.expect("resume");
    assert_eq!(resumed.text, sent.text);
    let after =
        read_workspace_facts(workspace.path(), "fact-log-live").expect("facts after resume");
    assert_eq!(
        after
            .iter()
            .filter(|fact| fact.kind == "model.turn")
            .count(),
        1,
        "resume must not append another model turn"
    );
}
