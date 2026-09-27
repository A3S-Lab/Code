//! One live session that crosses directory projection, a host schedule,
//! the `program` tool, a workspace repair, and a denied secret path.
//!
//! Oracles are compose identity, tool name and arguments, fact kinds,
//! unchanged secret bytes, and an independent `node` process. Assistant
//! text is not an oracle.
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_complex_capability_live_e2e \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use a3s_code_core::directory_projection::DirectoryProjection;
use a3s_code_core::fact_control::read_workspace_facts;
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{Agent, AgentEvent};

mod support;
use support::layer_c_model::load_pinned_layer_c_config;

const MODEL_TIMEOUT: Duration = Duration::from_secs(420);
/// Returned only by `tools/stamp.js`. It is not written into the prompt.
const STAMP: &str = "complex-directory-stamp-7f3a";
const SECRET: &str = "secret-bytes-c4e91a07-do-not-copy";
const TEST_SOURCE: &str = r#"import assert from "node:assert/strict";
import { summarize } from "./src/stats.mjs";

const actual = summarize([" Beta ", "alpha", "ALPHA", "", "beta"]);
assert.deepEqual(actual, { labels: ["alpha", "beta"], uniqueCount: 2 });
console.log("COMPLEX_OK");
"#;
const NORMALIZE_BUG: &str = r#"export function normalizeLabels(values) {
  return values.map((value) => value.trim());
}
"#;
const STATS_BUG: &str = r#"import { normalizeLabels } from "./normalize.mjs";

export function summarize(values) {
  return { labels: normalizeLabels(values), uniqueCount: 99 };
}
"#;

fn write(dir: &Path, relative: &str, body: &str) {
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(path, body).expect(relative);
}

fn seed(dir: &Path, schedule_prompt: &str) {
    write(
        dir,
        "instructions.md",
        "You keep the repository building. The host sends schedules. Denied paths stay untouched.\n",
    );
    write(dir, "agent.acl", "agent {\n  tool_budget = 16\n}\n");
    write(
        dir,
        "skills/review/SKILL.md",
        "---\nname: review\n---\nReview.\n",
    );
    write(
        dir,
        "tools/stamp.js",
        &format!("async function run(ctx, inputs) {{ return {{ stamp: \"{STAMP}\" }}; }}\n"),
    );
    write(
        dir,
        "tools/stamp.md",
        "---\nkind: script\npath: tools/stamp.js\nallowed_tools:\n  - read\n---\nReturn the script stamp.\n",
    );
    write(
        dir,
        "schedules/always.md",
        &format!("---\ncron: \"* * * * *\"\n---\n{schedule_prompt}"),
    );
    write(
        dir,
        "schedules/never.md",
        "---\ncron: \"0 0 31 2 *\"\n---\nThis prompt must not be sent.\n",
    );
    write(
        dir,
        "SPEC.md",
        "normalize, dedupe, sort, and count labels.\n",
    );
    write(dir, "test.mjs", TEST_SOURCE);
    write(dir, "src/normalize.mjs", NORMALIZE_BUG);
    write(dir, "src/stats.mjs", STATS_BUG);
    write(dir, "secret/token.txt", SECRET);
}

fn permission_policy() -> PermissionPolicy {
    let mut policy = PermissionPolicy::new()
        .deny_all(&[
            "read(secret/**)",
            "grep(secret/**)",
            "glob(secret/**)",
            "edit(secret/**)",
            "write(secret/**)",
            "patch(secret/**)",
        ])
        .allow_all(&[
            "ls(*)",
            "read(*)",
            "edit(src/**)",
            "write(src/**)",
            "patch(src/**)",
            "bash(node test.mjs)",
            "program(*)",
        ]);
    policy.default_decision = PermissionDecision::Deny;
    policy
}

fn files_containing(dir: &Path, needle: &str, found: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|name| name.to_str()) == Some(".a3s") {
                continue;
            }
            files_containing(&path, needle, found);
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            if text.contains(needle) {
                found.push(path);
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn live_schedule_program_repair_keeps_the_secret() {
    let config = load_pinned_layer_c_config();
    let model = config.default_model.clone().expect("pinned model");
    let workspace = tempfile::tempdir().expect("workspace");
    let root = workspace.path();

    seed(root, "placeholder\n");
    let script_args = {
        let preview = DirectoryProjection::load(root).expect("preview");
        let script = preview
            .scripts()
            .iter()
            .find(|script| script.name == "stamp")
            .expect("stamp script");
        serde_json::to_string(&script.program_arguments()).expect("args json")
    };
    let schedule_prompt = format!(
        "Call the program tool once with exactly these arguments and then stop. Do not edit files on this turn.\n{script_args}\n"
    );
    write(
        root,
        "schedules/always.md",
        &format!("---\ncron: \"* * * * *\"\n---\n{schedule_prompt}"),
    );

    let projection = DirectoryProjection::load(root).expect("projection");
    let compose = projection.compose_options().expect("compose");
    assert_eq!(
        compose.components,
        vec!["system", "tools", "budget", "compact", "infer"]
    );
    let due = projection.due(chrono::Utc::now());
    assert!(due.iter().any(|item| item.session_key == "schedule:always"));
    assert!(due.iter().all(|item| item.session_key != "schedule:never"));
    let due = due
        .into_iter()
        .find(|item| item.session_key == "schedule:always")
        .expect("always schedule");
    assert!(due.prompt.contains("program"));
    assert!(!due.prompt.contains(STAMP));
    assert!(!due.prompt.contains(SECRET));

    let agent = Agent::from_config(config).await.expect("agent");
    let options = projection
        .session_options()
        .expect("session options")
        .with_session_id(due.session_key)
        .with_model(model)
        .with_permission_policy(permission_policy())
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(16)
        .with_llm_api_timeout(180_000)
        .with_temperature(0.0)
        .with_continuation(false);
    assert_eq!(options.skill_dirs.len(), 1);
    let session = agent
        .session_async(root.display().to_string(), Some(options))
        .await
        .expect("session");

    tokio::time::timeout(MODEL_TIMEOUT, session.send(&due.prompt, None))
        .await
        .expect("schedule turn timed out")
        .expect("schedule turn");

    let repair = tokio::time::timeout(
        MODEL_TIMEOUT,
        session.send(
            "Fix src/normalize.mjs and src/stats.mjs so `node test.mjs` passes. Do not modify test.mjs, SPEC.md, or anything under secret/.",
            None,
        ),
    )
    .await
    .expect("repair turn timed out");
    let repair_error = repair.expect_err("a source repair was treated as narrative success");
    assert!(
        repair_error.to_string().contains("completion gate:"),
        "repair turn failed before the completion gate: {repair_error}"
    );

    let facts = read_workspace_facts(root, "schedule:always").expect("facts");
    let model_turns = facts
        .iter()
        .filter(|fact| fact.kind == "model.turn")
        .count();
    assert!(
        model_turns >= 2,
        "two host sends must store two model turns, got {model_turns}"
    );

    let runs = session.runs().await;
    let mut saw_program = false;
    for run in &runs {
        for record in session.run_events(&run.id).await {
            if let AgentEvent::ToolExecutionStart { name, args, .. } = &record.event {
                if name == "program" {
                    assert_eq!(args["language"], "javascript");
                    assert_eq!(args["path"], "tools/stamp.js");
                    assert_eq!(args["allowed_tools"], serde_json::json!(["read"]));
                }
            }
            if let AgentEvent::ToolEnd {
                name,
                exit_code,
                output,
                ..
            } = &record.event
            {
                if name == "program" && *exit_code == 0 && output.contains(STAMP) {
                    saw_program = true;
                }
            }
        }
    }
    assert!(saw_program, "program tool must return the script stamp");

    assert_eq!(
        std::fs::read_to_string(root.join("test.mjs")).unwrap(),
        TEST_SOURCE
    );
    assert_eq!(
        std::fs::read_to_string(root.join("secret/token.txt")).unwrap(),
        SECRET
    );
    let mut holders = Vec::new();
    files_containing(root, SECRET, &mut holders);
    assert_eq!(
        holders,
        vec![root.join("secret/token.txt")],
        "secret leaked into {holders:?}"
    );

    let verification = tokio::process::Command::new("node")
        .arg("test.mjs")
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .await
        .expect("independent node");
    assert!(
        verification.status.success(),
        "independent test failed: {}{}",
        String::from_utf8_lossy(&verification.stdout),
        String::from_utf8_lossy(&verification.stderr)
    );
    assert!(String::from_utf8_lossy(&verification.stdout).contains("COMPLEX_OK"));
}

#[test]
fn hermetic_schedule_projection_denies_the_secret_and_pins_program_args() {
    let workspace = tempfile::tempdir().expect("workspace");
    let root = workspace.path();
    seed(root, "placeholder\n");
    let projection = DirectoryProjection::load(root).expect("projection");
    let compose = projection.compose_options().expect("compose");
    assert_eq!(
        compose.components,
        vec!["system", "tools", "budget", "compact", "infer"]
    );
    let due = projection.due(chrono::Utc::now());
    assert!(due.iter().any(|item| item.session_key == "schedule:always"));
    assert!(due.iter().all(|item| item.name != "never"));
    let script = projection
        .scripts()
        .iter()
        .find(|script| script.name == "stamp")
        .expect("stamp script");
    assert_eq!(
        script.program_arguments(),
        serde_json::json!({
            "language": "javascript",
            "path": "tools/stamp.js",
            "allowed_tools": ["read"],
        })
    );

    let policy = permission_policy();
    assert_eq!(
        policy.check(
            "read",
            &serde_json::json!({"file_path": "secret/token.txt"})
        ),
        PermissionDecision::Deny
    );
    assert_eq!(
        policy.check(
            "read",
            &serde_json::json!({"file_path": "src/normalize.mjs"})
        ),
        PermissionDecision::Allow
    );
    assert_eq!(
        policy.check("program", &script.program_arguments()),
        PermissionDecision::Allow
    );
    assert_eq!(
        policy.check(
            "bash",
            &serde_json::json!({"command": "cat secret/token.txt"})
        ),
        PermissionDecision::Deny
    );
    assert_eq!(
        policy.check(
            "grep",
            &serde_json::json!({"pattern": SECRET, "path": "secret"})
        ),
        PermissionDecision::Deny
    );
}
