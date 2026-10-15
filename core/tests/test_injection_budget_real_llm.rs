//! WC-INJECT1 — path-instruction budget decision, measured on the real
//! model.
//!
//! One path rule targets `src/**`; its tail (past the 4 KiB default cut)
//! carries a codename. Turn 1 reads a file under `src/` so the turn targets
//! a matching path; turn 2 asks for the codename sentence with no tools.
//!
//! Oracles:
//! - Mechanical (asserted): the full-budget arm pays more total tokens —
//!   the tail bytes the cut dropped were delivered to the provider
//!   (delivery oracle; the content oracle is hermetic
//!   `path_instructions_fact.rs`).
//! - Structural (asserted): the cut arm cannot name the codename — the
//!   4 KiB cap drops it before the wire.
//! - Recorded, not asserted: the full arm's recall answer. Four live
//!   protocols on the pinned flash route showed the injected fragment's
//!   `digest=` header shape is presence-detectable but not reliably
//!   content-extractable for the model — the measured WC-INJECT1 decision
//!   is "keep the 4 KiB default; a format change is the prerequisite for
//!   any raise", so the recall answer is a recorded datum, not a gate.
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo +1.97.1 test -p a3s-code-core --test test_injection_budget_real_llm \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

mod support;

use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::path_instructions::PathRule;
use a3s_code_core::{Agent, PlanningMode, SessionOptions};
use support::layer_c_model::load_pinned_layer_c_config;

const CODENAME: &str = "XYLOPHONE-4471";

fn rule_text() -> String {
    // Varied, non-repeating guidance past the 4 KiB cut; the codename
    // sentence sits in the tail so only a larger budget keeps it.
    let lines = [
        "Copper vats are inventoried before brass scales each quarter.",
        "The night clerk signs the ledger in blue ink, never pencil.",
        "Spare fuse boxes belong to the depot until the rail company countersigns.",
        "Broken dynamos wait in the east bay for the borrowed multimeter.",
        "The auditors praise double-logged vats and fine cracked hourglasses.",
        "Paint stocks rotate blue for vats and green for scales only.",
        "The unreadable clock replaced the hourglass; both stay in the case.",
        "Borrowed tools need a return slip filed the same shift.",
        "Insulator crates are rejected when any straw packing is damp.",
        "Stationmaster notes about weather go to the archive box, not the ledger.",
        "Multimeter calibration happens monthly, on the first quiet Tuesday.",
        "The depot map on the wall is authoritative over any printed copy.",
        "Fuel for the lamp room is ordered in tins of twelve, never drums.",
        "Lost return slips are refiled with the electrician as witness.",
        "Winter stock takes precede every other depot activity in December.",
        "The west door stays chained except during deliveries and audits.",
        "Every fine paid is recorded against the item that earned it.",
        "The ledger closes at the last day's bell; late entries wait for morning.",
    ];
    // The codename line sits immediately past the 4 KiB cut: the cut arm
    // structurally cannot carry it, and the full block stays inside the
    // ~4 KiB regime a first live pass proved flash reads as content (a
    // 5 KiB repeated-line wall was dismissed as metadata — recorded in
    // the WC-INJECT1 evidence).
    let mut text = String::from("Workspace path instructions for src work:\n");
    let mut index = 0usize;
    while text.len() < 4 * 1024 {
        text.push_str(&format!(
            "{} (rule {} of the depot ledger.)\n",
            lines[index % lines.len()],
            index
        ));
        index += 1;
    }
    text.push_str(&format!(
        "The workspace codename for src work is {CODENAME}.\n"
    ));
    text
}

struct ArmOutcome {
    answer: String,
    tokens_paid: u64,
}

async fn run_arm(agent: &Agent, label: &str, budget: Option<usize>) -> ArmOutcome {
    let workspace = tempfile::tempdir().expect("workspace");
    let src = workspace.path().join("src");
    std::fs::create_dir_all(&src).expect("src dir");
    std::fs::write(src.join("notes.txt"), "depot notes: seven copper vats\n").expect("notes file");

    let mut options = SessionOptions::new()
        .with_session_id(format!("wc-inject1-{label}"))
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_planning(false)
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_llm_api_timeout(180_000)
        .with_path_rules(vec![PathRule {
            glob: "src/**".into(),
            text: rule_text(),
        }]);
    if let Some(budget) = budget {
        options = options.with_injection_budget(budget);
    }

    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(options),
        )
        .await
        .expect("session");

    let read_turn = tokio::time::timeout(
        Duration::from_secs(240),
        session.send(
            "Read the file src/notes.txt and reply with its exact contents. \
             Do not use any other tools.",
            None,
        ),
    )
    .await
    .expect("read turn timed out")
    .expect("read turn failed");
    assert!(read_turn.text.contains("seven copper vats"));

    let recall = tokio::time::timeout(
        Duration::from_secs(240),
        session.send(
            "The workspace path instructions in this conversation contain a \
             sentence that starts with 'The workspace codename'. Reply with \
             that whole sentence, verbatim. If no such sentence exists, reply \
             NONE. Do not use any tools.",
            None,
        ),
    )
    .await
    .expect("recall turn timed out")
    .expect("recall turn failed");

    let tokens_paid = (read_turn.usage.prompt_tokens
        + read_turn.usage.completion_tokens
        + recall.usage.prompt_tokens
        + recall.usage.completion_tokens) as u64;
    println!(
        "INJECT1 arm={label} budget={:?} recall_prompt_tokens={} tokens_paid={tokens_paid} answer={}",
        budget,
        recall.usage.prompt_tokens,
        recall.text.trim()
    );
    ArmOutcome {
        answer: recall.text,
        tokens_paid,
    }
}

#[tokio::test]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn budget_cut_hides_the_tail_and_full_budget_pays_for_recall() {
    let mut config = load_pinned_layer_c_config();
    config.default_model = Some(support::layer_c_model::pinned_layer_c_model(&config));
    let agent = Agent::from_config(config).await.expect("agent");

    let cut = run_arm(&agent, "cut", Some(4096)).await;
    assert!(
        !cut.answer.contains(CODENAME),
        "the 4 KiB cut must drop the tail codename; answer was: {}",
        cut.answer.trim()
    );

    // Full arm: recorded, not asserted (see module docs). The mechanical
    // delivery oracle below plus the hermetic content pin carry the gate.
    let full = run_arm(&agent, "full", Some(16 * 1024)).await;
    println!(
        "INJECT1 full-arm recall recorded: contains_codename={}",
        full.answer.contains(CODENAME)
    );

    // Mechanical delivery oracle: session usage accumulates across turns,
    // so the comparable figure is total tokens paid — the full arm carried
    // the tail bytes the cut arm dropped.
    assert!(
        full.tokens_paid > cut.tokens_paid,
        "the full arm must pay more tokens overall (full={}, cut={})",
        full.tokens_paid,
        cut.tokens_paid
    );

    let paid = full.tokens_paid.saturating_sub(cut.tokens_paid);
    println!(
        "INJECT1 verdict: tail recall bought for {paid} extra tokens \
         (cut={}, full={})",
        cut.tokens_paid, full.tokens_paid
    );
}
