#!/usr/bin/env python3
"""Audit the four settled coding requests; never infer full TUI qualification."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

from verify_tui_read_evidence import digest, identity, require


PROMPTS = (
    "Read rate.py, fee.py, and gate.py. Name both wrong returns. Do not edit.",
    "Fix only rate.py. Run exactly `python3 gate.py`. If denied, request escalation. Quote output.",
    "Fix fee.py. Run exactly `python3 gate.py`. If denied, request escalation. Quote output.",
    "Run exactly `python3 gate.py`. If denied, request escalation. Quote its output.",
)
PASS, FAIL = "DUAL-GATE-7K", "DUAL-HOLD"


def prompt_digest(text: str) -> str:
    return "sha256:" + hashlib.sha256(b"a3s.test.tui-prompt.v1\0" + text.encode()).hexdigest()


def valid_digest(value: str, prefixed: bool = True) -> bool:
    if not isinstance(value, str):
        return False
    if prefixed:
        if not value.startswith("sha256:"):
            return False
        value = value[7:]
    return len(value) == 64 and all(c in "0123456789abcdef" for c in value)


def prepared_digest(value: dict) -> str:
    fields = ("schema", "identity", "tool_name", "original_args_digest", "effective_args_digest", "recovery")
    require(set(fields) <= set(value) <= set(fields) | {"baseline"}, "preparation schema")
    require(value["schema"] == "a3s.code.tool-prepared.v1", "preparation version")
    require(value["recovery"] in ("never_replay", "query_status", "idempotency_key"), "recovery contract")
    for key in ("original_args_digest", "effective_args_digest"):
        require(valid_digest(value[key]), "invalid argument digest")
    require(valid_digest(value["identity"]["admission"]["capability_binding_digest"]), "invalid capability binding")
    ordered = {key: identity(value[key]) if key == "identity" else value[key] for key in fields}
    if value.get("baseline") is not None:
        ordered["baseline"] = json.loads(json.dumps(value["baseline"], sort_keys=True))
    return digest("a3s.code.tool-prepared-digest.v1", ordered)


def effect_digest(state: dict) -> str:
    # Independent encoding of typed MutationLedger V1 after-states. This
    # fixture permits only these three paths and has no background writers.
    encoded = bytearray(b"a3s.code.workspace-effect-state.v1\0")

    def field(text):
        data = text.encode()
        encoded.extend(str(len(data)).encode() + b":" + data)

    for path, after in sorted(state.items()):
        require(set(after) <= {"kind", "mode", "content_digest"}, "file identity schema")
        require(after["kind"] == "file", "unexpected fixture object type")
        encoded.append(1)
        field(path)
        field(after["kind"])
        mode = after.get("mode")
        encoded.append(mode is not None)
        if mode is not None:
            require(type(mode) is int and 0 <= mode < 2**32, "file mode")
            encoded.extend(mode.to_bytes(4, "big"))
        content = after.get("content_digest")
        encoded.append(content is not None)
        if content is not None:
            require(valid_digest(content), "file content digest")
            field(content)
    return hashlib.sha256(encoded).hexdigest()


def raw_projection(row: dict) -> dict:
    """Project private facts without returning prompt, output or reply content."""
    payload, kind = row["payload"], row["type"]
    if kind == "tool.result":
        receipt = payload["receipt"]
        require(receipt["schema"] == "a3s.code.tool-result.v1", "raw receipt schema")
        try:
            wire = json.loads(receipt["output"])
        except (ValueError, TypeError):
            wire = {}
        meta = wire.get("metadata") or {}
        report = meta.get("verification_report") or {}
        payload = {key: receipt[key] for key in ("identity", "prepared_digest", "outcome", "effects")}
        payload.update(exit_code=meta.get("exit_code"), verification_status=report.get("status"),
                       verification_subject=report.get("subject"), verification_effect_digest=report.get("effect_digest"))
    elif kind == "model.turn":
        calls = ([payload["call"]] if "call" in payload else []) + payload.get("also", [])
        payload = {"kind": payload["kind"], "prompt_tokens": payload.get("prompt_tokens"),
                   "quotes_checker_output": PASS in payload.get("text", ""),
                   "calls": [{"id": call["id"], "name": call["name"],
                              "args_digest": digest("a3s.code.tool-arguments.v1", json.loads(json.dumps(call["args"], sort_keys=True))),
                              "exact_checker_command": call["name"] == "bash" and call["args"].get("command", "").strip() == "python3 gate.py"}
                             for call in calls]}
    return {"seq": row["seq"], "type": kind, "payload": payload}


def verify_artifact(value: dict, raw: list[dict]) -> dict:
    require(value["schema"] == "a3s.test.tui-coding-evidence.v1" and value["case"] == "tui-coding-multiturn", "artifact schema/case")
    require(value["assertions_finished"] is True and value["test_panicked"] is False, "unfinished or panicked assertions")
    require(value["checker_unchanged"] is True, "checker changed")
    phases, facts = value["settled_prompts"], value["facts"]
    require(len(phases) == 4, "four settled prompts required")
    require(value["prompt_events"] == [event for phase in phases for event in (phase["request"], phase["response"])], "trace/phase mismatch")
    require(len({phase["run_id"] for phase in phases}) == 4, "distinct Run ids required")
    require(len({phase["request"]["prompt_id"] for phase in phases}) == 4, "distinct prompt ids required")
    require(len({phase["request"]["rpc_id"] for phase in phases}) == 4, "distinct RPC ids required")
    require(len({phase["request"]["session_id"] for phase in phases}) == 1, "same session required")
    seqs = [row["seq"] for row in facts]
    require(seqs == sorted(set(seqs)), "unordered or duplicate fact sequence")
    public = {row["seq"]: row for row in facts}
    raw_seqs = [row["seq"] for row in raw]
    require(raw_seqs == sorted(set(raw_seqs)), "unordered or duplicate private facts")
    private = {row["seq"]: row for row in raw}
    for row in raw:
        if row["type"] in ("model.turn", "tool.prepared", "tool.result", "turn.complete"):
            require(row["seq"] in public, "public evidence omitted an original fact")
            require(raw_projection(row) == public[row["seq"]], "public/private fact mismatch")

    last_event, last_fact, state, carried_keys, reports = 0, 0, {}, set(), []
    raw_final_coverage, total_receipts = [], 0
    for index, phase in enumerate(phases):
        request, response = phase["request"], phase["response"]
        require(request["kind"] == "prompt-request" and response["kind"] == "prompt-response", "event kind")
        require(request["in_flight_before"] == 0, "queued request")
        require(last_event < request["seq"] < response["seq"], "request overlap or reversed response")
        last_event = response["seq"]
        require(request["prompt_digest"] == prompt_digest(PROMPTS[index]), "wrong submitted prompt")
        for key in ("rpc_id", "prompt_id", "session_id", "prompt_digest"):
            require(request[key] == response[key] and request[key] is not None and request[key] != "", "foreign response identity")
        if index == 1:
            require(response["error_code"] == -32603 and response["completion_gate"] is True and response["stop_reason"] is None, "partial checkpoint must be unverified")
        else:
            require(response["error_code"] is None and response["stop_reason"] == "end_turn" and response["completion_gate"] is False, "successful prompt terminal")
        first, last = phase["first_fact_seq"], phase["last_fact_seq"]
        require(last_fact < first <= last, "overlapping fact ranges")
        last_fact = last
        rows = [row for row in facts if first <= row["seq"] <= last]
        originals = [row for row in raw if first <= row["seq"] <= last]
        messages = [row for row in originals if row["type"] == "user.message"]
        require(len(messages) == 1, "missing actual user message")
        # Core may append its typed owed-work context after this exact input.
        text = messages[0]["payload"]["text"]
        require(text == PROMPTS[index] or (index == 2 and text.startswith(PROMPTS[index] + "\n\n[owed work] ")), "actual input mismatch")
        prepared, results, calls, completes, checks = {}, {}, {}, [], []
        for row in rows:
            p, kind = row["payload"], row["type"]
            if kind == "model.turn":
                require(type(p["prompt_tokens"]) is int and p["prompt_tokens"] > 0, "missing actual provider usage")
                for call in p["calls"]:
                    require(call["id"] not in calls, "duplicate model call")
                    calls[call["id"]] = (row["seq"], call)
            elif kind == "tool.prepared":
                item = p["prepared"]
                ident = identity(item["identity"])
                require(ident["admission"]["run_id"] == phase["run_id"] and ident["admission"]["thread_id"] == request["session_id"], "foreign Run/session")
                key = digest("a3s.code.tool-invocation.v1", ident)
                require(key not in prepared, "duplicate preparation")
                seq, call = calls[ident["tool_call_id"]]
                require(seq < row["seq"] and call["name"] == item["tool_name"] and call["args_digest"] == item["original_args_digest"], "model/preparation mismatch")
                prepared[key] = (row["seq"], prepared_digest(item), item, call)
            elif kind == "tool.result":
                key = digest("a3s.code.tool-invocation.v1", identity(p["identity"]))
                require(key in prepared and key not in results, "missing preparation or duplicate result")
                seq, expected, item, call = prepared[key]
                require(seq < row["seq"] and p["prepared_digest"] == expected, "result/preparation binding")
                require(p["outcome"] in ("succeeded", "failed", "denied"), "Unknown outcome")
                effects = p["effects"]
                require(effects["completeness"] == "complete" and effects.get("unresolved_external_operations", []) == [], "unsettled effects")
                for change in effects["changes"]:
                    require(change["path"] in ("rate.py", "fee.py", "runs.txt"), "unexpected fixture mutation")
                    require(index != 0 and not (index == 1 and change["path"] == "fee.py"), "premature fixture mutation")
                    state[change["path"]] = change["after"]
                results[key] = row
                if item["tool_name"] == "bash" and call["exact_checker_command"] is True:
                    require(item["original_args_digest"] == item["effective_args_digest"], "transformed checker arguments")
                    if p["exit_code"] is not None:
                        checks.append((key, row))
            elif kind == "turn.complete":
                completes.append(row)
        require(prepared and set(prepared) == set(results), "missing original result")
        total_receipts += len(results)
        carried_keys.update(results)
        models = [row for row in rows if row["type"] == "model.turn"]
        require(models and models[-1]["payload"]["kind"] == "text", "missing final provider reply")
        reply = models[-1]
        raw_final_coverage.append(reply["seq"] in private)
        if index == 1:
            failed = [row for _, row in checks if row["payload"]["outcome"] == "failed" and row["payload"]["exit_code"] == 1]
            require(not completes and failed and not any(row["payload"]["verification_status"] == "passed" for row in results.values()), "partial checkpoint falsely completed")
            require(reply["seq"] > failed[-1]["seq"] and reply["seq"] in private, "missing original failed reply")
            require(FAIL in private[reply["seq"]]["payload"].get("text", ""), "assistant did not quote actual failure")
            wire = json.loads(private[failed[-1]["seq"]]["payload"]["receipt"]["output"])
            require(wire["kind"] == "a3s.fact_tool_result" and wire["is_error"] is True and FAIL in [line.strip() for line in wire["text"].splitlines()], "missing actual failed checker output")
        else:
            require(len(completes) == 1, "one completion required")
            complete = completes[0]
            p = complete["payload"]
            require(reply["seq"] <= p["log_cut"]["seq"] < complete["seq"], "reply outside completion cut")
            require(set(p["receipt_keys"]) == carried_keys and len(p["receipt_keys"]) == len(carried_keys), "completion receipt coverage")
            require(p.get("background_receipt_keys", []) == [], "unexpected background completion")
            if index == 0:
                require(p["terminal"] == "narrative" and not state and p["effect_digest"] is None, "diagnosis mutated or verified")
            else:
                passed = [(key, row) for key, row in checks if row["payload"]["outcome"] == "succeeded" and row["payload"]["exit_code"] == 0 and row["payload"]["verification_status"] == "passed"]
                require(passed and p["terminal"] == "verified", "missing executed Passed checker")
                key, checker = passed[-1]
                require(checker["seq"] < reply["seq"] and reply["payload"]["quotes_checker_output"] is True, "missing final checker quote")
                require(key in p["receipt_keys"] and checker["payload"]["verification_subject"] == "shell:contract:dual-defect", "foreign checker report")
                expected = effect_digest(state)
                require(expected == p["effect_digest"] == checker["payload"]["verification_effect_digest"], "stale effect digest")
                require(checker["seq"] in private, "missing original passing result")
                wire = json.loads(private[checker["seq"]]["payload"]["receipt"]["output"])
                require(wire["kind"] == "a3s.fact_tool_result" and wire["is_error"] is False and PASS in [line.strip() for line in wire["text"].splitlines()], "missing original passing output")
                reports.append(expected)
            state, carried_keys = {}, set()
    require(all(any(phase["first_fact_seq"] <= row["seq"] <= phase["last_fact_seq"] for phase in phases) for row in facts), "facts outside settled phases")
    return {"kind": "selected-four-request-coding-audit", "result": "passed", "distinct_runs": 4,
            "terminals": ["narrative", "unverified", "verified", "verified"], "original_receipts": total_receipts,
            "actual_partial_checker_exit": 1, "unknown": 0, "effect_digests_recomputed": reports,
            "private_final_reply_coverage": raw_final_coverage,
            "limitations": ["public final-quote flags are cross-checked only where a private snapshot contains that reply",
                            "not full fact-log hash-chain or atomic compiler-input authentication",
                            "not default-native, platform, adapter-matrix, full-catalog or release qualification"]}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifact", type=Path)
    parser.add_argument("private_facts", type=Path, help="private original facts; contents are never printed")
    args = parser.parse_args()
    try:
        result = verify_artifact(json.loads(args.artifact.read_text()), json.loads(args.private_facts.read_text()))
    except (OSError, ValueError, KeyError, TypeError, IndexError):
        # Malformed private values may contain arbitrary content. No traceback
        # or exception value is emitted across this privacy boundary.
        print(json.dumps({"kind": "selected-four-request-coding-audit", "result": "failed"}))
        return 1
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
