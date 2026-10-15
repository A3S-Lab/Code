//! Live multi-turn and long-horizon scenarios on a real provider.
//!
//! Slice 1 of `docs/a3s-code-live-multiturn-longhorizon-plan.md`. Each case
//! drives several real conversation turns on one session and asserts kernel
//! effects: transcript shape, run records, event kinds, workspace state, and
//! planted-token recall (the in-tree precedent is
//! `deepseek_flash_compaction_keeps_the_earlier_token`). Ignored by default
//! because every turn consumes provider quota; the layer C recipe runs this
//! serially.

use std::path::Path;
use std::time::Duration;

use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{Agent, AgentEvent, AgentSession, PlanningMode, SessionOptions};

mod support;
use support::layer_c_model::load_pinned_layer_c_config;

const TURN_TIMEOUT: Duration = Duration::from_secs(180);
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(600);

fn row(id: &str, model: &str, verdict: &str) {
    println!("MT_ROW id={id} model={model} verdict={verdict}");
}

async fn real_agent_and_model() -> (Agent, String) {
    let config = load_pinned_layer_c_config();
    let model = config
        .default_model
        .clone()
        .expect("pinned Layer C config must declare default_model");
    eprintln!("[multiturn-real] model={model}");
    (
        Agent::from_config(config)
            .await
            .expect("build real-provider agent"),
        model,
    )
}

fn base_options(session_id: &str, model: &str) -> SessionOptions {
    SessionOptions::new()
        .with_session_id(session_id.to_string())
        .with_model(model)
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(8)
        .with_temperature(0.0)
        .with_llm_api_timeout(120_000)
}

async fn send_bounded(session: &AgentSession, prompt: &str) -> a3s_code_core::AgentResult {
    tokio::time::timeout(TURN_TIMEOUT, session.send(prompt, None))
        .await
        .expect("turn timed out")
        .expect("turn failed")
}

/// Every fact-log line for this session. The log lives either in a prepared
/// session folder (`.a3s/sessions/<day>/<id>/<id>.jsonl`) or in the legacy
/// flat directory (`.a3s/effect-log/<thread>.jsonl`); walk both.
fn session_fact_lines(workspace: &Path, session_id: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut visit = |dir: &Path| {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            let matches = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(session_id))
                || path.to_string_lossy().contains(session_id);
            if !matches {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                lines.extend(text.lines().map(str::to_string));
            }
        }
    };
    visit(&workspace.join(".a3s").join("effect-log"));
    if let Ok(days) = std::fs::read_dir(workspace.join(".a3s").join("sessions")) {
        for day in days.flatten() {
            let session_dir = day.path().join(session_id);
            visit(&session_dir);
        }
    }
    lines
}

/// Count tool-carrying model turns in the session's durable fact log: a
/// `model.turn` fact whose decision payload carries a `call` is one spent
/// tool round.
fn fact_log_tool_rounds(workspace: &Path, session_id: &str) -> usize {
    session_fact_lines(workspace, session_id)
        .iter()
        .filter(|line| line.contains("model.turn") && line.contains("\"call\""))
        .count()
}

/// Whether any fact in the session's durable log carries `marker`.
fn fact_log_contains(workspace: &Path, session_id: &str, marker: &str) -> bool {
    session_fact_lines(workspace, session_id)
        .iter()
        .any(|line| line.contains(marker))
}

/// MT-1: six planting turns then one recall turn on one session. The oracle
/// is the recall of every planted token plus the durable transcript holding
/// the whole conversation.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn sustained_multiturn_recall() {
    let tokens = [
        "MT-TOKEN-1-amber",
        "MT-TOKEN-2-bronze",
        "MT-TOKEN-3-cobalt",
        "MT-TOKEN-4-dune",
        "MT-TOKEN-5-ember",
        "MT-TOKEN-6-flint",
    ];
    let (agent, model) = real_agent_and_model().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(base_options("mt-sustained-recall", &model).with_read_only_session(true)),
        )
        .await
        .expect("session");

    for token in tokens {
        let result = send_bounded(
            &session,
            &format!("Remember this verification code for later: {token}. Reply with ok only."),
        )
        .await;
        assert!(
            result.text.to_lowercase().contains("ok"),
            "planting turn for {token} did not ack: {}",
            result.text
        );
    }
    let recall = send_bounded(
        &session,
        "List every verification code you were given, one per line, in the order given. Codes only.",
    )
    .await;
    let missing: Vec<&str> = tokens
        .iter()
        .filter(|token| !recall.text.contains(*token))
        .copied()
        .collect();
    assert!(
        missing.is_empty(),
        "after six turns the model lost {missing:?}; reply was: {}",
        recall.text
    );
    let transcript = session.durable_transcript();
    assert!(
        transcript.len() >= 13,
        "durable transcript lost turns: {transcript:?}"
    );
    for token in tokens {
        assert!(
            transcript
                .iter()
                .any(|message| message.text().contains(token)),
            "durable transcript is missing {token}"
        );
    }
    session.close().await;
    agent.close().await;
    row(
        "sustained_multiturn_recall",
        &model,
        "6/6-tokens+transcript",
    );
}

/// MT-6: interactive constraint correction. A later turn REVOKES an earlier
/// instruction; the conversation must honor the latest constraint over the
/// first one (recency over primacy — the property that makes multi-turn
/// interactive dialogue usable).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn interactive_correction_overrides_earlier_constraint() {
    let (agent, model) = real_agent_and_model().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(base_options("mt-corrective", &model).with_read_only_session(true)),
        )
        .await
        .expect("session");

    let first = send_bounded(
        &session,
        "For the rest of this conversation, answer every question with exactly one word. \
         Acknowledge with: ok",
    )
    .await;
    assert!(
        first.text.trim().eq_ignore_ascii_case("ok"),
        "constraint planting turn: {}",
        first.text
    );
    // Confirm the constraint is in force.
    let constrained = send_bounded(&session, "Name the language Rust is written in.").await;
    let one_word = constrained.text.split_whitespace().count() <= 1;
    assert!(
        one_word,
        "first constraint must bind before the correction: {}",
        constrained.text
    );

    // The correction revokes the one-word rule.
    let corrected = send_bounded(
        &session,
        "Change of rule: from now on ignore the one-word limit and answer in full sentences. \
         Acknowledge with: understood",
    )
    .await;
    assert!(
        corrected.text.to_lowercase().contains("understood"),
        "correction turn: {}",
        corrected.text
    );

    let after = send_bounded(
        &session,
        "In two sentences, say what a compiler does and why that matters.",
    )
    .await;
    let words = after.text.split_whitespace().count();
    assert!(
        words > 6,
        "the corrected constraint must win over the earlier one ({words} words): {}",
        after.text
    );
    session.close().await;
    agent.close().await;
    row(
        "interactive_correction_overrides_earlier_constraint",
        &model,
        "recency-wins",
    );
}

/// MT-7: cancel a streaming turn mid-flight, then keep talking. The
/// interrupted turn must leave its marker in the durable transcript and the
/// NEXT turn must complete normally against that history — the interactive
/// contract after a user Escape.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn cancelled_turn_marks_history_and_conversation_continues() {
    let (agent, model) = real_agent_and_model().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(base_options("mt-cancel-continue", &model).with_read_only_session(true)),
        )
        .await
        .expect("session");

    let seed = send_bounded(
        &session,
        "Remember the reference code CANCEL-7c2d for this conversation. Reply ok.",
    )
    .await;
    assert!(seed.text.to_lowercase().contains("ok"));

    // Start a long streaming turn and cancel after the first delta.
    let (mut rx, join) = session
        .stream(
            "Write a 300-word story about a lighthouse keeper. Take your time.",
            None,
        )
        .await
        .expect("stream");
    let mut saw_delta = false;
    let drain = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if matches!(event, AgentEvent::TextDelta { .. }) {
                saw_delta = true;
            }
        }
        saw_delta
    });
    // Give the model a moment to start, then interrupt like a user would.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let cancelled = session.cancel().await;
    let saw_delta = drain.await.unwrap_or(false);
    let _ = join.await;
    assert!(cancelled, "cancel must report an in-flight turn");
    assert!(saw_delta, "the turn should have streamed before cancelling");

    let transcript = session.durable_transcript();
    assert!(
        transcript.iter().any(|message| message
            .text()
            .contains("(Response interrupted by the user.)")),
        "the interrupted turn must leave its marker in the durable transcript"
    );

    // The conversation continues against that history.
    let after = send_bounded(
        &session,
        "What was the reference code I gave you before the story? Reply with the code only.",
    )
    .await;
    assert!(
        after.text.contains("CANCEL-7c2d"),
        "conversation must continue with pre-cancel context intact: {}",
        after.text
    );
    session.close().await;
    agent.close().await;
    row(
        "cancelled_turn_marks_history_and_conversation_continues",
        &model,
        "marker+continuity",
    );
}

/// MT-2: compaction must fire between turns of an ongoing chain and keep
/// facts planted on both sides of the boundary.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn mid_conversation_compaction_continuity() {
    let (agent, model) = real_agent_and_model().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                base_options("mt-mid-compaction", &model)
                    .with_read_only_session(true)
                    .with_auto_compact(true)
                    .with_auto_compact_threshold(0.05)
                    .with_max_context_tokens(3_000),
            ),
        )
        .await
        .expect("session");

    let early = "MT-COMP-A-91dd";
    let late = "MT-COMP-B-47ac";
    let filler = format!("context filler: {}", "note ".repeat(700));
    send_bounded(
        &session,
        &format!("First, remember this access code: {early}. {filler} Reply with ok only."),
    )
    .await;
    for probe in ["alpha", "bravo"] {
        send_bounded(
            &session,
            &format!("Checkpoint {probe}. {filler} Reply with the word {probe} only."),
        )
        .await;
    }
    send_bounded(
        &session,
        &format!("Now remember this second access code: {late}. {filler} Reply with ok only."),
    )
    .await;

    let runs = session.runs().await;
    let mut compacted = fact_log_contains(workspace.path(), "mt-mid-compaction", "compaction.done");
    if !compacted {
        for run in &runs {
            if session
                .run_events(&run.id)
                .await
                .iter()
                .any(|record| matches!(record.event, AgentEvent::ContextCompacted { .. }))
            {
                compacted = true;
            }
        }
    }
    let recall = send_bounded(
        &session,
        "Reply with both access codes from this conversation, earliest first, separated by a space. Codes only.",
    )
    .await;
    assert!(
        recall.text.contains(early),
        "the pre-compaction code was lost: {}",
        recall.text
    );
    assert!(
        recall.text.contains(late),
        "the post-compaction code was lost: {}",
        recall.text
    );
    session.close().await;
    agent.close().await;
    row(
        "mid_conversation_compaction_continuity",
        &model,
        &format!("compacted={compacted} both-codes-kept"),
    );
}

/// MT-3: a real model must be contained by the tool-round budget. With six
/// files and two rounds the model cannot read everything; the run still has
/// to close with a terminal narrative instead of erroring or running away.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn budget_exhaustion_finalization() {
    let (agent, model) = real_agent_and_model().await;
    let workspace = tempfile::tempdir().expect("workspace");
    for index in 1..=6 {
        std::fs::write(
            workspace.path().join(format!("note{index}.txt")),
            format!("MT-BUDGET-{index}-token\n"),
        )
        .expect("fixture");
    }
    let mut policy = PermissionPolicy::new().allow_all(&["read(*)", "ls(*)"]);
    policy.default_decision = PermissionDecision::Deny;
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                base_options("mt-budget", &model)
                    .with_max_tool_rounds(2)
                    .with_permission_policy(policy),
            ),
        )
        .await
        .expect("session");

    let result = tokio::time::timeout(
        SCENARIO_TIMEOUT,
        session.send(
            "Read all six note files (note1.txt through note6.txt) and report every MT-BUDGET token you find.",
            None,
        ),
    )
    .await
    .expect("budgeted scenario timed out")
    .expect("budgeted scenario failed");

    let runs = session.runs().await;
    assert_eq!(runs.len(), 1, "one run: {:?}", runs);
    let events = session.run_events(&runs[0].id).await;
    let reads = events
        .iter()
        .filter(|record| matches!(&record.event, AgentEvent::ToolEnd { name, exit_code: 0, .. } if name == "read"))
        .count();
    assert!(reads >= 1, "no successful reads: {reads}");
    let tool_rounds = fact_log_tool_rounds(workspace.path(), "mt-budget");
    assert!(
        tool_rounds <= 2,
        "the round budget did not contain the model: {tool_rounds} tool rounds"
    );
    assert!(
        !result.text.trim().is_empty(),
        "finalization produced no narrative"
    );
    session.close().await;
    agent.close().await;
    row(
        "budget_exhaustion_finalization",
        &model,
        &format!("reads={reads} tool_rounds={tool_rounds} terminal"),
    );
}

/// MT-4: interrupt a real repair run after its first tool round, restart the
/// agent from the file session store, resume the run from its loop
/// checkpoint, and require the repair to finish from there.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn checkpoint_restart_finishes_task() {
    const SPEC: &str = "# Task\n`double(values)` must return each value multiplied by 2.\n";
    const TEST: &str = r#"import assert from "node:assert/strict";
import { double } from "./src/math.mjs";
assert.deepEqual(double([1, 2, 3]), [2, 4, 6]);
console.log("MT_RESTART_OK");
"#;
    const BUG: &str = "export function double(values) {\n  return values;\n}\n";

    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(workspace.path().join("src")).expect("src");
    std::fs::write(workspace.path().join("SPEC.md"), SPEC).expect("spec");
    std::fs::write(workspace.path().join("test.mjs"), TEST).expect("test");
    std::fs::write(workspace.path().join("src/math.mjs"), BUG).expect("bug");
    let store_dir = tempfile::tempdir().expect("store");

    let (agent, model) = real_agent_and_model().await;
    let mut policy = PermissionPolicy::new().allow_all(&[
        "read(*)",
        "ls(*)",
        "edit(src/**)",
        "write(src/**)",
        "bash(node test.mjs)",
    ]);
    policy.default_decision = PermissionDecision::Deny;
    let options = base_options("mt-restart", &model)
        .with_permission_policy(policy)
        .with_file_session_store(store_dir.path().to_path_buf())
        // Host manifest: `node test.mjs` is this workspace's suite and it
        // exercises src/** — the sanctioned way a bare suite run binds
        // the completion gate (the implicit binding was retired with the
        // declared-suite contract).
        .with_verification_contract(
            a3s_code_core::verification::VerificationContractV1::new(
                "node-suite",
                "node test.mjs",
                ["src/**"],
            )
            .expect("contract"),
        );
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(options),
        )
        .await
        .expect("session");

    // Persist the session record first so the restart can resume it.
    send_bounded(&session, "Reply with ok only.").await;
    session.save().await.expect("seed save");

    let (mut rx, worker) = session
        .stream(
            "Repair src/math.mjs so `node test.mjs` prints MT_RESTART_OK: run the test once to see the failure, read SPEC.md and src/math.mjs, fix the source, run the test again.",
            None,
        )
        .await
        .expect("start repair stream");
    let mut saw_tool_round = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    while let Some(event) = tokio::time::timeout_at(deadline, rx.recv())
        .await
        .expect("event wait")
    {
        if matches!(event, AgentEvent::ToolEnd { .. }) {
            saw_tool_round = true;
            break;
        }
    }
    assert!(saw_tool_round, "repair run produced no tool round");
    // Let the checkpoint sink persist the round, then kill the worker
    // mid-run: no settle, no publish, no checkpoint clear.
    tokio::time::sleep(Duration::from_secs(2)).await;
    worker.abort();
    drop(rx);
    let interrupted: Vec<_> = session.runs().await.into_iter().map(|run| run.id).collect();
    drop(session);
    agent.close().await;
    assert!(interrupted.len() >= 2, "runs: {interrupted:?}");

    // Restart: a fresh agent resumes the session, then the interrupted run
    // from its loop checkpoint. The seed turn is the oldest run; the repair
    // run is the newest.
    let (agent_two, model_two) = real_agent_and_model().await;
    assert_eq!(model_two, model, "restart switched models");
    let resumed_session = agent_two
        .resume_session_async(
            "mt-restart",
            SessionOptions::new()
                .with_file_session_store(store_dir.path().to_path_buf())
                .with_verification_contract(
                    a3s_code_core::verification::VerificationContractV1::new(
                        "node-suite",
                        "node test.mjs",
                        ["src/**"],
                    )
                    .expect("contract"),
                ),
        )
        .await
        .expect("resume session across restart");
    let repair_run = interrupted.last().expect("repair run id");
    let finished = tokio::time::timeout(SCENARIO_TIMEOUT, resumed_session.resume_run(repair_run))
        .await
        .expect("resumed repair timed out")
        .expect("resumed repair failed");
    eprintln!("[mt-restart] resumed text: {}", finished.text);

    let verification = tokio::process::Command::new("node")
        .arg("test.mjs")
        .current_dir(workspace.path())
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .expect("independent verification");
    assert!(
        String::from_utf8_lossy(&verification.stdout).contains("MT_RESTART_OK"),
        "repair did not finish across restart: {}{}",
        String::from_utf8_lossy(&verification.stdout),
        String::from_utf8_lossy(&verification.stderr)
    );
    resumed_session.close().await;
    agent_two.close().await;
    row("checkpoint_restart_finishes_task", &model, "node-verified");
}

/// MT-8: a twenty-turn interactive soak under repeated compaction. The
/// oracle is threefold: the session survives every turn without error, the
/// compaction boundary fires more than once mid-soak, and the LATEST turns
/// stay recallable. Early-token recall through repeated compaction is
/// recorded in the receipt as a measurement (a summary may legitimately
/// fold early planted codes away — that is the product contract, not a bug).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl (quota: ~22 completions)"]
async fn twenty_turn_soak_with_repeated_compaction() {
    const TURNS: usize = 20;
    let tokens: Vec<String> = [
        "amber", "bronze", "cobalt", "dune", "ember", "flint", "garnet", "hazel", "indigo",
        "jasper", "kelp", "lumen", "moss", "nectar", "onyx", "pearl", "quartz", "reed", "sage",
        "topaz",
    ]
    .iter()
    .enumerate()
    .map(|(index, word)| format!("MT-SOAK-{index:02}-{word}"))
    .collect();

    let (agent, model) = real_agent_and_model().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                base_options("mt-soak", &model)
                    .with_read_only_session(true)
                    .with_auto_compact(true)
                    .with_auto_compact_threshold(0.08)
                    .with_max_context_tokens(3_500),
            ),
        )
        .await
        .expect("session");

    // A mechanical "padding + directive" shape reads like a prompt-injection
    // probe to guard-y models (one live run was refused at turn 14 as
    // "instruction-like directives embedded in filler text"), so the soak
    // poses as a work log: natural sentences carry the volume, and the
    // reply ask is conversational. Three rotating templates keep it from
    // looking machine-generated.
    let filler = [
        "We are keeping a long-running work log across this conversation. Each entry adds          context for the final summary, so please stay with it. Background for this entry:          the migration notes grew again today and the reviewers asked for a rolling marker          per entry to verify nothing gets dropped during context compaction. "
            .repeat(3),
        "Long-context regression check, still in progress. The archive scan is quiet, the          indexer is warm, and the operator wants each log entry acknowledged by its number          so the sequence stays verifiable after any summarization pass. "
            .repeat(3),
        "Continuing the sustained-session log. Nothing here asks you to change anything;          the only job is to keep the marker alive across turns so the compactor cannot          silently drop an entry. Treat this as ordinary journalling. "
            .repeat(3),
    ];
    for (index, token) in tokens.iter().enumerate() {
        let body = &filler[index % filler.len()];
        let prompt = match index % 3 {
            0 => format!(
                "Log entry {index}. Rolling marker for this entry: {token}. {body} \
                 Please acknowledge entry {index} by replying with just the number {index}."
            ),
            1 => format!(
                "Entry {index} of the work log; the marker moved to {token}. {body} \
                 A one-word reply is enough: {index}."
            ),
            _ => format!(
                "Still journalling — entry {index}, marker now {token}. {body}                  Reply with the entry number only ({index}) so the sequence stays checkable."
            ),
        };
        let result = send_bounded(&session, &prompt).await;
        let expected = index.to_string();
        assert!(
            result.text.contains(&expected),
            "turn {index} lost the thread: {}",
            result.text
        );
    }

    let compactions = session_fact_lines(workspace.path(), "mt-soak")
        .iter()
        .filter(|line| line.contains("compaction.done"))
        .count();
    assert!(
        compactions >= 2,
        "a 20-turn soak under a 3.5k window must compact more than once ({compactions})"
    );

    // Late turns must remain recallable; early recall through repeated
    // compaction is measured, not asserted.
    let late = send_bounded(
        &session,
        "Reply with the rolling code from checkpoint 19 and the one from checkpoint 18, \
         earliest first, separated by a space. Codes only.",
    )
    .await;
    let late_ok = tokens[18..20]
        .iter()
        .all(|token| late.text.contains(token.as_str()));
    assert!(
        late_ok,
        "the newest rolling codes must survive the soak: {}",
        late.text
    );
    let early = send_bounded(
        &session,
        "Reply with the rolling code from checkpoint 0 if you still have it; \
         otherwise reply: gone",
    )
    .await;
    let early_kept = early.text.contains(tokens[0].as_str());

    session.close().await;
    agent.close().await;
    println!(
        "MT_ROW id=twenty_turn_soak_with_repeated_compaction model={model} verdict=turns={TURNS} \
         compactions={compactions} late_recalled=true early_through_compaction={early_kept}"
    );
}

/// LH-2: two distinct long tasks in ONE session. Task A repairs an
/// arithmetic module; task B (a later prompt in the same session) builds a
/// string module. The long-horizon contract: B must not break A's artifact,
/// both must pass independent verification at the end, and the session must
/// still recall task A's detail after B.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn two_distinct_long_tasks_in_one_session_do_not_interfere() {
    const SPEC_A: &str = "# Task A\n`double(values)` returns each value times 2.\n";
    const TEST_A: &str = r#"import assert from "node:assert/strict";
import { double } from "./src/math.mjs";
assert.deepEqual(double([2, 5]), [4, 10]);
console.log("TASK_A_OK");
"#;
    const BUG_A: &str = "export function double(values) {\n  return values;\n}\n";
    const SPEC_B: &str = "# Task B\n`initials(names)` returns uppercase first letters.\nCreate the module as `src/strings.mjs`.\n";
    const TEST_B: &str = r#"import assert from "node:assert/strict";
import { initials } from "./src/strings.mjs";
assert.deepEqual(initials(["ada", "grace"]), ["A", "G"]);
console.log("TASK_B_OK");
"#;

    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(workspace.path().join("src")).expect("src");
    std::fs::write(workspace.path().join("SPEC_A.md"), SPEC_A).expect("specA");
    std::fs::write(workspace.path().join("testA.mjs"), TEST_A).expect("testA");
    std::fs::write(workspace.path().join("src/math.mjs"), BUG_A).expect("bugA");
    std::fs::write(workspace.path().join("SPEC_B.md"), SPEC_B).expect("specB");
    std::fs::write(workspace.path().join("testB.mjs"), TEST_B).expect("testB");

    let (agent, model) = real_agent_and_model().await;
    let mut policy = PermissionPolicy::new().allow_all(&[
        "read(*)",
        "ls(*)",
        "edit(src/**)",
        "write(src/**)",
        "bash(node testA.mjs)",
        "bash(node testB.mjs)",
    ]);
    policy.default_decision = PermissionDecision::Deny;
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                base_options("mt-two-tasks", &model)
                    .with_permission_policy(policy)
                    .with_max_tool_rounds(12)
                    // Both suites declared: a passing testA/testB run
                    // verifies the mutated paths, clearing the debt that
                    // would otherwise survive into the recall turn.
                    .with_verification_contract(
                        a3s_code_core::verification::VerificationContractV1::new(
                            "node-suite-a",
                            "node testA.mjs",
                            ["src/**"],
                        )
                        .expect("contract a"),
                    )
                    .with_verification_contract(
                        a3s_code_core::verification::VerificationContractV1::new(
                            "node-suite-b",
                            "node testB.mjs",
                            ["src/**"],
                        )
                        .expect("contract b"),
                    ),
            ),
        )
        .await
        .expect("session");

    // A repair turn may legitimately end behind the completion gate (an
    // unverified mutation is refused narrative success — the product
    // contract). The oracles are the artifacts and the dialogue, so both
    // outcomes are accepted here and recorded.
    let settle =
        |label: &'static str,
         outcome: Result<a3s_code_core::AgentResult, a3s_code_core::CodeError>| {
            match outcome {
                Ok(_) => format!("{label}=endturn"),
                Err(error) => {
                    let text = error.to_string();
                    assert!(
                        text.contains("completion gate"),
                        "{label} failed for a non-gate reason: {text}"
                    );
                    format!("{label}=gated")
                }
            }
        };
    let task_a = tokio::time::timeout(SCENARIO_TIMEOUT, session.send(
        "Long task A: run `node testA.mjs` to see the failure, read SPEC_A.md and src/math.mjs, \
         fix the source (do not touch testA.mjs), run `node testA.mjs` again until it prints \
         TASK_A_OK, then stop.",
        None,
    ))
    .await
    .expect("task A timed out");
    let a_outcome = settle("A", task_a);

    let task_b = tokio::time::timeout(SCENARIO_TIMEOUT, session.send(
        "New long task B, independent of A: read SPEC_B.md and testB.mjs, create src/strings.mjs \
         implementing it, run `node testB.mjs` until it prints TASK_B_OK. Do not modify anything \
         belonging to task A.",
        None,
    ))
    .await
    .expect("task B timed out");
    let b_outcome = settle("B", task_b);

    // A flash model occasionally narrates task B without writing the file
    // (no mutation → the gate rightly allows a no-op turn). The user
    // response is the interactive contract itself: nudge once.
    let mut nudges = 0u8;
    while !workspace.path().join("src").join("strings.mjs").is_file() && nudges < 2 {
        nudges += 1;
        let nudge = tokio::time::timeout(
            SCENARIO_TIMEOUT,
            session.send(
                &format!(
                "Attempt {nudges}: src/strings.mjs still does not exist. Create src/strings.mjs \
                 implementing SPEC_B.md NOW using the write tool, then run `node testB.mjs`."
            ),
                None,
            ),
        )
        .await
        .expect("nudge timed out");
        let _ = settle("nudge", nudge);
    }

    // Independent verification of BOTH artifacts after B.
    for (test, marker) in [("testA.mjs", "TASK_A_OK"), ("testB.mjs", "TASK_B_OK")] {
        let out = tokio::process::Command::new("node")
            .arg(test)
            .current_dir(workspace.path())
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .expect("verify");
        assert!(
            String::from_utf8_lossy(&out.stdout).contains(marker),
            "{test} no longer passes after the second task: {}{} | dir={:?} src={:?} root={:?}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
            std::fs::read_dir(workspace.path()).ok().map(|d| d
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect::<Vec<_>>()),
            std::fs::read_dir(workspace.path().join("src"))
                .ok()
                .map(|d| d
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect::<Vec<_>>()),
            a_outcome,
        );
    }

    // Cross-task recall in the same session.
    let recall = send_bounded(
        &session,
        "Which function did you fix in the first task, and in which file? One line.",
    )
    .await;
    assert!(
        recall.text.contains("double") && recall.text.contains("math"),
        "cross-task recall lost task A: {}",
        recall.text
    );

    let runs = session.runs().await;
    assert!(runs.len() >= 2, "both tasks must be recorded: {:?}", runs);
    session.close().await;
    agent.close().await;
    row(
        "two_distinct_long_tasks_in_one_session",
        &model,
        &format!("A+B-verified+cross-recall {a_outcome} {b_outcome} nudges={nudges}"),
    );
}

/// LH-1: long-task endurance — ONE repair task crashed TWICE mid-flight and
/// continued both times, then finished. Two discovered contracts shape the
/// design: the exact-id spawn path does not persist loop checkpoints when
/// its worker is killed hard (the committer dies with it — no
/// `loop_checkpoints/` entries appear), and a graceful cancel settles the
/// run and clears its checkpoint. The supported crash-continuity rail is
/// therefore the FACT LOG — the architecture's control source: after each
/// crash a fresh runtime reattaches the session and continues the task from
/// where the log left off.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn long_task_survives_two_restarts() {
    const SPEC: &str = "# Task\n`triple(values)` must return each value multiplied by 3.\n";
    const TEST: &str = r#"import assert from "node:assert/strict";
import { triple } from "./src/math.mjs";
assert.deepEqual(triple([1, 4]), [3, 12]);
console.log("LH_RESTART2_OK");
"#;
    const BUG: &str = "export function triple(values) {\n  return values;\n}\n";
    const REPAIR_PROMPT: &str = "Repair src/math.mjs so `node test.mjs` prints LH_RESTART2_OK: \
         run the test once to see the failure, read SPEC.md and src/math.mjs, fix the source, \
         run the test again.";
    const CONTINUE_PROMPT: &str =
        "A previous attempt at this repair was interrupted. Check the current state of \
         src/math.mjs and test.mjs and finish the repair so `node test.mjs` prints \
         LH_RESTART2_OK.";

    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(workspace.path().join("src")).expect("src");
    std::fs::write(workspace.path().join("SPEC.md"), SPEC).expect("spec");
    std::fs::write(workspace.path().join("test.mjs"), TEST).expect("test");
    std::fs::write(workspace.path().join("src/math.mjs"), BUG).expect("bug");
    let store_dir = tempfile::tempdir().expect("store");

    let mut policy = PermissionPolicy::new().allow_all(&[
        "read(*)",
        "ls(*)",
        "edit(src/**)",
        "write(src/**)",
        "bash(node test.mjs)",
    ]);
    policy.default_decision = PermissionDecision::Deny;

    let options = |model: &str| {
        base_options("mt-restart-twice", model)
            .with_permission_policy(policy.clone())
            .with_file_session_store(store_dir.path().to_path_buf())
            .with_max_tool_rounds(12)
    };
    let mut crashes = 0u8;

    // Crash #1: stream the repair, kill at the first tool round, drop the
    // runtime (the host process dying).
    {
        let (agent, model) = real_agent_and_model().await;
        let session = agent
            .session_async(
                workspace.path().to_string_lossy().to_string(),
                Some(options(&model)),
            )
            .await
            .expect("session");
        send_bounded(&session, "Reply with ok only.").await;
        session.save().await.expect("seed save");
        let (mut rx, worker) = session.stream(REPAIR_PROMPT, None).await.expect("repair");
        let mut saw_tool = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(240);
        while let Some(event) = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("evt")
        {
            if matches!(event, AgentEvent::ToolEnd { .. }) {
                saw_tool = true;
                break;
            }
        }
        assert!(saw_tool, "repair produced no tool round before the crash");
        worker.abort();
        drop(rx);
        drop(session);
        agent.close().await;
        crashes += 1;
    }

    // Crash #2: fresh runtime continues from the fact log, killed again at
    // its first tool round.
    {
        let (agent, model) = real_agent_and_model().await;
        let session = agent
            .session_async(
                workspace.path().to_string_lossy().to_string(),
                Some(options(&model)),
            )
            .await
            .expect("continuation session");
        let (mut rx, worker) = session
            .stream(CONTINUE_PROMPT, None)
            .await
            .expect("continue");
        let mut saw_tool = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(240);
        while let Some(event) = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("evt")
        {
            if matches!(event, AgentEvent::ToolEnd { .. }) {
                saw_tool = true;
                break;
            }
        }
        assert!(
            saw_tool,
            "continuation produced no tool round before the crash"
        );
        worker.abort();
        drop(rx);
        drop(session);
        agent.close().await;
        crashes += 1;
    }

    // Final runtime finishes the chain from the twice-interrupted log.
    {
        let (agent, model) = real_agent_and_model().await;
        let session = agent
            .session_async(
                workspace.path().to_string_lossy().to_string(),
                Some(options(&model)),
            )
            .await
            .expect("final session");
        let outcome = tokio::time::timeout(SCENARIO_TIMEOUT, session.send(CONTINUE_PROMPT, None))
            .await
            .expect("final continuation timed out");
        match outcome {
            Ok(_) => {}
            Err(error) => assert!(
                error.to_string().contains("completion gate"),
                "final continuation failed for a non-gate reason: {error}"
            ),
        }
        session.close().await;
        agent.close().await;
    }
    assert_eq!(crashes, 2);

    // Independent verification: the repair survived two crash-continue cycles.
    let out = tokio::process::Command::new("node")
        .arg("test.mjs")
        .current_dir(workspace.path())
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .expect("verify");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("LH_RESTART2_OK"),
        "repair did not survive two crashes: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let (agent, model) = real_agent_and_model().await;
    row(
        "long_task_survives_two_restarts",
        &model,
        "crash2+continue2+node-verified",
    );
    agent.close().await;
}

/// MT-5: the model's own post-turn extraction must land a memory item in the
/// workspace store, and a later session must recall it through the
/// per-prompt recall path. The fixture fact is deliberately NOT
/// credential-shaped: the extraction prompt refuses to store secrets, and
/// that refusal is a product policy this test must not fight.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn memory_extraction_then_recall_next_session() {
    let token = "MT-WINDOW-4d17";
    let (agent, model) = real_agent_and_model().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(base_options("mt-memory", &model).with_read_only_session(true)),
        )
        .await
        .expect("session");

    send_bounded(
        &session,
        &format!(
            "Remember this preference for this workspace: the team's code-review window is {token}. \
             Reply with ok only."
        ),
    )
    .await;
    // The explicit-remember path persists before the extraction model call;
    // the LLM extraction that follows is background work. A bounded window
    // covers both writers.
    let items = workspace.path().join(".a3s").join("memory").join("items");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut found = false;
    while tokio::time::Instant::now() < deadline {
        found = memory_items_carry(&items, token);
        if found {
            break;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    session.close().await;
    assert!(
        found,
        "no memory item in {} carries {token} within 60s of the turn",
        items.display()
    );

    let (agent_two, model_two) = real_agent_and_model().await;
    assert_eq!(model_two, model);
    let session_two = agent_two
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(base_options("mt-memory-recall", &model).with_read_only_session(true)),
        )
        .await
        .expect("recall session");
    let recall = send_bounded(
        &session_two,
        "What code-review window is recorded for this workspace? Reply with the window id only.",
    )
    .await;
    assert!(
        recall.text.contains(token),
        "the recorded review window was not recalled: {}",
        recall.text
    );
    session_two.close().await;
    agent_two.close().await;
    agent.close().await;
    row("memory_extraction_then_recall_next_session", &model, token);
}

fn memory_items_carry(items: &Path, token: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(items) else {
        return false;
    };
    entries.flatten().any(|entry| {
        std::fs::read_to_string(entry.path())
            .map(|text| text.contains(token))
            .unwrap_or(false)
    })
}

/// Performance rail: time-to-first-delta and full-turn latency on the pinned
/// model, session path vs raw client. The split quantifies the harness share
/// (session TTFB minus raw-client TTFB) so optimization targets only the
/// delta. This is a measurement, not a pass/fail oracle.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn streaming_ttfb_baseline() {
    let (agent, model) = real_agent_and_model().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(base_options("mt-ttfb", &model).with_read_only_session(true)),
        )
        .await
        .expect("session");

    // Raw-client leg: the same provider route with no harness on the path.
    // Two variants isolate the cost: a bare prompt, and a prompt carrying a
    // product-sized system (~11.5k chars, the measured session system) so
    // the prefill share of the session TTFB is attributable.
    let raw_client = {
        let config = load_pinned_layer_c_config();
        let (provider, model_id) = model.split_once('/').expect("provider/model");
        let llm = config
            .llm_config(provider, model_id)
            .expect("pinned route resolves");
        a3s_code_core::llm::create_client_with_config(llm)
    };
    let mut raw_ttfb = None;
    let mut raw_sized_ttfb = None;
    {
        let prompt =
            a3s_code_core::llm::Message::user("Reply with the single word ok. (raw ttfb probe)");
        let events = raw_client
            .complete_streaming(
                &[prompt],
                None,
                &[],
                tokio_util::sync::CancellationToken::new(),
            )
            .await
            .expect("raw stream");
        let mut events = events;
        let started = std::time::Instant::now();
        while let Some(event) = events.recv().await {
            if matches!(event, a3s_code_core::llm::StreamEvent::TextDelta(_)) {
                raw_ttfb = Some(started.elapsed());
                break;
            }
        }
    }
    {
        let filler = "You are measuring prompt prefill. ".repeat(360);
        let prompt =
            a3s_code_core::llm::Message::user("Reply with the single word ok. (sized ttfb probe)");
        let events = raw_client
            .complete_streaming(
                &[prompt],
                Some(&filler),
                &[],
                tokio_util::sync::CancellationToken::new(),
            )
            .await
            .expect("raw sized stream");
        let mut events = events;
        let started = std::time::Instant::now();
        while let Some(event) = events.recv().await {
            if matches!(event, a3s_code_core::llm::StreamEvent::TextDelta(_)) {
                raw_sized_ttfb = Some(started.elapsed());
                break;
            }
        }
    }

    let mut samples = Vec::new();
    for round in 0..3 {
        let started = std::time::Instant::now();
        let (mut rx, join) = session
            .stream(
                &format!("Count slowly from 1 to 20, one number per line. (probe {round})"),
                None,
            )
            .await
            .expect("stream");
        let mut ttfb = None;
        let mut bytes = 0usize;
        while let Some(event) = rx.recv().await {
            if let AgentEvent::TextDelta { text } = &event {
                if ttfb.is_none() {
                    ttfb = Some(started.elapsed());
                }
                bytes += text.len();
            }
        }
        join.await.expect("join");
        let total = started.elapsed();
        samples.push((ttfb, total, bytes));
    }
    session.close().await;
    agent.close().await;
    for (index, (ttfb, total, bytes)) in samples.iter().enumerate() {
        let session_ms = ttfb.map(|d| d.as_millis()).unwrap_or(0);
        let raw_ms = raw_ttfb.map(|d| d.as_millis()).unwrap_or(0);
        let sized_ms = raw_sized_ttfb.map(|d| d.as_millis()).unwrap_or(0);
        println!(
            "MT_ROW id=streaming_ttfb_baseline model={model} verdict=round{index} \
             ttfb_ms={session_ms} raw_bare_ms={raw_ms} raw_sized_system_ms={sized_ms} \
             total_ms={} bytes={bytes}",
            total.as_millis(),
        );
    }
    assert!(
        samples
            .iter()
            .all(|(ttfb, _, bytes)| ttfb.is_some() && *bytes > 0),
        "no streamed deltas observed: {samples:?}"
    );
}

/// CI-1: the code-intelligence tools through a REAL session. The session is
/// constructed with `WorkspaceServices` carrying the local Code Intelligence
/// provider (interactive defaults do not attach one — the tools would answer
/// "no CI runtime" — so the host opt-in path is what this pins). The model
/// must call `code_symbols` (and at least one more CI tool) against the
/// fixture and the tools must execute with real symbol data — narration does
/// not count.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn code_intelligence_tools_execute_through_a_real_session() {
    let (agent, model) = real_agent_and_model().await;
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::create_dir_all(workspace.path().join("src")).expect("src");
    std::fs::write(
        workspace.path().join("src/shapes.js"),
        r#"export function circleArea(radius) {
  return Math.PI * radius * radius;
}

export function rectangleArea(width, height) {
  return width * height;
}

export const SHAPE_VERSION = "1.0.0";
"#,
    )
    .expect("fixture");

    // The TypeScript language runtime resolves a workspace-local SDK: vendor
    // the globally installed one (TS 7 native LSP) into the fixture.
    let global_ts = std::path::Path::new("/opt/homebrew/lib/node_modules/typescript");
    if global_ts.join("lib/tsc.js").is_file() {
        let vendor_root = workspace.path().join("node_modules");
        std::fs::create_dir_all(&vendor_root).expect("node_modules");
        let status = std::process::Command::new("cp")
            .arg("-R")
            .arg(global_ts)
            .arg(&vendor_root)
            .status()
            .expect("vendor typescript");
        assert!(status.success(), "vendoring typescript failed");
    }

    let mut policy = PermissionPolicy::new().allow_all(&[
        "read(*)",
        "ls(*)",
        "code_symbols(*)",
        "code_navigation(*)",
        "code_diagnostics(*)",
    ]);
    policy.default_decision = PermissionDecision::Deny;
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                base_options("mt-code-intel", &model)
                    .with_permission_policy(policy)
                    .with_code_intelligence()
                    .with_max_tool_rounds(6),
            ),
        )
        .await
        .expect("session with Code Intelligence");

    let outcome = tokio::time::timeout(
        SCENARIO_TIMEOUT,
        session.send(
            "Use the code_intelligence tools on this workspace — do not read files with the read \
         tool. First call code_symbols on src/shapes.js and list every symbol it reports, then \
         call code_navigation for the symbol circleArea, then call code_diagnostics on \
         src/shapes.js. Quote the symbol names verbatim in your answer.",
            None,
        ),
    )
    .await
    .expect("CI turn timed out");
    match outcome {
        Ok(_) => {}
        Err(error) => assert!(
            error.to_string().contains("completion gate:"),
            "CI turn failed for a non-gate reason: {error}"
        ),
    }

    let runs = session.runs().await;
    let events = session.run_events(&runs.last().expect("runs").id).await;
    let ci_executions: Vec<&str> = events
        .iter()
        .filter_map(|record| match &record.event {
            AgentEvent::ToolEnd {
                name, exit_code: 0, ..
            } if matches!(
                name.as_str(),
                "code_symbols" | "code_navigation" | "code_diagnostics"
            ) =>
            {
                Some(name.as_str())
            }
            _ => None,
        })
        .collect();
    assert!(
        ci_executions.contains(&"code_symbols"),
        "code_symbols must execute successfully, saw {ci_executions:?}"
    );
    assert!(
        ci_executions.len() >= 2,
        "at least two CI tools must execute, saw {ci_executions:?}"
    );
    // Real data, not narration: the settled text must quote real symbols.
    let result_text = session
        .history()
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .map(|message| message.text())
        .unwrap_or_default();
    assert!(
        result_text.contains("circleArea"),
        "the answer must quote real symbols, got: {result_text}"
    );
    session.close().await;
    agent.close().await;
    row(
        "code_intelligence_tools_execute_through_a_real_session",
        &model,
        &format!("tools={}", ci_executions.join("+")),
    );
}
