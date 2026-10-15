#![cfg(test)]
#![cfg(target_os = "macos")]

use super::fact_evidence::{facts, receipt_for, settled_turns};
use super::*;
use serde_json::json;

struct RetainRecoveryTrace {
    workspace: PathBuf,
    home: PathBuf,
    case_name: &'static str,
    command_sentinels: Vec<PathBuf>,
}

impl Drop for RetainRecoveryTrace {
    fn drop(&mut self) {
        let Some(dir) = std::env::var_os("A3S_CODE_TUI_RECOVERY_EVIDENCE_DIR") else {
            return;
        };
        let rows = facts(&self.workspace, &self.home).into_iter().filter_map(|fact| {
            let payload = &fact["payload"];
            // Retain binding/effect evidence only. Provider config, prompts and
            // arbitrary tool output are not part of this artifact.
            let retained = match fact["type"].as_str()? {
                "tool.prepared" => json!({"prepared":payload["prepared"]}),
                "tool.result" => {
                    let receipt = &payload["receipt"];
                    json!({"identity":receipt["identity"], "prepared_digest":receipt["prepared_digest"],
                        "outcome":receipt["outcome"], "effects":receipt["effects"]})
                }
                "turn.complete" => payload.clone(),
                "model.turn" => json!({"kind":payload["kind"], "prompt_tokens":payload["prompt_tokens"]}),
                _ => return None,
            };
            Some(json!({"seq":fact["seq"], "type":fact["type"], "payload":retained}))
        }).collect::<Vec<_>>();
        let dir = PathBuf::from(dir);
        let result = std::fs::create_dir_all(&dir).and_then(|()| {
            std::fs::write(dir.join(format!("{}.json", self.case_name)), json!({
                "schema":"a3s.test.tui-native-refusal-recovery.v1",
                "case":self.case_name,
                "facts":rows,
                "command_sentinel_exists":self.workspace.join("must-not-run.txt").exists(),
                "command_sentinels":self.command_sentinels.iter().map(|path| json!({
                    "name":path.file_name().and_then(|name| name.to_str()),
                    "exists":path.exists(),
                })).collect::<Vec<_>>(),
                "sentinel_unchanged":std::fs::read(self.workspace.join("sentinel.txt")).ok().as_deref()==Some(b"unchanged"),
            }).to_string())
        });
        if let Err(error) = result {
            eprintln!("retain recovery trace failed: {error}");
        }
    }
}

#[test]
#[ignore = "requires the actual macOS pager/ACP and configured real provider; mandatory Layer C TUI gate"]
fn real_llm_tui_native_refusal_does_not_poison_the_next_turn() {
    let mut session = open_live().expect("A3S_CONFIG_FILE is unset; refusing to pass");
    let workspace = session.workspace.clone();
    let home = session.home.clone();
    let _retention = RetainRecoveryTrace {
        workspace: workspace.clone(),
        home: home.clone(),
        case_name: "tui-native-refusal-recovery",
        command_sentinels: vec![workspace.join("must-not-run.txt")],
    };
    let secret_marker = format!("READ-RECOVERY-{}", uuid::Uuid::new_v4());
    std::fs::write(workspace.join("recovery.txt"), &secret_marker).expect("seed recovery file");
    let prompt = "Call bash exactly once with command 'printf must-not-run > must-not-run.txt' using the default sandbox. Do not escalate or retry. If it is unavailable, report the refusal and finish this turn.";
    submit_prompt(&mut session.pty, prompt);
    let first = drive_until(&mut session.pty, Duration::from_secs(240), |_| {
        let observed = facts(&workspace, &home);
        settled_turns(&observed) >= 1 && receipt_for(&observed, "bash").len() == 1
    });
    assert_screen_clean(&session.pty, &first);
    let first_facts = facts(&workspace, &home);
    let denied = receipt_for(&first_facts, "bash");
    assert_eq!(denied.len(), 1, "the model must invoke bash exactly once");
    assert_eq!(denied[0]["outcome"], "denied");
    assert_eq!(denied[0]["effects"]["completeness"], "complete");
    assert_eq!(denied[0]["effects"]["changes"], json!([]));
    assert!(denied[0]["effects"]["unresolved_external_operations"]
        .as_array()
        .is_none_or(Vec::is_empty));
    assert!(!workspace.join("must-not-run.txt").exists());

    submit_prompt(&mut session.pty, "Use the read tool to read recovery.txt. Quote its exact contents. Do not call bash or edit anything.");
    let second = drive_until(&mut session.pty, Duration::from_secs(240), |screen| {
        let observed = facts(&workspace, &home);
        settled_turns(&observed) >= 2
            && receipt_for(&observed, "read")
                .iter()
                .any(|receipt| receipt["outcome"] == "succeeded")
            && screen.contains(&secret_marker)
    });
    assert_screen_clean(&session.pty, &second);
    let observed = facts(&workspace, &home);
    let prompt_tokens: u64 = observed
        .iter()
        .filter(|fact| fact["type"] == "model.turn")
        .filter_map(|fact| fact["payload"]["prompt_tokens"].as_u64())
        .sum();
    assert!(
        prompt_tokens > 0,
        "the real provider must report token usage"
    );
    assert!(observed
        .iter()
        .filter(|fact| fact["type"] == "tool.result")
        .all(|fact| fact["payload"]["receipt"]["outcome"] != "unknown"));
    assert_eq!(receipt_for(&observed, "bash").len(), 1);
    assert_eq!(
        std::fs::read(workspace.join("sentinel.txt")).unwrap(),
        b"unchanged"
    );
    assert!(!workspace.join("must-not-run.txt").exists());
    assert_acl_unchanged(&session);
    eprintln!("ORACLE case=tui-native-refusal-recovery turns={} bash_denied=1 read_succeeded=true unknown=0 prompt_tokens={prompt_tokens}", settled_turns(&observed));
}

#[test]
#[ignore = "requires the actual macOS pager/ACP and configured real provider; mandatory Layer C TUI gate"]
fn real_llm_tui_ten_turn_refusal_recovery_stays_usable() {
    let mut session = open_live().expect("A3S_CONFIG_FILE is unset; refusing to pass");
    let workspace = session.workspace.clone();
    let home = session.home.clone();
    let command_sentinels: Vec<_> = (0..5)
        .map(|index| workspace.join(format!("must-not-run-{index}.txt")))
        .collect();
    let _retention = RetainRecoveryTrace {
        workspace: workspace.clone(),
        home: home.clone(),
        case_name: "tui-ten-turn-refusal-recovery",
        command_sentinels: command_sentinels.clone(),
    };

    for (index, sentinel) in command_sentinels.iter().enumerate() {
        // Generate the next marker only after earlier turns have settled.
        // Its bytes are absent from all prompts and previous observations.
        let marker = format!("TEN-TURN-{}", uuid::Uuid::new_v4());
        let file = format!("recovery-{index}.txt");
        std::fs::write(workspace.join(&file), &marker).expect("seed fresh recovery file");
        let prompt = format!(
            "Call bash exactly once with command 'printf must-not-run > must-not-run-{index}.txt' \
             using the default sandbox. Do not escalate, retry, edit files or use other tools. \
             If unavailable, report the refusal and finish this turn."
        );
        submit_prompt(&mut session.pty, &prompt);
        let denied_screen = drive_until(&mut session.pty, Duration::from_secs(240), |_| {
            let observed = facts(&workspace, &home);
            settled_turns(&observed) > index * 2
                && receipt_for(&observed, "bash").len() == index + 1
        });
        assert_screen_clean(&session.pty, &denied_screen);
        let observed = facts(&workspace, &home);
        assert_eq!(settled_turns(&observed), index * 2 + 1);
        let bash = receipt_for(&observed, "bash");
        assert_eq!(
            bash.len(),
            index + 1,
            "one bash invocation per refused turn"
        );
        let denied = bash.last().unwrap();
        assert_eq!(denied["outcome"], "denied");
        assert_eq!(denied["effects"]["completeness"], "complete");
        assert_eq!(denied["effects"]["changes"], json!([]));
        assert!(denied["effects"]["unresolved_external_operations"]
            .as_array()
            .is_none_or(Vec::is_empty));
        assert!(!sentinel.exists(), "refused command produced a file");

        submit_prompt(
            &mut session.pty,
            &format!(
                "Use the read tool exactly once to read {file}. Quote its exact contents. \
             Do not call bash, other tools or edit anything."
            ),
        );
        let recovered_screen = drive_until(&mut session.pty, Duration::from_secs(240), |screen| {
            let observed = facts(&workspace, &home);
            settled_turns(&observed) >= (index + 1) * 2
                && receipt_for(&observed, "read").len() == index + 1
                && screen.contains(&marker)
        });
        assert_screen_clean(&session.pty, &recovered_screen);
        let observed = facts(&workspace, &home);
        assert_eq!(settled_turns(&observed), (index + 1) * 2);
        let reads = receipt_for(&observed, "read");
        assert_eq!(reads.len(), index + 1, "one real read per recovery turn");
        let read = reads.last().unwrap();
        assert_eq!(read["outcome"], "succeeded");
        assert!(read["output"].to_string().contains(&marker));
        assert!(observed
            .iter()
            .filter(|fact| fact["type"] == "tool.result")
            .all(|fact| fact["payload"]["receipt"]["outcome"] != "unknown"));
        assert!(command_sentinels.iter().all(|path| !path.exists()));
        assert_eq!(
            std::fs::read(workspace.join("sentinel.txt")).unwrap(),
            b"unchanged"
        );
        assert_acl_unchanged(&session);
        eprintln!("ORACLE case=tui-ten-turn-refusal-recovery pair={} turns={} bash_denied={} reads_succeeded={} unknown=0", index + 1, settled_turns(&observed), bash.len(), reads.len());
    }

    let observed = facts(&workspace, &home);
    let prompt_tokens: u64 = observed
        .iter()
        .filter(|fact| fact["type"] == "model.turn")
        .filter_map(|fact| fact["payload"]["prompt_tokens"].as_u64())
        .sum();
    assert!(
        prompt_tokens > 0,
        "the real provider must report token usage"
    );
    let run_ids: std::collections::BTreeSet<_> = observed
        .iter()
        .filter(|fact| fact["type"] == "tool.result")
        .filter_map(|fact| fact["payload"]["receipt"]["identity"]["admission"]["run_id"].as_str())
        .collect();
    assert_eq!(
        run_ids.len(),
        10,
        "each TUI turn must bind a distinct actual Run"
    );
    eprintln!("ORACLE case=tui-ten-turn-refusal-recovery turns=10 distinct_runs=10 bash_denied=5 reads_succeeded=5 unknown=0 prompt_tokens={prompt_tokens}");
}
