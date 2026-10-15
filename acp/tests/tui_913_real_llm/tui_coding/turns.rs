#![cfg(test)]

//! Request-terminal barriers and bounded per-Run evidence for coding turns.

use super::*;
use a3s_code_core::content_digest::digest_bytes;
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
pub(crate) enum ExpectedTerminal {
    Success,
    Unverified,
}

pub(crate) struct PromptTurn {
    pub(crate) facts: Vec<Value>,
    pub(crate) screen: String,
    pub(crate) run_id: String,
    request: Value,
    response: Value,
}

impl PromptTurn {
    pub(crate) fn evidence(&self) -> Value {
        serde_json::json!({"request":self.request,"response":self.response,
            "run_id":self.run_id,"first_fact_seq":self.facts.first().map(|v|&v["seq"]),
            "last_fact_seq":self.facts.last().map(|v|&v["seq"])})
    }
}

pub(crate) fn trace_events(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn trace_pair(events: &[Value], prompt: &str) -> Option<(Value, Value)> {
    let requests: Vec<_> = events
        .iter()
        .filter(|v| v["kind"] == "prompt-request")
        .collect();
    let responses: Vec<_> = events
        .iter()
        .filter(|v| v["kind"] == "prompt-response")
        .collect();
    if requests.len() != 1 || responses.len() != 1 {
        return None;
    }
    let request = requests[0];
    let response = responses[0];
    if request["in_flight_before"] != 0
        || request["prompt_digest"] != digest_bytes("a3s.test.tui-prompt.v1", prompt.as_bytes())
        || request["seq"].as_u64()? >= response["seq"].as_u64()?
        || request["prompt_id"].as_str()?.is_empty()
        || request["session_id"].as_str()?.is_empty()
        || request["rpc_id"].is_null()
        || ["rpc_id", "prompt_id", "session_id", "prompt_digest"]
            .iter()
            .any(|field| request[field] != response[field])
    {
        return None;
    }
    Some((request.clone(), response.clone()))
}

/// Every prepared operation must have exactly one matching complete result.
/// No narrative, RPC error, local scan or subsequent turn can settle Unknown.
fn settled_run(facts: &[Value]) -> Option<String> {
    let mut runs = BTreeSet::new();
    let mut results = 0usize;
    let preparations: Vec<_> = facts
        .iter()
        .filter(|v| v["type"] == "tool.prepared")
        .collect();
    for fact in &preparations {
        let prepared: Preparation =
            serde_json::from_value(fact["payload"]["prepared"].clone()).ok()?;
        if prepared.schema != "a3s.code.tool-prepared.v1"
            || prepared.identity.admission.run_id.is_empty()
            || prepared.identity.admission.thread_id.is_empty()
            || validate_digest(&prepared.identity.admission.capability_binding_digest).is_err()
        {
            return None;
        }
        runs.insert(prepared.identity.admission.run_id.clone());
        let identity = serde_json::to_value(&prepared.identity).ok()?;
        let digest = digest_json("a3s.code.tool-prepared-digest.v1", &prepared).ok()?;
        let matching: Vec<_> = facts
            .iter()
            .filter(|row| {
                row["type"] == "tool.result" && row["payload"]["receipt"]["identity"] == identity
            })
            .collect();
        if matching.len() != 1 {
            return None;
        }
        let row = matching[0];
        let receipt = &row["payload"]["receipt"];
        if row["seq"].as_u64()? <= fact["seq"].as_u64()?
            || receipt["schema"] != "a3s.code.tool-result.v1"
            || receipt["prepared_digest"] != digest
            || !matches!(
                receipt["outcome"].as_str()?,
                "succeeded" | "failed" | "denied"
            )
            || receipt["effects"]["completeness"] != "complete"
            || receipt["effects"]
                .get("unresolved_external_operations")
                .is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty))
        {
            return None;
        }
        results += 1;
    }
    if runs.len() != 1 || results != facts.iter().filter(|v| v["type"] == "tool.result").count() {
        return None;
    }
    runs.into_iter().next()
}

pub(crate) fn drive_prompt(
    session: &mut LiveSession,
    prompt: &str,
    expected: ExpectedTerminal,
) -> PromptTurn {
    let path = session
        .prompt_trace
        .clone()
        .expect("coding request relay enabled");
    let before_event = trace_events(&path)
        .last()
        .and_then(|v| v["seq"].as_u64())
        .unwrap_or(0);
    let before_fact = fact_evidence::facts(&session.workspace, &session.home)
        .last()
        .and_then(|v| v["seq"].as_u64())
        .unwrap_or(0);
    session.pty.clear_line();
    session.pty.type_text(prompt);
    session.pty.wait_for(prompt, Duration::from_secs(5));
    session.pty.enter();
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        let screen = session.pty.screen();
        if answer_dialog(&mut session.pty, &screen) {
            continue;
        }
        let events: Vec<_> = trace_events(&path)
            .into_iter()
            .filter(|v| v["seq"].as_u64().is_some_and(|seq| seq > before_event))
            .collect();
        if let Some((request, response)) = trace_pair(&events, prompt) {
            match expected {
                ExpectedTerminal::Success => {
                    assert!(
                        response["error_code"].is_null(),
                        "unexpected prompt error: {response}"
                    );
                    assert_eq!(response["stop_reason"], "end_turn", "{response}");
                }
                ExpectedTerminal::Unverified => {
                    assert_eq!(response["error_code"], -32603, "{response}");
                    assert_eq!(response["completion_gate"], true, "{response}");
                }
            }
            if !screen_is_busy(&screen) {
                let facts: Vec<_> = fact_evidence::facts(&session.workspace, &session.home)
                    .into_iter()
                    .filter(|v| v["seq"].as_u64().is_some_and(|seq| seq > before_fact))
                    .collect();
                let run_id = settled_run(&facts).expect(
                    "one admitted Run with complete original receipts; no Unknown or orphan result",
                );
                return PromptTurn {
                    facts,
                    screen,
                    run_id,
                    request,
                    response,
                };
            }
        }
        assert!(
            Instant::now() < deadline,
            "no identity-bound prompt terminal\n{}",
            session.pty.redact(&screen)
        );
        std::thread::sleep(Duration::from_millis(40));
    }
}

pub(crate) fn assert_failed_checker(turn: &PromptTurn) {
    assert!(
        !turn.facts.iter().any(|v| v["type"] == "turn.complete"),
        "an unverified checkpoint must not claim completion"
    );
    let failed = turn
        .facts
        .iter()
        .filter(|v| v["type"] == "tool.prepared")
        .find_map(|fact| {
            let p: Preparation =
                serde_json::from_value(fact["payload"]["prepared"].clone()).ok()?;
            let call = turn
                .facts
                .iter()
                .filter(|v| v["type"] == "model.turn")
                .flat_map(|v| {
                    v["payload"]
                        .get("call")
                        .into_iter()
                        .chain(v["payload"]["also"].as_array().into_iter().flatten())
                })
                .find(|call| {
                    call["id"] == p.identity.tool_call_id
                        && call["name"] == "bash"
                        && call["args"]["command"]
                            .as_str()
                            .is_some_and(|s| s.trim() == "python3 gate.py")
                        && digest_json("a3s.code.tool-arguments.v1", &call["args"])
                            .ok()
                            .as_deref()
                            == Some(p.original_args_digest.as_str())
                })?;
            if p.original_args_digest != p.effective_args_digest || call["name"] != "bash" {
                return None;
            }
            let identity = serde_json::to_value(&p.identity).ok()?;
            let result = turn.facts.iter().find(|v| {
                v["type"] == "tool.result" && v["payload"]["receipt"]["identity"] == identity
            })?;
            let receipt = &result["payload"]["receipt"];
            let wire: Value = serde_json::from_str(receipt["output"].as_str()?).ok()?;
            (receipt["outcome"] == "failed"
                && wire["metadata"]["exit_code"] == 1
                && wire["kind"] == "a3s.fact_tool_result"
                && wire["is_error"] == true
                && wire["text"]
                    .as_str()?
                    .lines()
                    .any(|s| s.trim() == FAIL_LINE))
            .then_some(result["seq"].as_u64()?)
        })
        .expect("an exact checker must really execute with exit 1; Denied is not a failed check");
    let reply = turn
        .facts
        .iter()
        .rev()
        .find(|v| v["type"] == "model.turn")
        .unwrap();
    assert!(
        reply["seq"].as_u64().unwrap() > failed
            && reply["payload"]["kind"] == "text"
            && reply["payload"]["prompt_tokens"]
                .as_u64()
                .is_some_and(|n| n > 0)
            && reply["payload"]["text"]
                .as_str()
                .is_some_and(|s| s.contains(FAIL_LINE)),
        "actual assistant must quote the failed checker after its result"
    );
}

pub(crate) fn assert_verified(turn: &PromptTurn) -> GateExec {
    let exec =
        typed_checker_exec(&turn.facts).expect("current exact checker report and original receipt");
    let key = exec.receipt_key.as_deref().unwrap();
    let completion = turn
        .facts
        .iter()
        .rev()
        .find(|v| v["type"] == "turn.complete")
        .expect("completed fact");
    assert_eq!(completion["payload"]["terminal"], "verified");
    assert_eq!(
        completion["payload"]["effect_digest"],
        exec.verification_effect_digest.as_deref().unwrap()
    );
    assert!(completion["payload"]["receipt_keys"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == key));
    assert!(
        final_quote_for_checker(&turn.facts, completion, key).is_some(),
        "actual final reply must quote the bound checker"
    );
    exec
}

#[test]
fn request_terminal_requires_the_current_unqueued_identity_and_exact_prompt() {
    let request = serde_json::json!({"seq":1,"kind":"prompt-request","rpc_id":8,"prompt_id":"p1","session_id":"s1",
        "prompt_digest":digest_bytes("a3s.test.tui-prompt.v1",b"hello"),"in_flight_before":0});
    let mut response = request.clone();
    response["kind"] = "prompt-response".into();
    response["seq"] = 2.into();
    let rows = vec![request, response];
    assert!(trace_pair(&rows, "hello").is_some());
    for (row, field, value) in [
        (0, "in_flight_before", serde_json::json!(1)),
        (1, "rpc_id", serde_json::json!(9)),
        (1, "prompt_id", serde_json::json!("foreign")),
        (1, "session_id", serde_json::json!("foreign")),
        (1, "seq", serde_json::json!(0)),
    ] {
        let mut corrupt = rows.clone();
        corrupt[row][field] = value;
        assert!(trace_pair(&corrupt, "hello").is_none());
    }
    assert!(trace_pair(&rows, "different").is_none());
    assert!(trace_pair(&rows[..1], "hello").is_none());
    let mut duplicate = rows.clone();
    duplicate.push(rows[1].clone());
    assert!(trace_pair(&duplicate, "hello").is_none());
}

#[test]
fn settled_receipts_reject_unknown_incomplete_missing_duplicate_and_cross_run_results() {
    let rows = checker_rows();
    assert_eq!(settled_run(&rows).as_deref(), Some("actual-run-fixture"));
    for (field, value) in [
        ("outcome", serde_json::json!("unknown")),
        ("prepared_digest", serde_json::json!("foreign")),
    ] {
        let mut corrupt = rows.clone();
        corrupt[2]["payload"]["receipt"][field] = value;
        assert!(settled_run(&corrupt).is_none());
    }
    let mut corrupt = rows.clone();
    corrupt[2]["payload"]["receipt"]["effects"]["completeness"] = "incomplete".into();
    assert!(settled_run(&corrupt).is_none());
    assert!(settled_run(&rows[..2]).is_none());
    let mut duplicate = rows.clone();
    duplicate.push(rows[2].clone());
    assert!(settled_run(&duplicate).is_none());
    let mut foreign = rows.clone();
    foreign[2]["payload"]["receipt"]["identity"]["admission"]["run_id"] = "foreign".into();
    assert!(settled_run(&foreign).is_none());
}

#[test]
fn failed_checker_requires_execution_and_a_real_post_result_reply() {
    let mut facts = checker_rows();
    facts[2]["payload"]["receipt"]["outcome"] = "failed".into();
    let wire = serde_json::json!({"kind":"a3s.fact_tool_result", "is_error":true,
        "text":FAIL_LINE,"metadata":{"exit_code":1}});
    facts[2]["payload"]["receipt"]["output"] = wire.to_string().into();
    facts.push(serde_json::json!({"seq":4,"type":"model.turn",
        "payload":{"kind":"text","text":FAIL_LINE,"prompt_tokens":30}}));
    let turn = |facts| PromptTurn {
        facts,
        screen: String::new(),
        run_id: "fixture".into(),
        request: Value::Null,
        response: Value::Null,
    };
    assert_failed_checker(&turn(facts.clone()));
    let mut denied = facts.clone();
    denied[2]["payload"]["receipt"]["outcome"] = "denied".into();
    assert!(std::panic::catch_unwind(|| assert_failed_checker(&turn(denied))).is_err());
    let mut not_started = facts.clone();
    let mut wire = wire;
    wire["metadata"]["exit_code"] = Value::Null;
    not_started[2]["payload"]["receipt"]["output"] = wire.to_string().into();
    assert!(std::panic::catch_unwind(|| assert_failed_checker(&turn(not_started))).is_err());
    let mut no_quote = facts.clone();
    no_quote[3]["payload"]["text"] = "completed".into();
    assert!(std::panic::catch_unwind(|| assert_failed_checker(&turn(no_quote))).is_err());
    let mut no_usage = facts;
    no_usage[3]["payload"]["prompt_tokens"] = 0.into();
    assert!(std::panic::catch_unwind(|| assert_failed_checker(&turn(no_usage))).is_err());
}
