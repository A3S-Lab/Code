"""Explicit live gate; requires A3S_CONFIG_FILE, never skips.
One real Run per session proves the four work-policy x goal modes."""

import json
import os
import tempfile
import time
from pathlib import Path

from a3s_code import Agent, AutoDelegationConfig, PermissionPolicy, SessionOptions

PROMPT = "Reply with exactly the word READY. Do not use tools."


def _options(session_id, work_policy=None):
    options = SessionOptions()
    options.session_id = session_id
    options.planning_mode = "disabled"
    options.manual_delegation_enabled = False
    options.auto_delegation = AutoDelegationConfig(enabled=False)
    options.continuation_enabled = False
    options.max_tool_rounds = 2
    options.llm_api_timeout_ms = 120_000
    options.sandbox_network = "deny"
    options.permission_policy = PermissionPolicy(default_decision="deny")
    options.work_policy = work_policy
    return options


def _create(session, digest):
    return {
        "kind": "create", "operation_id": "create", "goal_id": "release",
        "session_id": session.session_id,
        "workspace": {"id": session.workspace, "display_root": session.workspace},
        "goal": {
            "description": "Report the release code", "success_criteria": ["Reported exactly"],
            "progress": 0.0, "achieved": False, "created_at": int(time.time()), "achieved_at": None,
        },
        "non_goals": [], "scope_fact_sequences": [], "work_policy_digest": digest,
        "expected_goal_revision": None, "expected_plan_revision": None, "expected_goal_sequence": None,
    }


def main():
    config = Path(os.environ["A3S_CONFIG_FILE"])
    assert config.is_file(), "A3S_CONFIG_FILE must name a credentialed ACL file"
    agent = Agent.create(str(config))
    started = time.monotonic()

    plain = agent.session(tempfile.mkdtemp(), _options("python-goal-live-plain"))
    assert "READY" in plain.send(PROMPT).text
    assert plain.work_policy_digest() is None, "no policy: a Run binds none"
    assert plain.apply_goal_definition(_create(plain, None))["binding"]["work_policy_digest"] is None
    plain.close()

    policy = {"id": "eng", "revision": 1, "instructions": "Answer exactly what is asked and change nothing else."}
    governed = agent.session(tempfile.mkdtemp(), _options("python-goal-live-policy", policy))
    assert governed.work_policy_digest() is None, "configured but not yet admitted"
    result = governed.send(PROMPT)
    assert "READY" in result.text
    digest = governed.work_policy_digest()
    assert digest, "the first Run admits the policy"
    assert governed.goal_definition() is None, "policy without goal"
    try:
        governed.apply_goal_definition(_create(governed, None))
    except Exception as error:  # noqa: BLE001 - Core's refusal crosses as the SDK error
        assert "goal." in str(error), error
    else:
        raise AssertionError("a goal without the admitted policy digest must be refused")
    bound = governed.apply_goal_definition(_create(governed, digest))
    assert bound["binding"]["work_policy_digest"] == digest
    governed.close()
    print(json.dumps({
        "sdk": "python", "digest": digest, "tokens": result.total_tokens,
        "ms": int((time.monotonic() - started) * 1000),
    }))


if __name__ == "__main__":
    main()
