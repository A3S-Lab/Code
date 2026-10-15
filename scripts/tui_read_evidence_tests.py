#!/usr/bin/env python3
"""Reject corrupted copies of actual TL01 evidence; never fabricate live passes."""

import argparse
import copy
import json
from pathlib import Path

from verify_tui_read_evidence import EXPECTED_OUTCOMES, verify_artifact


def fact(value, kind):
    return next(row for row in value["facts"] if row["type"] == kind)


def field(kind, key, replacement):
    def mutate(value):
        fact(value, kind)["payload"][key] = replacement
    return mutate


def effect(key, replacement):
    def mutate(value):
        fact(value, "tool.result")["payload"]["effects"][key] = replacement
    return mutate


def omit(kind):
    def mutate(value):
        value["facts"] = [row for row in value["facts"] if row["type"] != kind]
    return mutate


def no_usage(value):
    for row in value["facts"]:
        if row["type"] == "model.turn":
            row["payload"]["prompt_tokens"] = 0


def first_turn_no_usage(value):
    for row in value["facts"]:
        if row["type"] == "turn.complete":
            break
        if row["type"] == "model.turn":
            row["payload"]["prompt_tokens"] = 0


def foreign_identity(value):
    fact(value, "tool.result")["payload"]["identity"]["turn"] += 1


def wrong_cut(value):
    fact(value, "turn.complete")["payload"]["log_cut"]["seq"] = 0


def duplicate_fact(value):
    value["facts"].append(copy.deepcopy(value["facts"][-1]))


def changed_requested_args(value):
    value["requests"][0]["requested_args"]["file_path"] = "different.txt"


def failed_oracle(value):
    value["oracles"][0]["utf8_preserved"] = False


def changed_fixture(value):
    value["files"][0]["after"] = "sha256:" + "0" * 64


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path, help="five artifacts from an actual passing run")
    args = parser.parse_args()
    actual = {case: json.loads((args.directory / f"{case}.json").read_text()) for case in EXPECTED_OUTCOMES}
    for artifact in actual.values():
        verify_artifact(artifact)
    mutations = [
        ("unfinished-assertions", "01", lambda v: v.update(assertions_finished=False)),
        ("panicked-test", "01", lambda v: v.update(test_panicked=True)),
        ("unknown-outcome", "01", field("tool.result", "outcome", "unknown")),
        ("incomplete-effects", "01", effect("completeness", "incomplete")),
        ("read-mutated-file", "01", effect("changes", [{"path": "unicode.txt"}])),
        ("unsettled-operation", "01", effect("unresolved_external_operations", [{"id": "unsettled"}])),
        ("foreign-result", "01", foreign_identity),
        ("prepared-digest-tamper", "01", field("tool.result", "prepared_digest", "sha256:" + "0" * 64)),
        ("missing-preparation", "01", omit("tool.prepared")),
        ("missing-completion", "01", omit("turn.complete")),
        ("missing-submission", "01", omit("user.message")),
        ("duplicate-fact-sequence", "01", duplicate_fact),
        ("wrong-completion-terminal", "01", field("turn.complete", "terminal", "verified")),
        ("receipt-outside-completion-cut", "01", wrong_cut),
        ("wrong-completion-key", "01", field("turn.complete", "receipt_keys", ["sha256:" + "0" * 64])),
        ("wrong-requested-arguments", "01", changed_requested_args),
        ("missing-request", "01", lambda v: v.update(requests=[])),
        ("wrong-request-digest", "01", lambda v: v["requests"][0].update(requested_args_digest="sha256:" + "0" * 64)),
        ("input-mismatch", "01", lambda v: v["requests"][0].update(submitted_message_matches=False)),
        ("actual-submission-tamper", "01", field("user.message", "text_digest", "sha256:" + "0" * 64)),
        ("failed-variant-oracle", "01", failed_oracle),
        ("zero-provider-usage", "01", no_usage),
        ("one-turn-missing-usage", "04", first_turn_no_usage),
        ("changed-fixture-digest", "01", changed_fixture),
        ("missing-fixture", "01", lambda v: v.update(files=[])),
        ("wrong-boundary-outcome", "05", field("tool.result", "outcome", "failed")),
    ]
    results = []
    for name, suffix, mutate in mutations:
        value = copy.deepcopy(actual[f"TUI-TL01-{suffix}"])
        mutate(value)
        try:
            verify_artifact(value)
        except (ValueError, KeyError, TypeError) as error:
            results.append({"mutation": name, "result": "rejected", "reason": str(error)})
        else:
            raise AssertionError(f"corrupt real evidence was accepted: {name}")
    print(json.dumps({"actual_baselines_verified": len(actual), "corrupted_copies_rejected": len(results),
                      "checks": results, "limitations": ["does not authenticate the omitted full fact-log hash chain"]}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
