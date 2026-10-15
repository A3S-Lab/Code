"""Explicit v9.1.3 live gate; requires A3S_CONFIG_FILE, never skips."""

import importlib.metadata
import json
import os
from pathlib import Path
import tempfile

from a3s_code import Agent, AutoDelegationConfig, PermissionPolicy, SessionOptions


def main():
    config = Path(os.environ["A3S_CONFIG_FILE"])
    assert config.is_file(), "A3S_CONFIG_FILE must name a credentialed ACL file"
    assert importlib.metadata.version("a3s-code") == "9.1.3"
    agent = Agent.create(str(config))
    marker = "SDK-LIVE-913:PYTHON:42"
    receipts = []
    try:
        with tempfile.TemporaryDirectory(prefix="a3s-python-live-release-") as root:
            options = SessionOptions()
            options.session_id = "python-release-real-913"
            options.planning_mode = "disabled"
            options.manual_delegation_enabled = False
            options.auto_delegation = AutoDelegationConfig(enabled=False)
            options.continuation_enabled = False
            options.max_tool_rounds = 2
            options.llm_api_timeout_ms = 120_000
            options.sandbox_network = "deny"
            options.permission_policy = PermissionPolicy(default_decision="deny")
            session = agent.session(root, options)
            try:
                previous = None
                for prompt in (
                    f"Remember the verification code {marker}. Reply with exactly that code. Do not use tools.",
                    "Return only the verification code from my previous message. Do not use tools.",
                ):
                    session.set_permission_policy(PermissionPolicy(default_decision="deny"))
                    admitted = session.stream_with_run(prompt)
                    run = admitted["run"]
                    assert run is not None and run.session_id == session.session_id
                    if previous is not None:
                        assert run.id != previous.id
                        assert previous.cancel() is False
                    types = [event.type for event in admitted["stream"]]
                    snapshot = run.snapshot()
                    assert snapshot["id"] == run.id
                    assert snapshot["status"] == "completed"
                    assert snapshot["result_text"].strip() == marker
                    assert "text_delta" in types and types.count("agent_end") == 1
                    assert "tool_execution_start" not in types
                    target = {"schema": "a3s.code.execution-target.v1", "session_id": session.session_id, "run_id": run.id}
                    evidence = session.read_evaluation_evidence({"target": target})
                    assert evidence["target"] == target
                    try:
                        session.read_evaluation_evidence({"target": dict(target, session_id="foreign")})
                    except RuntimeError as error:
                        assert "not found" in str(error).lower()
                    else:
                        raise AssertionError("foreign evidence must refuse")
                    receipts.append({"run_id": run.id, "status": snapshot["status"], "result": snapshot["result_text"], "event_types": sorted(set(types)), "evidence_target": evidence["target"]})
                    previous = run
                print("SDK_RELEASE_REAL " + json.dumps({"sdk": "python", "version": "9.1.3", "model": session.model_name, "session_id": session.session_id, "receipts": receipts}))
            finally:
                session.close()
    finally:
        agent.close()


if __name__ == "__main__":
    main()
