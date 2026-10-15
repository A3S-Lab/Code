#![cfg(test)]

//! Current typed-fact checker binding for the coding acceptance tests.

use super::*;
use a3s_code_core::content_digest::{digest_json, validate_digest};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[path = "tui_coding/turns.rs"]
pub(super) mod turns;

#[test]
fn terminal_turn_failure_is_not_a_settled_tool_refusal() {
    assert_eq!(
        banner("Turn failed in 29s: completion gate: no bound Passed verification"),
        Some("Turn failed")
    );
    assert_eq!(
        banner("Turn failed: tool_effect_unresolved: 1 invocation obligation"),
        Some("Turn failed")
    );
    assert_eq!(
        banner("bash denied; receipt Denied + Complete; continuing"),
        None
    );
    assert_eq!(
        banner("tool_effect_unresolved requires identity-bound reconciliation"),
        None
    );
}

pub(super) struct RetainCodingTrace {
    workspace: PathBuf,
    home: PathBuf,
    case: &'static str,
    finished: bool,
    prompt_trace: Option<PathBuf>,
    phases: Vec<Value>,
}

impl RetainCodingTrace {
    pub(super) fn new(session: &LiveSession, case: &'static str) -> Self {
        Self {
            workspace: session.workspace.clone(),
            home: session.home.clone(),
            case,
            finished: false,
            prompt_trace: session.prompt_trace.clone(),
            phases: Vec::new(),
        }
    }

    pub(super) fn finish(&mut self) {
        self.finished = true;
    }

    pub(super) fn record_turn(&mut self, turn: &turns::PromptTurn) {
        self.phases.push(turn.evidence());
    }
}

impl Drop for RetainCodingTrace {
    fn drop(&mut self) {
        let Some(dir) = std::env::var_os("A3S_CODE_TUI_CODING_EVIDENCE_DIR") else {
            return;
        };
        let rows: Vec<Value> = fact_evidence::facts(&self.workspace, &self.home).into_iter().filter_map(|fact| {
            let p = &fact["payload"];
            let payload = match fact["type"].as_str()? {
                "tool.prepared" => serde_json::json!({"prepared":p["prepared"]}),
                "tool.result" => {
                    let receipt = &p["receipt"];
                    let wire = receipt["output"].as_str().and_then(|text| serde_json::from_str::<Value>(text).ok());
                    serde_json::json!({"identity":receipt["identity"],"prepared_digest":receipt["prepared_digest"],
                        "outcome":receipt["outcome"],"effects":receipt["effects"],
                        "exit_code":wire.as_ref().map(|v| &v["metadata"]["exit_code"]),
                        "verification_status":wire.as_ref().map(|v| &v["metadata"]["verification_report"]["status"]),
                        "verification_subject":wire.as_ref().map(|v| &v["metadata"]["verification_report"]["subject"]),
                        "verification_effect_digest":wire.as_ref().map(|v| &v["metadata"]["verification_report"]["effect_digest"])})
                }
                "turn.complete" => p.clone(),
                "model.turn" => serde_json::json!({"kind":p["kind"],"prompt_tokens":p["prompt_tokens"],
                    "quotes_checker_output":p["text"].as_str().is_some_and(|text| text.contains(PASS_LINE)),
                    "calls":p.get("call").into_iter().chain(p["also"].as_array().into_iter().flatten()).map(|call| {
                        serde_json::json!({"id":call["id"],"name":call["name"],
                            "args_digest":digest_json("a3s.code.tool-arguments.v1",&call["args"]).ok(),
                            "exact_checker_command":call["name"]=="bash" && call["args"]["command"].as_str().is_some_and(|command| command.trim()=="python3 gate.py")})
                    }).collect::<Vec<_>>() }),
                _ => return None,
            };
            Some(serde_json::json!({"seq":fact["seq"],"type":fact["type"],"payload":payload}))
        }).collect();
        let digest = |path: &Path| {
            std::fs::read(path).ok().map(|bytes| {
                a3s_code_core::content_digest::digest_bytes("a3s.test.coding-fixture.v1", &bytes)
            })
        };
        let dir = PathBuf::from(dir);
        let fixture_digests: Vec<Value> = ["rate.py", "fee.py", "gate.py", "runs.txt"]
            .iter()
            .map(
                |name| serde_json::json!({"name":name,"digest":digest(&self.workspace.join(name))}),
            )
            .collect();
        let artifact = serde_json::json!({"schema":"a3s.test.tui-coding-evidence.v1","case":self.case,
            "assertions_finished":self.finished,"test_panicked":std::thread::panicking(),"facts":rows,
            "prompt_events":self.prompt_trace.as_deref().map(turns::trace_events).unwrap_or_default(),
            "settled_prompts":self.phases,
            "checker_unchanged":std::fs::read(self.workspace.join("gate.py")).ok().as_deref()==Some(GATE_PY.as_bytes()),
            "fixture_digests":fixture_digests});
        let result = std::fs::create_dir_all(&dir).and_then(|()| {
            std::fs::write(
                dir.join(format!("{}.json", self.case)),
                artifact.to_string(),
            )
        });
        if let Err(error) = result {
            eprintln!("coding evidence retention failed: {error}");
            assert!(
                std::thread::panicking(),
                "cannot pass without requested coding evidence"
            );
        }
    }
}

// Preserve the published V1 struct order for domain-separated digests. Value
// objects alone serialize their keys in a different order. This independent
// test adapter refuses extra identity/preparation fields rather than guessing.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Admission {
    run_id: String,
    thread_id: String,
    capability_binding_digest: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    turn: u64,
    cycle: u64,
    tool_call_id: String,
    batch_index: u32,
    admission: Admission,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Preparation {
    schema: String,
    identity: Identity,
    tool_name: String,
    original_args_digest: String,
    effective_args_digest: String,
    recovery: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    baseline: Option<Value>,
}

pub(super) fn typed_checker_exec(rows: &[Value]) -> Option<GateExec> {
    for row in rows.iter().filter(|row| row["type"] == "tool.prepared") {
        let Some(preparation_seq) = row["seq"].as_u64() else {
            continue;
        };
        let Ok(prepared) =
            serde_json::from_value::<Preparation>(row["payload"]["prepared"].clone())
        else {
            continue;
        };
        if prepared.schema != "a3s.code.tool-prepared.v1"
            || prepared.tool_name != "bash"
            || prepared.identity.turn == 0
            || prepared.identity.tool_call_id.is_empty()
            || prepared.identity.admission.run_id.is_empty()
            || prepared.identity.admission.thread_id.is_empty()
            || validate_digest(&prepared.identity.admission.capability_binding_digest).is_err()
            || prepared.original_args_digest != prepared.effective_args_digest
            || !matches!(
                prepared.recovery.as_str(),
                "never_replay" | "query_status" | "idempotency_key"
            )
        {
            continue;
        }
        let calls: Vec<&Value> = rows
            .iter()
            .filter(|model| {
                model["type"] == "model.turn"
                    && model["payload"]["kind"] == "tool"
                    && model["seq"]
                        .as_u64()
                        .is_some_and(|seq| seq < preparation_seq)
            })
            .flat_map(|model| {
                model["payload"]
                    .get("call")
                    .into_iter()
                    .chain(model["payload"]["also"].as_array().into_iter().flatten())
            })
            .collect();
        let Some(call) = calls.into_iter().rev().find(|call| {
            call["name"] == "bash"
                && call["id"] == prepared.identity.tool_call_id
                && digest_json("a3s.code.tool-arguments.v1", &call["args"])
                    .ok()
                    .as_deref()
                    == Some(prepared.original_args_digest.as_str())
        }) else {
            continue;
        };
        // This fixture requests one stored checker command. A compound shell
        // command can replace its exit status or echo success after failure.
        let Some(command) = call["args"]["command"]
            .as_str()
            .filter(|c| c.trim() == "python3 gate.py")
        else {
            continue;
        };
        let identity = serde_json::to_value(&prepared.identity).ok()?;
        let preparation_digest = digest_json("a3s.code.tool-prepared-digest.v1", &prepared).ok()?;
        for result in rows.iter().filter(|result| {
            result["type"] == "tool.result"
                && result["seq"]
                    .as_u64()
                    .is_some_and(|seq| seq > preparation_seq)
        }) {
            let receipt = &result["payload"]["receipt"];
            if receipt["schema"] != "a3s.code.tool-result.v1"
                || receipt["identity"] != identity
                || receipt["prepared_digest"] != preparation_digest
                || receipt["outcome"] != "succeeded"
                || receipt["effects"]["completeness"] != "complete"
                || receipt["effects"]
                    .get("unresolved_external_operations")
                    .is_some_and(|ops| !ops.as_array().is_some_and(Vec::is_empty))
            {
                continue;
            }
            let Some(wire) = receipt["output"]
                .as_str()
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
            else {
                continue;
            };
            if wire["kind"] != "a3s.fact_tool_result"
                || wire["is_error"] != false
                || wire["metadata"]["exit_code"] != 0
            {
                continue;
            }
            let report = &wire["metadata"]["verification_report"];
            let Some(effect_digest) = report["effect_digest"].as_str() else {
                continue;
            };
            if report["subject"] != "shell:contract:dual-defect"
                || report["status"] != "passed"
                // MutationLedger V1 publishes raw hex; invocation/preparation
                // digests use the separate sha256:<hex> content-digest wire.
                || validate_digest(&format!("sha256:{effect_digest}")).is_err()
            {
                continue;
            }
            let Some(text) = wire["text"]
                .as_str()
                .filter(|text| output_prints_pass(text))
            else {
                continue;
            };
            let stdout_line = text
                .lines()
                .find(|line| line.trim() == PASS_LINE)?
                .trim()
                .to_string();
            return Some(GateExec {
                call_id: prepared.identity.tool_call_id.clone(),
                command: command.to_string(),
                stdout_line,
                receipt_key: Some(
                    digest_json("a3s.code.tool-invocation.v1", &prepared.identity).ok()?,
                ),
                verification_effect_digest: Some(effect_digest.to_owned()),
            });
        }
    }
    None
}

fn checker_rows() -> Vec<Value> {
    let args = serde_json::json!({"command":"python3 gate.py"});
    let digest = digest_json("a3s.code.tool-arguments.v1", &args).unwrap();
    let preparation = serde_json::json!({
        "schema":"a3s.code.tool-prepared.v1", "tool_name":"bash",
        "identity":{"turn":1,"cycle":1,"tool_call_id":"checker-call","batch_index":0,
            "admission":{"run_id":"actual-run-fixture","thread_id":"fixture-thread",
                "capability_binding_digest":format!("sha256:{}","0".repeat(64))}},
        "original_args_digest":digest,"effective_args_digest":digest,"recovery":"never_replay"});
    let typed: Preparation = serde_json::from_value(preparation.clone()).unwrap();
    let wire = serde_json::json!({"kind":"a3s.fact_tool_result","is_error":false,
        "text":format!("{PASS_LINE}\n"),"metadata":{"exit_code":0,
            "verification_report":{"subject":"shell:contract:dual-defect","status":"passed",
                "effect_digest":"a".repeat(64)}}})
    .to_string();
    vec![
        serde_json::json!({"seq":1,"type":"model.turn","payload":{"kind":"tool",
            "call":{"id":"checker-call","name":"bash","args":args}}}),
        serde_json::json!({"seq":2,"type":"tool.prepared","payload":{"prepared":preparation}}),
        serde_json::json!({"seq":3,"type":"tool.result","payload":{"receipt":{
            "schema":"a3s.code.tool-result.v1","identity":preparation["identity"],
            "prepared_digest":digest_json("a3s.code.tool-prepared-digest.v1",&typed).unwrap(),
            "outcome":"succeeded","output":wire,"effects":{"completeness":"complete",
                "changes":[],"unresolved_external_operations":[]}}}}),
    ]
}

#[test]
fn typed_checker_requires_preparation_arguments_result_and_exit_binding() {
    let actual = checker_rows();
    let exec = typed_checker_exec(&actual).expect("typed checker proof");
    assert_eq!(exec.command, "python3 gate.py");
    assert_eq!(exec.stdout_line, PASS_LINE);
    validate_digest(exec.receipt_key.as_deref().unwrap()).unwrap();
    for field in [
        "schema",
        "identity",
        "prepared_digest",
        "outcome",
        "effects",
        "output",
    ] {
        let mut corrupted = actual.clone();
        corrupted[2]["payload"]["receipt"][field] = Value::Null;
        assert!(
            typed_checker_exec(&corrupted).is_none(),
            "accepted corrupt {field}"
        );
    }
    let mut changed_arguments = actual.clone();
    changed_arguments[0]["payload"]["call"]["args"]["command"] = "echo python3 gate.py".into();
    assert!(typed_checker_exec(&changed_arguments).is_none());
    let mut failed_exit = actual.clone();
    let mut wire: Value = serde_json::from_str(
        failed_exit[2]["payload"]["receipt"]["output"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    wire["metadata"]["exit_code"] = 1.into();
    failed_exit[2]["payload"]["receipt"]["output"] = wire.to_string().into();
    assert!(typed_checker_exec(&failed_exit).is_none());
    let mut missing_report = actual.clone();
    let mut wire: Value = serde_json::from_str(
        missing_report[2]["payload"]["receipt"]["output"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    wire["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("verification_report");
    missing_report[2]["payload"]["receipt"]["output"] = wire.to_string().into();
    assert!(typed_checker_exec(&missing_report).is_none());
    let mut incomplete = actual;
    incomplete[2]["payload"]["receipt"]["effects"]["completeness"] = "incomplete".into();
    assert!(typed_checker_exec(&incomplete).is_none());
    let mut compound = checker_rows();
    compound[0]["payload"]["call"]["args"]["command"] =
        "python3 gate.py; printf DUAL-GATE-7K".into();
    let changed = digest_json(
        "a3s.code.tool-arguments.v1",
        &compound[0]["payload"]["call"]["args"],
    )
    .unwrap();
    compound[1]["payload"]["prepared"]["original_args_digest"] = changed.clone().into();
    compound[1]["payload"]["prepared"]["effective_args_digest"] = changed.into();
    let changed: Preparation =
        serde_json::from_value(compound[1]["payload"]["prepared"].clone()).unwrap();
    compound[2]["payload"]["receipt"]["prepared_digest"] =
        digest_json("a3s.code.tool-prepared-digest.v1", &changed)
            .unwrap()
            .into();
    assert!(
        typed_checker_exec(&compound).is_none(),
        "compound shell status must not qualify the checker"
    );
}

pub(super) fn require_verified_completion(
    session: &mut LiveSession,
    receipt_key: &str,
    effect_digest: &str,
) {
    let workspace = session.workspace.clone();
    let home = session.home.clone();
    let screen = drive_until(&mut session.pty, Duration::from_secs(60), |screen| {
        screen.contains("Turn failed")
            || screen.contains("tool_effect_unresolved")
            || fact_evidence::facts(&workspace, &home).iter().any(|fact| {
                fact["type"] == "turn.complete"
                    && fact["payload"]["receipt_keys"]
                        .as_array()
                        .is_some_and(|keys| keys.iter().any(|key| key == receipt_key))
            })
    });
    assert_screen_clean(&session.pty, &screen);
    let observed = fact_evidence::facts(&workspace, &home);
    let completion = observed
        .iter()
        .rev()
        .find(|fact| {
            fact["type"] == "turn.complete"
                && fact["payload"]["receipt_keys"]
                    .as_array()
                    .is_some_and(|keys| keys.iter().any(|key| key == receipt_key))
        })
        .expect("actual completion must bind the executed checker receipt");
    assert_eq!(
        completion["payload"]["terminal"], "verified",
        "coding acceptance requires verified closure"
    );
    validate_digest(&format!(
        "sha256:{}",
        completion["payload"]["effect_digest"]
            .as_str()
            .expect("verified effect digest")
    ))
    .expect("valid bound mutation digest");
    assert_eq!(
        completion["payload"]["effect_digest"], effect_digest,
        "completion must verify the current checker effect digest"
    );
    assert!(
        final_quote_for_checker(&observed, completion, receipt_key).is_some(),
        "the requested final quote must be a real provider reply after the bound checker and before completion"
    );
}

fn final_quote_for_checker<'a>(
    observed: &'a [Value],
    completion: &Value,
    receipt_key: &str,
) -> Option<&'a Value> {
    let cut = completion["payload"]["log_cut"]["seq"].as_u64()?;
    if cut >= completion["seq"].as_u64()? {
        return None;
    }
    let checker_seq = observed.iter().find(|fact| {
        fact["type"] == "tool.result"
            && serde_json::from_value::<Identity>(fact["payload"]["receipt"]["identity"].clone())
                .ok()
                .and_then(|identity| digest_json("a3s.code.tool-invocation.v1", &identity).ok())
                .as_deref()
                == Some(receipt_key)
    })?["seq"]
        .as_u64()?;
    let reply = observed.iter().rev().find(|fact| {
        fact["type"] == "model.turn"
            && fact["seq"]
                .as_u64()
                .is_some_and(|seq| seq > checker_seq && seq <= cut)
    })?;
    (reply["payload"]["kind"] == "text"
        && reply["payload"]["text"]
            .as_str()
            .is_some_and(|text| text.contains(PASS_LINE))
        && reply["payload"]["prompt_tokens"]
            .as_u64()
            .is_some_and(|tokens| tokens > 0))
    .then_some(reply)
}

#[test]
fn final_quote_requires_the_actual_last_reply_inside_the_bound_completion_cut() {
    let mut facts = checker_rows();
    let key = typed_checker_exec(&facts).unwrap().receipt_key.unwrap();
    facts.push(serde_json::json!({"seq":4,"type":"model.turn",
        "payload":{"kind":"text","text":PASS_LINE,"prompt_tokens":30}}));
    let completion = serde_json::json!({"seq":5,"type":"turn.complete",
        "payload":{"log_cut":{"seq":4},"receipt_keys":[key]}});
    assert!(final_quote_for_checker(&facts, &completion, &key).is_some());
    for (field, value) in [
        ("kind", serde_json::json!("tool")),
        ("kind", serde_json::json!("reasoning")),
        ("text", serde_json::json!("completed")),
        ("prompt_tokens", serde_json::json!(0)),
    ] {
        let mut corrupted = facts.clone();
        corrupted[3]["payload"][field] = value;
        assert!(
            final_quote_for_checker(&corrupted, &completion, &key).is_none(),
            "accepted {field}"
        );
    }
    assert!(final_quote_for_checker(&facts, &completion, "foreign-receipt").is_none());
    let mut outside = facts.clone();
    outside[3]["seq"] = serde_json::json!(6);
    assert!(final_quote_for_checker(&outside, &completion, &key).is_none());
    let mut later_tool = facts.clone();
    later_tool.push(serde_json::json!({"seq":5,"type":"model.turn","payload":{"kind":"tool"}}));
    let later_completion = serde_json::json!({"seq":6,"payload":{"log_cut":{"seq":5}}});
    assert!(final_quote_for_checker(&later_tool, &later_completion, &key).is_none());
    let mut before_checker = facts;
    before_checker[3]["seq"] = serde_json::json!(2);
    assert!(final_quote_for_checker(&before_checker, &completion, &key).is_none());
}
