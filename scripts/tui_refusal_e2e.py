#!/usr/bin/env python3
"""Run the actual A3S Test TUI engine and audit known-refusal recovery facts."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import signal
import shutil
import subprocess
import sys
import time
import uuid

from verify_tui_coding_evidence import prepared_digest, prompt_digest
from verify_tui_read_evidence import digest, identity, require


REFUSAL_PROMPT = "Use read once on task.txt, then follow its instructions exactly."
READ_PROMPT = "Read recovery.txt exactly once. Quote its exact contents. Do not call other tools."
COMMAND = "printf must-not-run > must-not-run.txt"
PROJECT = Path(__file__).resolve().parents[3]
SUITES = PROJECT / "apps/code/acp/tests/a3s-test"
RELAY = PROJECT / "apps/code/acp/tests/tui_913_real_llm/acp_trace_proxy.py"


def refusal_prompt(fixture: dict) -> str:
    if fixture.get("workflow", "task-file") == "task-file":
        return REFUSAL_PROMPT
    arguments = {"command": COMMAND} if fixture["case"] == "bash" else {
        "action": "start", "name": "probe", "command": COMMAND}
    return (f"Invoke {fixture['case']} exactly once with these exact arguments: "
            + json.dumps(arguments)
            + ". I authorize this invocation. Use the default sandbox. Do not escalate or retry. "
            + "Do not invoke any other tool. After observing its refusal, reply REFUSAL-OBSERVED.")


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def private_json(path: Path, value) -> None:
    with open(path, "x", opener=lambda name, flags: os.open(name, flags, 0o600)) as stream:
        json.dump(value, stream, indent=2)
        stream.write("\n")


def read_facts(root: Path) -> list[dict]:
    rows = []
    for directory in (root / "workspace/.a3s", root / "home/.a3s/logs"):
        for path in sorted(directory.rglob("*")):
            if path.is_symlink() or not path.is_file() or path.suffix not in (".jsonl", ".log"):
                continue
            for line in path.read_text(errors="replace").splitlines():
                try:
                    value = json.loads(line)
                except ValueError:
                    continue
                if isinstance(value, dict) and "seq" in value and "type" in value:
                    rows.append(value)
    return rows


def verify(fixture: dict, facts: list[dict], events: list[dict], report: dict) -> dict:
    require(report["status"] == "passed" and len(report["scenarios"]) == 1, "engine scenario did not pass")
    require(report["scenarios"][0]["status"] == "passed" and report["scenarios"][0].get("cleanup_error") is None, "scenario or cleanup failed")
    require(all(step["status"] == "passed" and step["attempts"] == 1
                for step in report["scenarios"][0]["steps"]), "engine step failed or retried")
    require(len(events) == 4, "two actual prompt requests and responses required")
    sessions, prompt_ids, rpc_ids = set(), set(), set()
    for index, prompt in enumerate((refusal_prompt(fixture), READ_PROMPT)):
        request, response = events[index * 2:index * 2 + 2]
        require(request["kind"] == "prompt-request" and response["kind"] == "prompt-response", "event type")
        require(request["in_flight_before"] == 0, "new prompt overlapped the previous request")
        require(request["prompt_digest"] == prompt_digest(prompt), "submitted prompt mismatch")
        require(request["seq"] == index * 2 + 1 and response["seq"] == index * 2 + 2, "prompt order")
        for key in ("rpc_id", "prompt_id", "session_id", "prompt_digest"):
            require(request[key] == response[key] and request[key] not in (None, ""), "foreign response")
        require(response["error_code"] is None and response["stop_reason"] == "end_turn", "prompt did not end successfully")
        sessions.add(request["session_id"])
        prompt_ids.add(request["prompt_id"])
        rpc_ids.add(request["rpc_id"])
    require(len(sessions) == 1 and len(prompt_ids) == len(rpc_ids) == 2, "session or prompt identity")
    seqs = [row["seq"] for row in facts]
    require(all(type(seq) is int and seq > 0 for seq in seqs), "invalid fact sequence")
    require(seqs == sorted(set(seqs)), "nonunique or unordered actual facts")
    prepared, results, calls, runs, completions, inputs, replies = {}, {}, {}, set(), [], [], []
    safe_facts, phase_tokens, token_totals, phase_keys, approvals = [], 0, [], set(), []
    last_model, last_result_seq = None, 0
    for row in facts:
        p, kind = row["payload"], row["type"]
        if kind == "user.message":
            inputs.append(p["text"])
        elif kind == "model.turn":
            last_model = row
            require(type(p.get("prompt_tokens")) is int and p["prompt_tokens"] > 0, "missing provider usage")
            phase_tokens += p["prompt_tokens"]
            if p["kind"] == "text":
                replies.append(row)
            for call in ([p["call"]] if "call" in p else []) + p.get("also", []):
                require(call["id"] not in calls, "duplicate model call")
                calls[call["id"]] = (row["seq"], call)
            safe_facts.append({"seq": row["seq"], "type": kind,
                               "payload": {"kind": p["kind"], "prompt_tokens": p["prompt_tokens"]}})
        elif kind == "tool.prepared":
            item = p["prepared"]
            ident = identity(item["identity"])
            key = digest("a3s.code.tool-invocation.v1", ident)
            require(key not in prepared and ident["admission"]["thread_id"] in sessions, "foreign or duplicate preparation")
            seq, call = calls[ident["tool_call_id"]]
            require(seq < row["seq"] and call["name"] == item["tool_name"], "model/preparation binding")
            expected_args = json.loads(json.dumps(call["args"], sort_keys=True))
            require(digest("a3s.code.tool-arguments.v1", expected_args) == item["original_args_digest"] == item["effective_args_digest"], "argument digest binding")
            prepared[key] = (row["seq"], prepared_digest(item), item, call)
            runs.add(ident["admission"]["run_id"])
            safe_facts.append(row)
        elif kind == "tool.result":
            receipt = p["receipt"]
            key = digest("a3s.code.tool-invocation.v1", identity(receipt["identity"]))
            require(key in prepared and key not in results, "foreign or duplicate result")
            seq, expected, _, _ = prepared[key]
            require(receipt["schema"] == "a3s.code.tool-result.v1" and seq < row["seq"] and receipt["prepared_digest"] == expected, "original result binding")
            require(receipt["outcome"] in ("succeeded", "denied"), "failed or Unknown invocation")
            effects = receipt["effects"]
            require(effects["completeness"] == "complete" and effects["changes"] == [] and effects.get("unresolved_external_operations", []) == [], "uncertain or mutated result")
            results[key] = row
            last_result_seq = row["seq"]
            phase_keys.add(key)
            safe_facts.append({"seq": row["seq"], "type": kind, "payload": {
                name: receipt[name] for name in ("identity", "prepared_digest", "outcome", "effects")}})
        elif kind == "confirmation.answered":
            approvals.append(row)
            safe_facts.append(row)
        elif kind == "turn.complete":
            require(p["terminal"] == "narrative" and p["effect_digest"] is None, "false verification or mutation")
            require(set(p["receipt_keys"]) == phase_keys and len(p["receipt_keys"]) == len(phase_keys), "completion receipt coverage")
            require(p.get("background_receipt_keys", []) == [], "unexpected background settlement")
            require(replies and last_model is not None and last_model["payload"]["kind"] == "text"
                    and last_model["seq"] == replies[-1]["seq"]
                    and last_result_seq < replies[-1]["seq"] <= p["log_cut"]["seq"] < row["seq"], "actual last provider reply must follow results inside cut")
            marker = fixture["refusal_marker"] if not completions else fixture["read_marker"]
            require(marker in replies[-1]["payload"]["text"], "final assistant omitted the observed marker")
            completions.append(row)
            token_totals.append(phase_tokens)
            phase_tokens, phase_keys = 0, set()
            safe_facts.append(row)
        elif kind.startswith("background."):
            raise ValueError("refused launch emitted a background fact")
    require(inputs == [refusal_prompt(fixture), READ_PROMPT], "actual user message mismatch")
    require(len(completions) == len(runs) == 2 and all(token_totals) and not phase_keys, "two complete actual Runs required")
    task_file = fixture.get("workflow", "task-file") == "task-file"
    require(set(prepared) == set(results) and len(results) == len(calls) == (3 if task_file else 2), "exact original invocation coverage required")
    requested = [(item[2]["tool_name"], item[3]["args"]) for item in prepared.values()]
    command_args = {"command": COMMAND} if fixture["case"] == "bash" else {"action": "start", "name": "probe", "command": COMMAND}
    expected = ([("read", {"file_path": "task.txt"})] if task_file else [])
    expected += [(fixture["case"], command_args), ("read", {"file_path": "recovery.txt"})]
    require(requested == expected, "wrong tool sequence or arguments")
    ordered_results = list(results.values())
    outcomes = (["succeeded"] if task_file else []) + ["denied", "succeeded"]
    require([r["payload"]["receipt"]["outcome"] for r in ordered_results] == outcomes, "wrong actual outcomes")
    denied = ordered_results[-2]["payload"]["receipt"]
    command_key = list(results)[-2]
    require(denied["exit_code"] is None and prepared[command_key][2].get("baseline") is None, "refusal was not pre-execution")
    command_call = prepared[command_key][3]
    require(command_call["needs_confirmation"] is True and len(approvals) == 1
            and approvals[0]["payload"] == {"approved": True, "tool_call_id": command_call["id"]}
            and calls[command_call["id"]][0] < approvals[0]["seq"] < prepared[command_key][0], "missing exact per-invocation approval")
    require("native_process_lifetime_unqualified" in denied["output"], "wrong refusal reason")
    require(fixture["read_marker"] in ordered_results[-1]["payload"]["receipt"]["output"], "read did not observe the fresh file")
    first_runs = {prepared[key][2]["identity"]["admission"]["run_id"] for key in completions[0]["payload"]["receipt_keys"]}
    second_runs = {prepared[key][2]["identity"]["admission"]["run_id"] for key in completions[1]["payload"]["receipt_keys"]}
    require(len(first_runs) == len(second_runs) == 1 and first_runs.isdisjoint(second_runs), "foreign Run completion")
    return {"schema": "a3s.test.tui-refusal-evidence.v1", "case": fixture["case"],
            "workflow": fixture.get("workflow", "task-file"), "result": "passed",
            "engine": "a3s-test", "distinct_runs": 2, "known_denied": 1, "successful_reads": 2 if task_file else 1,
            "unknown": 0, "prompt_tokens_by_run": token_totals, "prompt_events": events, "facts": safe_facts,
            "limitations": ["known pre-execution refusal only; not default native execution or post-dispatch uncertainty qualification",
                            "not complete catalog, adapter/platform matrix or full fact-log hash-chain authentication"]}


def owned_control_processes(root: Path, fixture: dict) -> list[int]:
    found = []
    rows = subprocess.check_output(["ps", "-axo", "pid,ppid,command"], text=True).splitlines()[1:]
    for row in rows:
        parts = row.split(None, 2)
        if len(parts) != 3:
            continue
        try:
            argv = shlex.split(parts[2])
        except ValueError:
            continue
        if not argv:
            continue
        if argv[0] in (fixture["pager"], fixture["acp"]) and str(root / "workspace") in argv:
            found.append(int(parts[0]))
        elif str(root / "acp-trace-proxy") in argv:
            found.append(int(parts[0]))
    return found


def run_engine(command: list[str], root: Path, fixture: dict) -> tuple[int, str | None]:
    environment = os.environ.copy()
    # The credential helper's exclusive ACL stays beneath this owned, private
    # root even if its process is interrupted before NamedTempFile Drop.
    environment["TMPDIR"] = str(root / "tmp")
    with (root / "engine-report.json").open("wb") as out, (root / "engine-stderr.log").open("wb") as err:
        child = subprocess.Popen(command, cwd=root, env=environment, stdout=out, stderr=err)
        deadline, cancel_deadline, reason = time.monotonic() + 360, None, None
        while child.poll() is None:
            if reason is None:
                trace = root / "prompt-trace.jsonl"
                try:
                    events = [json.loads(line) for line in trace.read_text().splitlines()]
                except (OSError, ValueError):
                    events = []
                if len(events) >= 2 and events[1]["kind"] == "prompt-response":
                    facts = read_facts(root)
                    completed = any(row["type"] == "turn.complete" for row in facts)
                    invoked = any(row["type"] == "tool.prepared" and row["payload"]["prepared"]["tool_name"] == fixture["case"] for row in facts)
                    if completed and not invoked:
                        reason = "first_request_ended_without_required_invocation"
                    elif events[1].get("error_code") is not None:
                        reason = "first_request_error"
                if time.monotonic() >= deadline:
                    reason = "campaign_deadline"
                if reason is not None:
                    child.send_signal(signal.SIGINT)
                    cancel_deadline = time.monotonic() + 15
            elif cancel_deadline is not None and time.monotonic() >= cancel_deadline:
                child.kill()
                break
            time.sleep(0.1)
        return child.wait(), reason


def launch(root: Path) -> None:
    require(os.environ.get("A3S_CODE_ACL_ENV_PREPARED") == "1", "launch requires the ACL credential helper")
    fixture = json.loads((root / "fixture.json").read_text())
    child_env = {key: os.environ[key] for key in ("A3S_OPENAI_API_KEY", "A3S_OPENAI_BASE_URL") if key in os.environ}
    child_env.update(A3S_CONFIG=os.environ["A3S_CONFIG_FILE"], HOME=str(root / "home"), GROK_HOME=str(root / "home/grok"),
                     A3S_DISABLE_CC_SWITCH="1", A3S_DISABLE_GROK_ACCOUNT="1", A3S_DISABLE_CURSOR_ACCOUNT="1",
                     A3S_ACP_AGENT_BIN=str(root / "acp-trace-proxy"), A3S_CODE_TUI_TEST_ACP_REAL=fixture["acp"],
                     A3S_CODE_TUI_TEST_ACP_TRACE=str(root / "prompt-trace.jsonl"),
                     TERM="xterm-256color", PATH="/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin", TMPDIR=str(root / "tmp"))
    os.execve(fixture["pager"], [fixture["pager"], "--cwd", str(root / "workspace")], child_env)


def run(args) -> int:
    require(sys.platform == "darwin", "native refusal fixture currently requires macOS")
    root = args.output.resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    for name in ("workspace", "home/grok", "tmp"):
        (root / name).mkdir(parents=True)
    suite = SUITES / ("native_refusal_recovery.acl" if args.workflow == "task-file" else f"native_{args.case}_refusal_recovery.acl")
    fixture = {"case": args.case, "workflow": args.workflow,
               "pager": str(args.pager.resolve()), "acp": str(args.acp.resolve()),
               "refusal_marker": "REFUSAL-CONFIRMED-" + str(uuid.uuid4()) if args.workflow == "task-file" else "REFUSAL-OBSERVED",
               "read_marker": "RECOVERED-" + str(uuid.uuid4())}
    private_json(root / "fixture.json", fixture)
    task = (f"Call {args.case} exactly once with the following exact arguments: "
            + json.dumps({"command": COMMAND} if args.case == "bash" else {"action": "start", "name": "probe", "command": COMMAND})
            + ". This required tool call is authorized by the user. Use the default sandbox. "
            + "Do not escalate or retry. Do not invoke write/edit tools or any additional command. "
            + "Only after actually invoking the required tool and observing its refusal, reply with the concatenation of "
            + json.dumps("REFUSAL") + " and " + json.dumps(fixture["refusal_marker"][len("REFUSAL"):]) + ".")
    (root / "workspace/task.txt").write_text(task)
    (root / "workspace/recovery.txt").write_text(fixture["read_marker"])
    (root / "workspace/sentinel.txt").write_bytes(b"unchanged")
    shutil.copyfile(RELAY, root / "acp-trace-proxy")
    (root / "acp-trace-proxy").chmod(0o700)
    fingerprints = {str(path): sha256(path) for path in (args.pager.resolve(), args.acp.resolve(), args.acl_helper.resolve(), args.engine.resolve(), args.config.resolve(), suite, RELAY, Path(__file__).resolve())}
    check = subprocess.run([str(args.engine), "check", str(suite), "--json"], capture_output=True)
    (root / "suite-check.json").write_bytes(check.stdout)
    require(check.returncode == 0, "ACL suite validation failed")
    command = [str(args.engine), "run", str(suite), "--tui-executable", str(args.acl_helper),
               "--tui-working-directory", str(root / "workspace"), "--tui-columns", "120", "--tui-rows", "45",
               "--command-timeout-ms", "240000", "--infrastructure-retries", "0", "--json"]
    for arg in ("run", str(args.config.resolve()), sys.executable, str(Path(__file__).resolve()), "_launch", str(root)):
        command.append("--tui-arg=" + arg)
    started = time.monotonic()
    code, early_failure = run_engine(command, root, fixture)
    facts = read_facts(root)
    private_json(root / "private-facts.json", facts)
    events = [json.loads(line) for line in (root / "prompt-trace.jsonl").read_text().splitlines()]
    remaining = owned_control_processes(root, fixture)
    retired = []
    if not remaining:
        for path in (root / "tmp").glob("a3s-code-env-*.acl"):
            require(path.is_file() and not path.is_symlink(), "unexpected temporary ACL object")
            retired.append({"name": path.name, "sha256": sha256(path)})
            path.unlink()
    status = {"engine_exit_code": code, "seconds": round(time.monotonic() - started, 3),
              "early_failure": early_failure, "owned_pager_acp_relay_remaining": remaining,
              "retired_private_temporary_acls": retired,
              "temporary_acl_remaining": len(list((root / "tmp").glob("a3s-code-env-*.acl"))),
              "fingerprints": fingerprints, "private_facts_sha256": sha256(root / "private-facts.json")}
    private_json(root / "run-status.json", status)
    require(code == 0 and early_failure is None, "actual TUI engine run failed")
    report = verify(fixture, facts, events, json.loads((root / "engine-report.json").read_text()))
    require(not (root / "workspace/must-not-run.txt").exists(), "refused command wrote a file")
    require((root / "workspace/sentinel.txt").read_bytes() == b"unchanged", "sentinel changed")
    require((root / "workspace/task.txt").read_text() == task and (root / "workspace/recovery.txt").read_text() == fixture["read_marker"], "fixture source changed")
    require(not owned_control_processes(root, fixture), "owned pager/ACP/relay still running after engine cleanup")
    require(all(sha256(Path(path)) == expected for path, expected in fingerprints.items()), "candidate/config/suite changed during execution")
    report.update(command_sentinel_exists=False, sentinel_unchanged=True, fixture_unchanged=True,
                  owned_pager_acp_relay_remaining=0)
    (root / "evidence.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"case": args.case, "workflow": args.workflow, "result": "passed", "engine": "a3s-test", "distinct_runs": 2,
                      "known_denied": 1, "unknown": 0, "seconds": status["seconds"]}))
    return 0


def main() -> int:
    if len(sys.argv) == 3 and sys.argv[1] == "_launch":
        launch(Path(sys.argv[2]))
        return 1
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", choices=("bash", "background"), required=True)
    parser.add_argument("--workflow", choices=("task-file", "direct"), default="task-file")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--pager", type=Path, required=True)
    parser.add_argument("--acp", type=Path, required=True)
    parser.add_argument("--acl-helper", type=Path, required=True)
    parser.add_argument("--engine", type=Path, default=Path(shutil.which("a3s-test") or "a3s-test"))
    args = parser.parse_args()
    try:
        return run(args)
    except (OSError, ValueError, KeyError, TypeError, subprocess.TimeoutExpired) as error:
        # Credentialed subprocess and original fact contents stay private.
        print(json.dumps({"case": args.case, "result": "failed", "error_type": type(error).__name__,
                          "evidence_directory": str(args.output)}))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
