//! PAR-ADOPT — parallel-adoption rail on the real model (speed axis).
//!
//! Independent reads are the cheapest parallelizable workload: the
//! harness runs them concurrently when the model emits one decision
//! with several calls (the `also` batch shape; WC-MEAS2 measured the
//! overlap guarantee hermetically). This rail measures how often the
//! REAL model spontaneously adopts that shape, and what one line of
//! guidance buys — two arms over one natural task:
//!
//! - arm `natural`: no choreography instruction at all;
//! - arm `guided`: the same task with one guidelines line advising
//!   independent reads share one turn.
//!
//! Mechanical oracles (record, never assert): per arm — batch turns,
//! total decisions, total read calls, wall time, and whether the sum
//! came back correct (capability rides along: a wrong sum is a failed
//! task no matter how fast). Bash is DENIED in both arms so one shell
//! `cat` cannot stand in for five reads.
//!
//! ```text
//! cargo +1.97.1 test -p a3s-code-core --test test_parallel_adoption_real_llm \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

mod support;

use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{Agent, SessionOptions, SystemPromptSlots};
use support::layer_c_model::load_pinned_layer_c_config;

const REAL_TIMEOUT: Duration = Duration::from_secs(300);
const VALUES: [(&str, i32); 5] = [
    ("alpha", 12),
    ("bravo", 7),
    ("charlie", 33),
    ("delta", 5),
    ("echo", 21),
];
const EXPECTED_SUM: i32 = 78;

const GUIDED_LINE: &str = "When several independent reads would each inform the same next step, issue them together in one assistant turn instead of one per turn.";

fn write_workspace(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("data")).expect("data dir");
    std::fs::write(
        root.join("README.md"),
        "Five data files live under data/.\n",
    )
    .expect("readme");
    for (name, value) in VALUES {
        std::fs::write(
            root.join("data").join(format!("{name}.mjs")),
            format!("export const {name} = {value};\n"),
        )
        .expect("data file");
    }
}

struct ArmReport {
    decisions: usize,
    batch_turns: usize,
    read_calls: usize,
    file_entries: usize,
    correct: bool,
    seconds: f64,
}

async fn run_arm(label: &str, guidelines: Option<&str>) -> ArmReport {
    let config = load_pinned_layer_c_config();
    let model = config
        .default_model
        .clone()
        .expect("loader pins a default model");
    let agent = Agent::from_config(config).await.expect("agent");
    let workspace = tempfile::tempdir().expect("workspace");
    write_workspace(workspace.path());
    let root = workspace.path().to_path_buf();

    let mut policy = PermissionPolicy::new().allow_all(&["ls(*)", "read(*)"]);
    policy.default_decision = PermissionDecision::Deny;

    let mut options = SessionOptions::new()
        .with_session_id(format!("par-adopt-{label}"))
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_permission_policy(policy)
        .with_default_security()
        .with_max_tool_rounds(16)
        .with_llm_api_timeout(120_000)
        .with_temperature(0.0)
        .with_continuation(false);
    if let Some(line) = guidelines {
        options = options.with_prompt_slots(SystemPromptSlots::default().with_guidelines(line));
    }
    let session = agent
        .session_async(root.to_string_lossy().to_string(), Some(options))
        .await
        .expect("session");

    let started = std::time::Instant::now();
    let answer = tokio::time::timeout(
        REAL_TIMEOUT,
        session.send(
            "Report the sum of the exported values across all five data files under data/. \
             Answer with the number alone at the end of one short sentence.",
            None,
        ),
    )
    .await
    .expect("turn timed out")
    .expect("turn failed");
    let seconds = started.elapsed().as_secs_f64();
    let text = answer.text.trim();
    println!("PAR-ADOPT arm={label} reply: {text}");

    let facts =
        a3s_code_core::fact_control::read_workspace_facts(&root, &format!("par-adopt-{label}"))
            .expect("facts");
    let mut decisions = 0usize;
    let mut batch_turns = 0usize;
    let mut read_calls = 0usize;
    let mut file_entries = 0usize;
    for fact in &facts {
        if fact.kind != "model.turn" {
            continue;
        }
        decisions += 1;
        let rendered = fact.payload.to_string();
        let calls = rendered.matches("\"id\"").count();
        if rendered.contains("\"also\"") && calls >= 2 {
            batch_turns += 1;
        }
        read_calls += rendered.matches("\"name\":\"read\"").count()
            + rendered.matches("\"name\": \"read\"").count();
        file_entries += rendered.matches("\"path\":").count();
    }
    let correct = text.contains(&EXPECTED_SUM.to_string());
    drop(session);
    drop(agent);
    let _ = std::fs::remove_dir_all(&root);
    let _ = model;
    ArmReport {
        decisions,
        batch_turns,
        read_calls,
        file_entries,
        correct,
        seconds,
    }
}

#[tokio::test]
#[ignore = "requires the model and credentials from the repo .a3s/config.acl"]
async fn measures_spontaneous_and_guided_parallel_adoption() {
    let natural = run_arm("natural", None).await;
    let guided = run_arm("guided", Some(GUIDED_LINE)).await;

    for (label, report) in [("natural", &natural), ("guided", &guided)] {
        println!(
            "PAR-ADOPT arm={label} decisions={} batch_turns={} read_calls={} file_entries={} correct={} seconds={:.1}",
            report.decisions, report.batch_turns, report.read_calls, report.file_entries, report.correct, report.seconds
        );
    }
    // The rail records; the mechanical floor is that both arms completed
    // a turn and the fact log carries their decisions.
    assert!(natural.decisions >= 1 && guided.decisions >= 1);
}
