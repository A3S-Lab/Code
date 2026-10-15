#![cfg(test)]
#![cfg(target_os = "macos")]

//! TL01's five cases through the actual pager, ACP, Core and real provider.
//! Fixture answers are generated after launch and never included in prompts.
//! No permission dialog is auto-approved by this driver.

use super::fact_evidence::{facts, receipt_for, settled_turns};
use super::*;
use a3s_code_core::content_digest::{digest_bytes, digest_json, validate_digest};
use serde_json::{json, Value};

struct ReadEvidence {
    workspace: PathBuf,
    home: PathBuf,
    case: &'static str,
    watched: Vec<(PathBuf, String)>,
    oracles: Vec<Value>,
    requests: Vec<Value>,
    finished: bool,
}

impl ReadEvidence {
    fn new(session: &LiveSession, case: &'static str) -> Self {
        Self {
            workspace: session.workspace.clone(),
            home: session.home.clone(),
            case,
            watched: Vec::new(),
            oracles: Vec::new(),
            requests: Vec::new(),
            finished: false,
        }
    }

    fn watch(&mut self, path: PathBuf) {
        self.watched.push((path.clone(), file_digest(&path)));
    }

    fn finish(&mut self, session: &LiveSession, expected_turns: usize) {
        let observed = facts(&self.workspace, &self.home);
        assert_eq!(settled_turns(&observed), expected_turns);
        assert_eq!(self.requests.len(), expected_turns);
        let tokens: u64 = observed
            .iter()
            .filter(|f| f["type"] == "model.turn")
            .filter_map(|f| f["payload"]["prompt_tokens"].as_u64())
            .sum();
        assert!(tokens > 0, "the actual provider must report token usage");
        let receipts = receipt_for(&observed, "read");
        assert_eq!(receipts.len(), expected_turns, "one read per actual turn");
        assert_eq!(
            observed
                .iter()
                .filter(|f| f["type"] == "tool.result")
                .count(),
            receipts.len(),
            "the model must not invoke additional tools"
        );
        for receipt in receipts {
            assert_ne!(receipt["outcome"], "unknown");
            assert_eq!(receipt["effects"]["completeness"], "complete");
            assert_eq!(receipt["effects"]["changes"], json!([]));
            assert!(receipt["effects"]["unresolved_external_operations"]
                .as_array()
                .is_none_or(Vec::is_empty));
            validate_digest(
                receipt["prepared_digest"]
                    .as_str()
                    .expect("prepared digest"),
            )
            .expect("well-formed prepared binding");
        }
        for (path, before) in &self.watched {
            assert_eq!(file_digest(path), *before, "fixture file changed");
        }
        assert_eq!(
            std::fs::read(self.workspace.join("sentinel.txt")).unwrap(),
            b"unchanged"
        );
        assert_acl_unchanged(session);
        self.finished = true;
        eprintln!("ORACLE case={} turns={expected_turns} read_receipts={expected_turns} unknown=0 prompt_tokens={tokens}", self.case);
    }
}

impl Drop for ReadEvidence {
    fn drop(&mut self) {
        let Some(dir) = std::env::var_os("A3S_CODE_TUI_READ_EVIDENCE_DIR") else {
            return;
        };
        let observed = facts(&self.workspace, &self.home);
        // Preserve identities, effects and test oracles. Arbitrary tool text,
        // provider configuration and user prompts are deliberately excluded.
        let retained: Vec<Value> = observed
            .into_iter()
            .filter_map(|f| {
                let p = &f["payload"];
                let payload = match f["type"].as_str()? {
                    "tool.prepared" => json!({"prepared":p["prepared"]}),
                    "tool.result" => {
                        let r = &p["receipt"];
                        json!({"identity":r["identity"],"prepared_digest":r["prepared_digest"],
                        "outcome":r["outcome"],"effects":r["effects"]})
                    }
                    "turn.complete" => p.clone(),
                    "model.turn" => json!({"kind":p["kind"],"prompt_tokens":p["prompt_tokens"]}),
                    "user.message" => json!({"text_digest":p["text"].as_str().map(|text|
                        digest_bytes("a3s.test.submitted-read-prompt.v1",text.as_bytes()))}),
                    _ => return None,
                };
                Some(json!({"seq":f["seq"],"type":f["type"],"payload":payload}))
            })
            .collect();
        let artifact = json!({"schema":"a3s.test.tui-read-evidence.v1","case":self.case,
            "assertions_finished":self.finished,"test_panicked":std::thread::panicking(),
            "facts":retained,"oracles":self.oracles,"requests":self.requests,
            "files":self.watched.iter().map(|(path,before)| {
                let after = std::fs::read(path).ok().map(|bytes| digest_bytes("a3s.test.read-fixture.v1", &bytes));
                json!({"name":path.file_name().and_then(|s|s.to_str()),"before":before,
                    "after":after,"unchanged":after.as_ref()==Some(before)})
            }).collect::<Vec<_>>()});
        let dir = PathBuf::from(dir);
        let result = std::fs::create_dir_all(&dir).and_then(|()| {
            std::fs::write(
                dir.join(format!("{}.json", self.case)),
                artifact.to_string(),
            )
        });
        if let Err(error) = result {
            eprintln!("read evidence retention failed: {error}");
            assert!(
                std::thread::panicking(),
                "cannot pass without requested evidence"
            );
        }
    }
}

fn file_digest(path: &Path) -> String {
    digest_bytes(
        "a3s.test.read-fixture.v1",
        &std::fs::read(path).expect("owned regular fixture"),
    )
}

fn seed(session: &LiveSession, evidence: &mut ReadEvidence, name: &str, contents: &str) {
    let path = session.workspace.join(name);
    std::fs::write(&path, contents).expect("owned fixture");
    evidence.watch(path);
}

fn marker() -> String {
    format!("READ-{}", uuid::Uuid::new_v4())
}

fn compact_composer(screen: &str) -> String {
    screen
        .chars()
        .filter(|ch| !ch.is_whitespace() && !matches!(ch, '│' | '╭' | '╮' | '╰' | '╯' | '─' | '❯'))
        .collect()
}

fn read_message_viewport(screen: &vt100::Screen) -> String {
    // This isolated-home, full-width fixture uses LayoutConfig's default
    // block_pad_right=2 and EntryRenderer's 10-column timestamp reservation.
    // Read actual VT cells so the right gutter cannot split a wrapped marker
    // with metadata. The exact persisted assistant reply is checked separately.
    let (_, cols) = screen.size();
    screen
        .rows(0, cols.saturating_sub(12))
        .flat_map(|row| {
            row.chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn submit_read_prompt(pty: &mut PtySession, prompt: &str) {
    pty.clear_line();
    pty.type_text(prompt);
    let expected = compact_composer(prompt);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let screen = pty.screen();
        assert_screen_clean(pty, &screen);
        // This is only a visual readiness check. The actual submitted user
        // message must equal every original byte before this turn qualifies.
        if compact_composer(&screen).contains(&expected) {
            pty.enter();
            return;
        }
        assert!(
            Instant::now() < deadline,
            "read input timed out\n{}",
            pty.redact(&screen)
        );
        match pty.rx.recv_timeout(Duration::from_millis(80)) {
            Ok(bytes) => {
                pty.raw.extend_from_slice(&bytes);
                pty.parser.process(&bytes);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => panic!("owned TUI exited"),
        }
    }
}

fn read_turn(
    session: &mut LiveSession,
    evidence: &mut ReadEvidence,
    args: Value,
    quoted: &[&str],
) -> (String, Value) {
    let before = settled_turns(&facts(&session.workspace, &session.home));
    let prompt = format!("Call the read tool exactly once using these JSON arguments: {args}. Do not call any other tools, retry, or request elevation. Then return only JSON without fences: an array of objects with exactly path, status, and text. Keep the requested path order. Set status to ok or error according to the actual result for that file; text must quote its returned text or error. Do not report a failed batch member as ok. Finish this turn.");
    submit_read_prompt(&mut session.pty, &prompt);
    let deadline = Instant::now() + Duration::from_secs(180);
    let screen = loop {
        let screen = session.pty.screen();
        assert_screen_clean(&session.pty, &screen);
        assert!(
            !screen.contains("Yes, proceed"),
            "unexpected approval; never auto-approve"
        );
        let observed = facts(&session.workspace, &session.home);
        let visible = read_message_viewport(session.pty.parser.screen());
        let reply_visible = observed
            .iter()
            .rev()
            .find(|f| f["type"] == "model.turn" && f["payload"]["kind"] == "text")
            .and_then(|f| f["payload"]["text"].as_str())
            .is_some_and(|text| {
                !text.is_empty()
                    && visible.contains(
                        &text
                            .chars()
                            .filter(|ch| !ch.is_whitespace())
                            .collect::<String>(),
                    )
            });
        if settled_turns(&observed) > before
            && reply_visible
            && quoted.iter().all(|m| visible.contains(m))
        {
            eprintln!(
                "OBSERVATION read_turn={} raw_marker_match={} wrapped_marker_match=true",
                before + 1,
                quoted.iter().all(|m| screen.contains(m))
            );
            break screen;
        }
        assert!(
            Instant::now() < deadline,
            "read turn timed out\n{}",
            session.pty.redact(&screen)
        );
        match session.pty.rx.recv_timeout(Duration::from_millis(80)) {
            Ok(bytes) => {
                session.pty.raw.extend_from_slice(&bytes);
                session.pty.parser.process(&bytes);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => panic!("owned TUI exited"),
        }
    };
    let observed = facts(&session.workspace, &session.home);
    let receipts = receipt_for(&observed, "read");
    assert_eq!(receipts.len(), before + 1, "exactly one read per turn");
    let user_messages: Vec<_> = observed
        .iter()
        .filter(|f| f["type"] == "user.message")
        .collect();
    assert_eq!(
        user_messages.len(),
        before + 1,
        "one actual submission per turn"
    );
    assert_eq!(
        user_messages.last().unwrap()["payload"]["text"],
        prompt,
        "submitted prompt bytes changed"
    );
    let receipt = *receipts.last().expect("bound actual read receipt");
    let prepared = observed
        .iter()
        .find(|f| {
            f["type"] == "tool.prepared"
                && f["payload"]["prepared"]["identity"] == receipt["identity"]
        })
        .expect("actual preparation for this receipt");
    let requested_digest = digest_json("a3s.code.tool-arguments.v1", &args).unwrap();
    assert_eq!(
        prepared["payload"]["prepared"]["original_args_digest"], requested_digest,
        "model changed requested read arguments"
    );
    evidence.requests.push(json!({"identity":receipt["identity"],"requested_args":args.clone(),"requested_args_digest":requested_digest,
        "submitted_message_digest":digest_bytes("a3s.test.submitted-read-prompt.v1",prompt.as_bytes()),
        "submitted_message_matches":true}));
    let reply = model_reply(session);
    let paths: Vec<&str> = if let Some(entries) = args["files"].as_array() {
        entries
            .iter()
            .map(|entry| entry["path"].as_str().unwrap())
            .collect()
    } else {
        vec![args["file_path"].as_str().unwrap()]
    };
    let rows = reply
        .as_array()
        .expect("actual model reply must follow the requested JSON contract");
    assert_eq!(rows.len(), paths.len());
    for (row, path) in rows.iter().zip(paths) {
        assert_eq!(row.as_object().expect("reply row").len(), 3);
        assert_eq!(row["path"], path);
        assert!(matches!(row["status"].as_str(), Some("ok" | "error")));
        assert!(row["text"].as_str().is_some());
    }
    if args.get("files").is_none() {
        let expected = if receipts.last().unwrap()["outcome"] == "succeeded" {
            "ok"
        } else {
            "error"
        };
        assert_eq!(
            rows[0]["status"], expected,
            "model claim must match the terminal receipt"
        );
    }
    (screen, receipt.clone())
}

fn model_reply(session: &LiveSession) -> Value {
    let observed = facts(&session.workspace, &session.home);
    let text = observed
        .iter()
        .rev()
        .find(|fact| fact["type"] == "model.turn" && fact["payload"]["kind"] == "text")
        .and_then(|fact| fact["payload"]["text"].as_str())
        .expect("actual assistant text fact");
    serde_json::from_str(text.trim())
        .expect("actual model must return the requested JSON without fences")
}

fn content(receipt: &Value) -> (String, Value) {
    let wire: Value =
        serde_json::from_str(receipt["output"].as_str().expect("tool output envelope"))
            .expect("typed tool output envelope");
    assert_eq!(wire["kind"], "a3s.fact_tool_result");
    (
        wire["text"].as_str().expect("tool text").to_string(),
        wire["metadata"].clone(),
    )
}

#[test]
#[ignore = "requires the actual macOS pager/ACP and configured real provider; mandatory Layer C TUI gate"]
fn real_llm_tui_tl01_01_reads_random_utf8_with_source() {
    let mut session = open_live().expect("A3S_CONFIG_FILE is required; refusing false PASS");
    let mut evidence = ReadEvidence::new(&session, "TUI-TL01-01");
    let hidden = marker();
    seed(
        &session,
        &mut evidence,
        "unicode.txt",
        &format!("中文与 emoji 🧪\n{hidden}\n"),
    );
    let (screen, receipt) = read_turn(
        &mut session,
        &mut evidence,
        json!({"file_path":"unicode.txt"}),
        &[&hidden],
    );
    assert_eq!(receipt["outcome"], "succeeded");
    let (text, metadata) = content(&receipt);
    assert!(text.contains(&hidden) && text.contains("中文与 emoji 🧪"));
    assert!(screen.contains("unicode.txt"));
    assert!(model_reply(&session)[0]["text"]
        .as_str()
        .unwrap()
        .contains(&hidden));
    assert_eq!(metadata["source_anchors"], json!(["unicode.txt"]));
    evidence.oracles.push(
        json!({"random_text_in_actual_receipt":true,"source_in_screen":true,"utf8_preserved":true}),
    );
    evidence.finish(&session, 1);
}

#[test]
#[ignore = "requires the actual macOS pager/ACP and configured real provider; mandatory Layer C TUI gate"]
fn real_llm_tui_tl01_02_respects_range_without_suffix_leak() {
    let mut session = open_live().expect("A3S_CONFIG_FILE is required; refusing false PASS");
    let mut evidence = ReadEvidence::new(&session, "TUI-TL01-02");
    let a = marker();
    let b = marker();
    let excluded = marker();
    seed(
        &session,
        &mut evidence,
        "range.txt",
        &format!("first\nsecond\n{a}\n{b}\n{excluded}\n"),
    );
    let (screen, receipt) = read_turn(
        &mut session,
        &mut evidence,
        json!({"file_path":"range.txt","offset":2,"limit":2}),
        &[&a, &b],
    );
    assert_eq!(receipt["outcome"], "succeeded");
    let (text, metadata) = content(&receipt);
    assert!(text.contains(&format!("     3\t{a}")) && text.contains(&format!("     4\t{b}")));
    assert!(!text.contains(&excluded) && !screen.contains(&excluded));
    assert!(
        !serde_json::to_string(&facts(&session.workspace, &session.home))
            .unwrap()
            .contains(&excluded)
    );
    assert_eq!(metadata["range"]["offset"], 2);
    assert_eq!(metadata["range"]["returned_lines"], 2);
    let answer = model_reply(&session)[0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(answer.contains(&a) && answer.contains(&b) && !answer.contains(&excluded));
    evidence
        .oracles
        .push(json!({"exact_range_and_line_numbers":true,"excluded_suffix_in_model_facts":false}));
    evidence.finish(&session, 1);
}

#[test]
#[ignore = "requires the actual macOS pager/ACP and configured real provider; mandatory Layer C TUI gate"]
fn real_llm_tui_tl01_03_preserves_batch_member_errors() {
    let mut session = open_live().expect("A3S_CONFIG_FILE is required; refusing false PASS");
    let mut evidence = ReadEvidence::new(&session, "TUI-TL01-03");
    let a = marker();
    let b = marker();
    seed(&session, &mut evidence, "batch-a.txt", &a);
    seed(&session, &mut evidence, "batch-b.txt", &b);
    let (screen, receipt) = read_turn(
        &mut session,
        &mut evidence,
        json!({"files":[{"path":"batch-a.txt"},{"path":"absent.txt"},{"path":"batch-b.txt"}]}),
        &[&a, &b],
    );
    assert_eq!(
        receipt["outcome"], "succeeded",
        "transport result carries explicit partial-member status"
    );
    let (text, metadata) = content(&receipt);
    let first = text.find("=== batch-a.txt ===").expect("first segment");
    let missing = text.find("=== absent.txt ===").expect("error segment");
    let last = text.find("=== batch-b.txt ===").expect("last segment");
    assert!(first < missing && missing < last);
    assert!(text.contains("Failed to read file absent.txt"));
    assert!(screen.contains("absent.txt"));
    assert_eq!(metadata["batch"]["successful_files"], 2);
    assert_eq!(metadata["batch"]["failed_files"], 1);
    let answer = model_reply(&session);
    assert_eq!(answer[0]["status"], "ok");
    assert_eq!(answer[1]["status"], "error");
    assert_eq!(answer[2]["status"], "ok");
    assert!(answer[0]["text"].as_str().unwrap().contains(&a));
    assert!(answer[2]["text"].as_str().unwrap().contains(&b));
    assert!(!session.workspace.join("absent.txt").exists());
    evidence.oracles.push(json!({"ordered_members":true,"success_count":2,"failure_count":1,"missing_member_visible":true}));
    evidence.finish(&session, 1);
}

#[test]
#[ignore = "requires the actual macOS pager/ACP and configured real provider; mandatory Layer C TUI gate"]
fn real_llm_tui_tl01_04_file_errors_remain_bounded_and_recoverable() {
    let mut session = open_live().expect("A3S_CONFIG_FILE is required; refusing false PASS");
    let mut evidence = ReadEvidence::new(&session, "TUI-TL01-04");
    std::fs::create_dir(session.workspace.join("directory")).unwrap();
    let fifo = session.workspace.join("writerless.fifo");
    assert!(std::process::Command::new("/usr/bin/mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let large = session.workspace.join("oversize.txt");
    std::fs::File::create(&large)
        .unwrap()
        .set_len(16 * 1024 * 1024 + 1)
        .unwrap();
    evidence.watch(large);
    for (path, reason) in [
        ("missing.txt", "no such file or directory"),
        ("directory", "not a regular file"),
        ("writerless.fifo", "not a regular file"),
        ("oversize.txt", "too large"),
    ] {
        let (_, receipt) = read_turn(&mut session, &mut evidence, json!({"file_path":path}), &[]);
        assert_eq!(
            receipt["outcome"], "failed",
            "a known file error is a terminal failed result"
        );
        let (text, _) = content(&receipt);
        assert!(
            text.to_lowercase().contains(reason),
            "wrong error contract: {text}"
        );
        let recovered = marker();
        seed(&session, &mut evidence, "recovery.txt", &recovered);
        let (_, receipt) = read_turn(
            &mut session,
            &mut evidence,
            json!({"file_path":"recovery.txt"}),
            &[&recovered],
        );
        assert_eq!(receipt["outcome"], "succeeded");
        assert_eq!(
            std::fs::read(session.workspace.join("recovery.txt")).unwrap(),
            recovered.as_bytes()
        );
        // Each recovery uses new bytes; retain their final digest once, below.
        evidence
            .watched
            .retain(|(p, _)| p.file_name().is_none_or(|n| n != "recovery.txt"));
        evidence
            .oracles
            .push(json!({"path":path,"failed_complete":true,"next_turn_read_succeeded":true}));
    }
    evidence.watch(session.workspace.join("recovery.txt"));
    evidence.finish(&session, 8);
}

#[test]
#[ignore = "requires the actual macOS pager/ACP and configured real provider; mandatory Layer C TUI gate"]
fn real_llm_tui_tl01_05_refuses_outside_and_symlink_reads() {
    let mut session = open_live().expect("A3S_CONFIG_FILE is required; refusing false PASS");
    let mut evidence = ReadEvidence::new(&session, "TUI-TL01-05");
    let outside = session
        .workspace
        .parent()
        .unwrap()
        .join("outside-canary.txt");
    let hidden = marker();
    std::fs::write(&outside, &hidden).unwrap();
    evidence.watch(outside.clone());
    std::os::unix::fs::symlink(&outside, session.workspace.join("outside-link.txt")).unwrap();
    for path in ["../outside-canary.txt", "outside-link.txt"] {
        let (screen, receipt) =
            read_turn(&mut session, &mut evidence, json!({"file_path":path}), &[]);
        // Both parent traversal and an existing symlink cross the host
        // guardrail before tool dispatch. This is Denied, not a file error.
        assert_eq!(receipt["outcome"], "denied");
        assert!(!receipt["output"].as_str().expect("host refusal").is_empty());
        assert!(
            !screen.contains(&hidden)
                && !serde_json::to_string(&facts(&session.workspace, &session.home))
                    .unwrap()
                    .contains(&hidden)
        );
        evidence
            .oracles
            .push(json!({"path":path,"boundary_refused":true,"outside_canary_leaked":false}));
    }
    evidence.finish(&session, 2);
}

#[test]
fn read_viewport_preserves_soft_wrap_and_excludes_timestamp_metadata() {
    let mut parser = vt100::Parser::new(4, 120, 0);
    parser.process(b"\x1b[1;3HREAD-first-\x1b[1;111H7:33 AM\x1b[2;3Hsecond");
    assert!(!compact_composer(&parser.screen().contents()).contains("READ-first-second"));
    assert!(read_message_viewport(parser.screen()).contains("READ-first-second"));
    parser.process(b"\x1b[2;3Hbad-second");
    assert!(!read_message_viewport(parser.screen()).contains("READ-first-second"));
    parser.process(b"\x1b[2J\x1b[1;111HREAD-right");
    assert!(!read_message_viewport(parser.screen()).contains("READ-right"));
}

#[test]
fn read_composer_handles_json_token_wrapping() {
    let prompt = "Call read {\"files\":[{\"path\":\"a\"},{\"path\":\"b\"}]} now.";
    let visible = "│ ❯ Call read {\"files\":[{\"path\":\"a\"}, │\n│   {\"path\":\"b\"}]} now. │";
    assert!(!screen_has(visible, prompt));
    assert!(compact_composer(visible).contains(&compact_composer(prompt)));
    assert!(!compact_composer(visible).contains(&compact_composer(&prompt.replace('b', "c"))));
}
