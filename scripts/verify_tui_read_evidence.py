#!/usr/bin/env python3
"""Check the selected real-provider TL01 receipts; never infer full TUI success."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


EXPECTED_OUTCOMES = {
    "TUI-TL01-01": ["succeeded"],
    "TUI-TL01-02": ["succeeded"],
    "TUI-TL01-03": ["succeeded"],  # Batch members are asserted separately.
    "TUI-TL01-04": ["failed", "succeeded"] * 4,
    "TUI-TL01-05": ["denied", "denied"],
}

EXPECTED_ARGS = {
    "TUI-TL01-01": [{"file_path": "unicode.txt"}],
    "TUI-TL01-02": [{"file_path": "range.txt", "offset": 2, "limit": 2}],
    "TUI-TL01-03": [{"files": [{"path": path} for path in ("batch-a.txt", "absent.txt", "batch-b.txt")]}],
    "TUI-TL01-04": [args for path in ("missing.txt", "directory", "writerless.fifo", "oversize.txt")
                    for args in ({"file_path": path}, {"file_path": "recovery.txt"})],
    "TUI-TL01-05": [{"file_path": path} for path in ("../outside-canary.txt", "outside-link.txt")],
}

EXPECTED_ORACLES = {
    "TUI-TL01-01": [{"random_text_in_actual_receipt": True, "source_in_screen": True, "utf8_preserved": True}],
    "TUI-TL01-02": [{"exact_range_and_line_numbers": True, "excluded_suffix_in_model_facts": False}],
    "TUI-TL01-03": [{"ordered_members": True, "success_count": 2, "failure_count": 1, "missing_member_visible": True}],
    "TUI-TL01-04": [{"path": path, "failed_complete": True, "next_turn_read_succeeded": True}
                    for path in ("missing.txt", "directory", "writerless.fifo", "oversize.txt")],
    "TUI-TL01-05": [{"path": path, "boundary_refused": True, "outside_canary_leaked": False}
                    for path in ("../outside-canary.txt", "outside-link.txt")],
}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def digest(domain: str, value: dict) -> str:
    encoded = json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()
    return "sha256:" + hashlib.sha256(domain.encode() + b"\0" + encoded).hexdigest()


def identity(value: dict) -> dict:
    # The published V1 struct's serialization order, independently reconstructed.
    require(set(value) == {"turn", "cycle", "tool_call_id", "batch_index", "admission"}, "identity schema")
    admission = value["admission"]
    require(set(admission) == {"run_id", "thread_id", "capability_binding_digest"}, "admission schema")
    require(value["turn"] > 0 and bool(value["tool_call_id"]), "missing invocation identity")
    require(bool(admission["run_id"]) and bool(admission["thread_id"]), "missing actual Run")
    return {**{key: value[key] for key in ("turn", "cycle", "tool_call_id", "batch_index")},
            "admission": {key: admission[key] for key in ("run_id", "thread_id", "capability_binding_digest")}}


def prepared_digest(value: dict) -> str:
    fields = ("schema", "identity", "tool_name", "original_args_digest", "effective_args_digest", "recovery")
    require(set(fields) <= set(value) <= set(fields) | {"baseline"}, "prepared schema")
    require(value["schema"] == "a3s.code.tool-prepared.v1" and value["tool_name"] == "read", "wrong tool preparation")
    ordered = {key: identity(value[key]) if key == "identity" else value[key] for key in fields}
    if value.get("baseline") is not None:
        # A serde_json::Value baseline has sorted object keys, unlike structs.
        ordered["baseline"] = json.loads(json.dumps(value["baseline"], sort_keys=True))
    return digest("a3s.code.tool-prepared-digest.v1", ordered)


def verify_artifact(value: dict) -> dict:
    case = value["case"]
    expected = EXPECTED_OUTCOMES[case]
    require(value["schema"] == "a3s.test.tui-read-evidence.v1", "artifact schema")
    require(value["assertions_finished"] and not value["test_panicked"], "test assertions did not finish")
    facts = value["facts"]
    seqs = [row["seq"] for row in facts]
    require(seqs == sorted(set(seqs)), "nonunique or unordered facts")
    requests = value["requests"]
    require([r["requested_args"] for r in requests] == EXPECTED_ARGS[case], "wrong requested case arguments")
    request_by_key = {digest("a3s.code.tool-invocation.v1", identity(r["identity"])): r for r in requests}
    require(len(request_by_key) == len(expected), "missing or duplicate requested invocations")
    for request in requests:
        require(request["submitted_message_matches"] is True, "submitted input mismatch")
        require(request["requested_args_digest"] == digest("a3s.code.tool-arguments.v1", request["requested_args"]), "request digest mismatch")
    prepared, results, completes, tokens, run_ids = {}, [], [], 0, set()
    messages, segment_tokens = [], 0
    for row in facts:
        payload = row["payload"]
        if row["type"] == "tool.prepared":
            item = payload["prepared"]
            key = digest("a3s.code.tool-invocation.v1", identity(item["identity"]))
            require(key not in prepared, "duplicate preparation")
            require(key in request_by_key, "preparation does not bind requested invocation")
            require(item["original_args_digest"] == request_by_key[key]["requested_args_digest"], "model changed requested arguments")
            prepared[key] = (row["seq"], prepared_digest(item))
            run_ids.add(item["identity"]["admission"]["run_id"])
        elif row["type"] == "tool.result":
            key = digest("a3s.code.tool-invocation.v1", identity(payload["identity"]))
            require(key in prepared, "foreign result identity")
            require(prepared[key][0] < row["seq"], "result before preparation")
            require(payload["prepared_digest"] == prepared[key][1], "prepared digest mismatch")
            effects = payload["effects"]
            require(effects["completeness"] == "complete" and effects["changes"] == [], "incomplete or mutated read")
            require(not effects.get("unresolved_external_operations", []), "unsettled read")
            results.append((key, row["seq"], payload["outcome"]))
        elif row["type"] == "turn.complete":
            require(payload["terminal"] == "narrative", "unexpected read terminal")
            keys = payload["receipt_keys"]
            require(len(keys) == 1, "read completion must bind exactly one receipt")
            require(payload["log_cut"]["seq"] < row["seq"], "invalid completion cut")
            require(any(key == keys[0] and seq <= payload["log_cut"]["seq"] for key, seq, _ in results), "completion omitted its prior result")
            require(segment_tokens > 0, "turn has no actual provider usage")
            segment_tokens = 0
            completes.extend(keys)
        elif row["type"] == "model.turn":
            tokens += payload.get("prompt_tokens", 0) or 0
            segment_tokens += payload.get("prompt_tokens", 0) or 0
        elif row["type"] == "user.message":
            messages.append(payload["text_digest"])
    require([outcome for _, _, outcome in results] == expected, "wrong actual terminal outcomes")
    require(len(prepared) == len(results) == len(completes) == len(expected), "incomplete invocation chain")
    require(len(set(completes)) == len(expected) and set(completes) == {key for key, _, _ in results}, "completion identity coverage")
    require(len(run_ids) == len(expected), "each turn must have a distinct actual Run")
    require(tokens > 0, "no actual provider usage")
    require(messages == [r["submitted_message_digest"] for r in requests], "actual submitted message digest mismatch")
    require(value["files"] and all(item["unchanged"] and item["before"] == item["after"] for item in value["files"]), "fixture changed")
    require(json.dumps(value["oracles"], sort_keys=True) == json.dumps(EXPECTED_ORACLES[case], sort_keys=True), "missing or failed variant oracles")
    return {"case": case, "turns": len(completes), "receipts": len(results), "distinct_runs": len(run_ids),
            "outcomes": expected, "prompt_tokens": tokens, "binding_and_effect_checks": "passed"}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    results = []
    for case in EXPECTED_OUTCOMES:
        try:
            results.append(verify_artifact(json.loads((args.directory / f"{case}.json").read_text())))
        except (OSError, ValueError, KeyError, TypeError) as error:
            print(json.dumps({"case": case, "result": "failed", "reason": str(error)}))
            return 1
    print(json.dumps({"kind": "selected-tl01-receipt-validation", "result": "passed", "cases": results,
                      "limitations": ["not a full fact-log hash-chain verification", "not full TUI, platform, provider or release certification"]}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
