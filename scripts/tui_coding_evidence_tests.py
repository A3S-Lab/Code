#!/usr/bin/env python3
"""Reject corrupt copies of actual coding evidence; no synthetic live baseline."""

import argparse
import copy
import json
from pathlib import Path

from verify_tui_coding_evidence import verify_artifact


def event(index, field, value, response=False):
    def mutate(public, raw):
        key = "response" if response else "request"
        public["settled_prompts"][index][key][field] = value
        public["prompt_events"][index * 2 + response][field] = value
    return mutate


def row(public, kind, phase=0):
    interval = public["settled_prompts"][phase]
    return next(r for r in public["facts"] if r["type"] == kind
                and interval["first_fact_seq"] <= r["seq"] <= interval["last_fact_seq"])


def result_field(field, value):
    def mutate(public, raw):
        target = row(public, "tool.result", 1)
        target["payload"][field] = value
        original = next(r for r in raw if r["seq"] == target["seq"])
        original["payload"]["receipt"][field] = value
    return mutate


def effects(field, value):
    def mutate(public, raw):
        target = row(public, "tool.result", 1)
        target["payload"]["effects"][field] = value
        original = next(r for r in raw if r["seq"] == target["seq"])
        original["payload"]["receipt"]["effects"][field] = value
    return mutate


def final_reply(field, value):
    def mutate(public, raw):
        interval = public["settled_prompts"][3]
        target = next(r for r in reversed(public["facts"]) if r["type"] == "model.turn"
                      and interval["first_fact_seq"] <= r["seq"] <= interval["last_fact_seq"])
        target["payload"][field] = value
        original = next(r for r in raw if r["seq"] == target["seq"])
        if field == "quotes_checker_output":
            original["payload"]["text"] = "completed"
        else:
            original["payload"][field] = value
    return mutate


def raw_failed(field, value):
    def mutate(public, raw):
        interval = public["settled_prompts"][1]
        target = next(r for r in public["facts"] if r["type"] == "tool.result"
                      and interval["first_fact_seq"] <= r["seq"] <= interval["last_fact_seq"]
                      and r["payload"]["exit_code"] == 1)
        original = next(r for r in raw if r["seq"] == target["seq"])
        if field == "outcome":
            target["payload"][field] = value
            original["payload"]["receipt"][field] = value
        else:
            wire = json.loads(original["payload"]["receipt"]["output"])
            wire["metadata"]["exit_code"] = value
            original["payload"]["receipt"]["output"] = json.dumps(wire)
            target["payload"]["exit_code"] = value
    return mutate


def stale_effect(public, raw):
    complete = row(public, "turn.complete", 2)
    expected = "a" * 64
    complete["payload"]["effect_digest"] = expected
    next(r for r in raw if r["seq"] == complete["seq"])["payload"]["effect_digest"] = expected
    interval = public["settled_prompts"][2]
    result = next(r for r in public["facts"] if r["type"] == "tool.result"
                  and interval["first_fact_seq"] <= r["seq"] <= interval["last_fact_seq"]
                  and r["payload"]["verification_status"] == "passed")
    result["payload"]["verification_effect_digest"] = expected
    original = next(r for r in raw if r["seq"] == result["seq"])
    wire = json.loads(original["payload"]["receipt"]["output"])
    wire["metadata"]["verification_report"]["effect_digest"] = expected
    original["payload"]["receipt"]["output"] = json.dumps(wire)


def omit_failed_quote(public, raw):
    interval = public["settled_prompts"][1]
    reply = next(r for r in reversed(raw) if r["type"] == "model.turn"
                 and interval["first_fact_seq"] <= r["seq"] <= interval["last_fact_seq"])
    reply["payload"]["text"] = "DUAL-GATE-7K"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifact", type=Path)
    parser.add_argument("private_facts", type=Path)
    args = parser.parse_args()
    actual, originals = json.loads(args.artifact.read_text()), json.loads(args.private_facts.read_text())
    baseline = verify_artifact(actual, originals)
    if not all(baseline["private_final_reply_coverage"]):
        raise ValueError("negative controls require a complete actual baseline")
    mutations = [
        ("unfinished", lambda p, r: p.update(assertions_finished=False)),
        ("panicked", lambda p, r: p.update(test_panicked=True)),
        ("changed-checker", lambda p, r: p.update(checker_unchanged=False)),
        ("queued-request", event(1, "in_flight_before", 1)),
        ("foreign-RPC-response", event(1, "rpc_id", 999, True)),
        ("foreign-prompt-response", event(1, "prompt_id", "foreign", True)),
        ("foreign-session-response", event(1, "session_id", "foreign", True)),
        ("wrong-prompt-digest", event(1, "prompt_digest", "sha256:" + "0" * 64)),
        ("early-next-request", event(1, "seq", 1)),
        ("partial-checkpoint-fake-success", event(1, "error_code", None, True)),
        ("partial-gate-flag-omitted", event(1, "completion_gate", False, True)),
        ("success-response-is-error", event(3, "error_code", -32603, True)),
        ("duplicate-Run", lambda p, r: p["settled_prompts"][3].update(run_id=p["settled_prompts"][2]["run_id"])),
        ("missing-request-phase", lambda p, r: p["settled_prompts"].pop()),
        ("missing-actual-user-input", lambda p, r: r.__setitem__(slice(None), [v for v in r if v["type"] != "user.message"])),
        ("wrong-actual-input", lambda p, r: next(v for v in r if v["type"] == "user.message")["payload"].update(text="different")),
        ("Unknown-outcome", result_field("outcome", "unknown")),
        ("incomplete-effects", effects("completeness", "incomplete")),
        ("unresolved-external-operation", effects("unresolved_external_operations", ["unresolved"])),
        ("preparation-digest-tamper", result_field("prepared_digest", "sha256:" + "0" * 64)),
        ("Denied-is-not-failed-execution", raw_failed("outcome", "denied")),
        ("missing-actual-failed-exit", raw_failed("exit_code", None)),
        ("failed-checker-reported-exit-zero", raw_failed("exit_code", 0)),
        ("missing-failed-checker-quote", omit_failed_quote),
        ("fake-final-quote", final_reply("quotes_checker_output", False)),
        ("zero-final-provider-usage", final_reply("prompt_tokens", 0)),
        ("stale-report-and-completion-agree", stale_effect),
        ("missing-original-facts", lambda p, r: r.clear()),
        ("missing-public-original-fact", lambda p, r: p["facts"].pop(0)),
        ("duplicate-fact-sequence", lambda p, r: p["facts"].append(copy.deepcopy(p["facts"][-1]))),
    ]
    rejected = []
    for name, mutate in mutations:
        public, raw = copy.deepcopy(actual), copy.deepcopy(originals)
        mutate(public, raw)
        try:
            verify_artifact(public, raw)
        except (ValueError, KeyError, TypeError, IndexError):
            rejected.append(name)
        else:
            raise AssertionError("corrupted actual evidence accepted: " + name)
    print(json.dumps({"actual_baselines_verified": 1, "corrupted_copies_rejected": len(rejected),
                      "checks": rejected, "limitations": ["negative controls are corrupt copies, not paid live runs"]}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
