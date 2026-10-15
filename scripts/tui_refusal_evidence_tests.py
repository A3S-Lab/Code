#!/usr/bin/env python3
"""Corrupt actual A3S Test recovery evidence and require rejection."""

import argparse
import copy
import json
from pathlib import Path

from tui_refusal_e2e import verify


def fact(rows, kind):
    return next(row for row in rows if row["type"] == kind)


def result_field(field, value):
    def mutate(fixture, rows, events, report):
        target = next(row for row in rows if row["type"] == "tool.result"
                      and row["payload"]["receipt"]["outcome"] == "denied")
        target["payload"]["receipt"][field] = value
    return mutate


def effect_field(field, value):
    def mutate(fixture, rows, events, report):
        fact(rows, "tool.result")["payload"]["receipt"]["effects"][field] = value
    return mutate


def missing(rows, kind):
    rows[:] = [row for row in rows if row["type"] != kind]


def foreign_result(fixture, rows, events, report):
    fact(rows, "tool.result")["payload"]["receipt"]["identity"]["turn"] += 1


def zero_usage(fixture, rows, events, report):
    for row in rows:
        if row["type"] == "model.turn":
            row["payload"]["prompt_tokens"] = 0


def unprepared_last_model_call(fixture, rows, events, report):
    complete = rows[-1]
    assert complete["type"] == "turn.complete"
    seq = complete["seq"]
    complete["seq"] += 1
    complete["payload"]["log_cut"]["seq"] = seq
    rows.insert(-1, {"seq": seq, "type": "model.turn", "payload": {
        "kind": "tool", "prompt_tokens": 100,
        "call": {"id": "unprepared-late-call", "name": "read", "args": {"file_path": "task.txt"}}}})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("actual_case", type=Path)
    args = parser.parse_args()
    root = args.actual_case
    baseline = (json.loads((root / "fixture.json").read_text()), json.loads((root / "private-facts.json").read_text()),
                [json.loads(line) for line in (root / "prompt-trace.jsonl").read_text().splitlines()],
                json.loads((root / "engine-report.json").read_text()))
    verify(*baseline)
    mutations = [
        ("engine-failed", lambda f, r, e, p: p.update(status="failed")),
        ("cleanup-failed", lambda f, r, e, p: p["scenarios"][0].update(cleanup_error={"code": "cleanup-failed"})),
        ("hidden-engine-retry", lambda f, r, e, p: p["scenarios"][0]["steps"][0].update(attempts=2)),
        ("queued-next-prompt", lambda f, r, e, p: e[2].update(in_flight_before=1)),
        ("foreign-response", lambda f, r, e, p: e[1].update(prompt_id="foreign")),
        ("wrong-submitted-prompt", lambda f, r, e, p: e[0].update(prompt_digest="sha256:" + "0" * 64)),
        ("prompt-response-error", lambda f, r, e, p: e[3].update(error_code=-32603)),
        ("missing-prompt-response", lambda f, r, e, p: e.pop()),
        ("Unknown-with-same-diagnostic", result_field("outcome", "unknown")),
        ("executed-failure-is-not-refusal", result_field("outcome", "failed")),
        ("invented-refusal-exit-code", result_field("exit_code", 1)),
        ("wrong-prepared-digest", result_field("prepared_digest", "sha256:" + "0" * 64)),
        ("incomplete-effects", effect_field("completeness", "incomplete")),
        ("unsettled-operation", effect_field("unresolved_external_operations", ["unsettled"])),
        ("observed-hook-effect", effect_field("changes", [{"path": "hook-effect.txt"}])),
        ("foreign-result-identity", foreign_result),
        ("missing-preparation", lambda f, r, e, p: missing(r, "tool.prepared")),
        ("missing-original-result", lambda f, r, e, p: missing(r, "tool.result")),
        ("missing-completion", lambda f, r, e, p: missing(r, "turn.complete")),
        ("missing-actual-input", lambda f, r, e, p: missing(r, "user.message")),
        ("zero-provider-usage", zero_usage),
        ("missing-approval", lambda f, r, e, p: missing(r, "confirmation.answered")),
        ("approval-denied", lambda f, r, e, p: fact(r, "confirmation.answered")["payload"].update(approved=False)),
        ("foreign-approval", lambda f, r, e, p: fact(r, "confirmation.answered")["payload"].update(tool_call_id="foreign")),
        ("false-Verified", lambda f, r, e, p: fact(r, "turn.complete")["payload"].update(terminal="verified")),
        ("wrong-completion-cut", lambda f, r, e, p: fact(r, "turn.complete")["payload"]["log_cut"].update(seq=0)),
        ("wrong-completion-receipt", lambda f, r, e, p: fact(r, "turn.complete")["payload"].update(receipt_keys=[])),
        ("duplicate-fact-sequence", lambda f, r, e, p: r.append(copy.deepcopy(r[-1]))),
        ("late-unprepared-model-call", unprepared_last_model_call),
    ]
    rejected = []
    for name, mutate in mutations:
        candidate = copy.deepcopy(baseline)
        mutate(*candidate)
        try:
            verify(*candidate)
        except (ValueError, KeyError, TypeError, IndexError):
            rejected.append(name)
        else:
            raise AssertionError("corrupt actual evidence accepted: " + name)
    print(json.dumps({"actual_baselines_verified": 1, "corrupted_copies_rejected": len(rejected),
                      "checks": rejected, "limitations": ["corrupted copies are not actual live Unknown/hook fault injection"]}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
