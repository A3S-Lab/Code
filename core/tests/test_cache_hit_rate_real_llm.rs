//! WC-MEAS1 — provider cache hit-rate rail on the real model.
//!
//! A stable 6-turn workload with no tools; every turn records the
//! provider-reported prompt/completion/cache tokens from the session usage
//! surface. The mechanical oracle is that every turn reports prompt tokens.
//! Cache fields are recorded, never asserted and never estimated: a
//! provider that does not report cached tokens prints `NOT-REPORTED`.
//!
//! Provider matrix: the pinned flash route by default; a second provider
//! through the existing `A3S_TEST_MODEL` override
//! (e.g. `A3S_TEST_MODEL=deepseek/deepseek-flash`).
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo +1.97.1 test -p a3s-code-core --test test_cache_hit_rate_real_llm \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

mod support;

use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::{Agent, PlanningMode, SessionOptions};
use support::layer_c_model::load_pinned_layer_c_config;

const TURNS: [&str; 6] = [
    "The depot ledger starts with seven copper vats, two brass scales, and one cracked hourglass. The night clerk logged each item twice to be safe. Summarize the opening inventory in one short sentence. Do not use any tools.",
    "On the second day the depot received four crates of glass insulators, rejected one cracked crate, and sent the stationmaster a note about the weather. Summarize the second day in one short sentence. Do not use any tools.",
    "The third day brought a broken dynamo, a borrowed multimeter, and an argument about whether the spare fuse box belongs to the depot or the rail company. Summarize the third day in one short sentence. Do not use any tools.",
    "Fourth day: the auditors arrived, found the double-logged vats, praised the night clerk's thoroughness, and fined the depot for the cracked hourglass anyway. Summarize the fourth day in one short sentence. Do not use any tools.",
    "On the fifth day the depot painted the vats blue, the brass scales green, and the hourglass was finally replaced with a clock nobody could read. Summarize the fifth day in one short sentence. Do not use any tools.",
    "Final day: the ledger was closed with everything accounted for except one borrowed multimeter that the electrician swears he returned. Summarize the whole week in exactly two short sentences. Do not use any tools.",
];

#[tokio::test]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn six_turn_workload_records_per_turn_cache_usage() {
    // The loader applies the `A3S_TEST_MODEL` remap; this rail measures the
    // provider matrix, so it runs whatever route the loader resolved
    // instead of asserting the Layer C flash pin.
    let config = load_pinned_layer_c_config();
    let model = config
        .default_model
        .clone()
        .expect("loader pins a default model");
    let agent = Agent::from_config(config).await.expect("agent");
    let workspace = tempfile::tempdir().expect("workspace");

    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("wc-meas1")
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

    let mut cache_reported_turns = 0usize;
    let mut cache_read_total: u64 = 0;
    let mut prompt_total: u64 = 0;
    for (index, prompt) in TURNS.iter().enumerate() {
        let turn = tokio::time::timeout(Duration::from_secs(240), session.send(prompt, None))
            .await
            .expect("turn timed out")
            .expect("turn failed");
        assert!(
            turn.usage.prompt_tokens > 0,
            "mechanical oracle: turn {} must report prompt tokens",
            index + 1
        );
        prompt_total += turn.usage.prompt_tokens as u64;
        let cache_read = turn.usage.cache_read_tokens;
        if let Some(read) = cache_read {
            if read > 0 {
                cache_reported_turns += 1;
                cache_read_total += read as u64;
            }
        }
        println!(
            "MEAS1 model={model} turn={} prompt_tokens={} completion_tokens={} cache_read={cache_read:?}",
            index + 1,
            turn.usage.prompt_tokens,
            turn.usage.completion_tokens,
        );
    }

    let verdict = if cache_reported_turns == 0 {
        "NOT-REPORTED"
    } else {
        "REPORTED"
    };
    let share = if prompt_total > 0 {
        cache_read_total as f64 / prompt_total as f64
    } else {
        0.0
    };
    println!(
        "MEAS1 model={model} turns=6 cache_reported_turns={cache_reported_turns} \
         cache_read_total={cache_read_total} prompt_total={prompt_total} \
         cache_share={share:.3} verdict={verdict}"
    );
}
