"""Durable goal and work-policy surface: Core parses every command and
refuses identically to the Rust, Node and Go hosts (no credentials needed)."""

from __future__ import annotations

import asyncio
import json
import tempfile
import time
from pathlib import Path

import pytest

from a3s_code import Agent, FileSessionStore, SessionOptions

INLINE_CONFIG = """
default_model = "anthropic/test-model"
providers "anthropic" {
  api_key = "test-key"
  models "test-model" { name = "Test Model" }
}
""".strip()


def _ref(kind: str, operation_id: str, view: dict) -> dict:
    binding = view["binding"]
    return {
        "kind": kind,
        "operation_id": operation_id,
        "goal_id": binding["goal_id"],
        "session_id": binding["session_id"],
        "workspace": binding["workspace"],
        "work_policy_digest": None,
        "expected_goal_revision": binding["goal_revision"],
        "expected_plan_revision": view["plan_revision"],
        "expected_goal_sequence": view["goal_sequence"],
    }


def test_work_policy_option_is_validated_by_core() -> None:
    options = SessionOptions()
    with pytest.raises(ValueError, match="work_policy"):
        options.work_policy = {"id": "", "revision": 1, "instructions": "x"}
    with pytest.raises(ValueError):
        options.work_policy = {"id": "eng", "revision": 1, "instructions": "x", "extra": 1}
    agent = Agent.create(INLINE_CONFIG)
    for policy in (None, {"id": "eng", "revision": 1, "instructions": "Make the smallest complete change."}):
        options = SessionOptions()
        options.work_policy = policy
        assert options.work_policy == policy
        session = agent.session(tempfile.mkdtemp(), options)
        digest = session.work_policy_digest()
        if policy is None:
            assert digest is None
        else:
            assert isinstance(digest, str) and digest, "policy is admitted at session build"
        assert session.work_policy_digest() == digest
        assert session.current_run() is None, "policy admission starts no Run"
        session.close()


def test_goal_commands_round_trip_through_core() -> None:
    agent = Agent.create(INLINE_CONFIG)
    options = SessionOptions()
    options.session_id = "python-goal"
    session = agent.session(tempfile.mkdtemp(), options)
    workspace = {"id": session.workspace, "display_root": session.workspace}
    assert session.goal_definition() is None
    create = {
        "kind": "create", "operation_id": "create", "goal_id": "release",
        "session_id": session.session_id, "workspace": workspace,
        "goal": {
            "description": "Report the release code", "success_criteria": ["Reported exactly"],
            "progress": 0.0, "achieved": False, "created_at": int(time.time()), "achieved_at": None,
        },
        "non_goals": ["Do not change files"], "scope_fact_sequences": [], "work_policy_digest": None,
        "expected_goal_revision": None, "expected_plan_revision": None, "expected_goal_sequence": None,
    }
    with pytest.raises(Exception, match="unknown field"):
        session.apply_goal_definition({**create, "unexpected": True})
    assert session.goal_definition() is None, "a refused command records nothing"

    created = session.apply_goal_definition(create)
    assert created["state"] == "active"
    assert session.goal_definition() == created
    proposed = session.apply_goal_plan({
        **_ref("propose_plan", "propose", created), "proposal_sequence": None,
        "plan": {
            "goal": "Release check", "complexity": "Simple", "required_tools": [], "estimated_steps": 1,
            "steps": [{
                "id": "report", "content": "Report the release code", "status": "pending",
                "priority": "medium", "success_criteria": "Reported exactly",
            }],
        },
    })
    accepted = session.apply_goal_plan({
        **_ref("accept_plan", "accept", proposed), "plan": None,
        "proposal_sequence": proposed["plan_fact_sequence"],
    })
    assert accepted["plan_accepted"] is True

    # Without trusted host goal_control bounds a ready step never advances.
    tick = _ref("tick", "tick-1", accepted)
    assert session.preview_goal_tick(tick) == "budget_exhausted"
    assert session.tick_goal(tick) == {"decision": "budget_exhausted"}
    assert session.stream_goal_tick(tick) == {"decision": "budget_exhausted"}

    assert session.apply_goal_control(_ref("pause", "pause", accepted))["state"] == "paused"
    with pytest.raises(Exception, match=r"goal\."):
        session.apply_goal_control(_ref("resume", "resume", accepted))
    assert session.reconcile_goal_slices() == 0
    session.close()


def test_fork_session_from_saved_snapshot() -> None:
    agent = Agent.create(INLINE_CONFIG)
    store = tempfile.mkdtemp()
    options = SessionOptions()
    options.session_id = "python-fork-src"
    options.session_store = FileSessionStore(store)
    source = agent.session(tempfile.mkdtemp(), options)
    source.save()
    [snapshot_file] = (Path(store) / "v1" / "sessions").glob("*.json")
    snapshot = json.loads(snapshot_file.read_text())

    async def fork():
        fork_options = SessionOptions()
        fork_options.session_id = "python-fork"
        forked = await agent.fork_session_async(snapshot, tempfile.mkdtemp(), fork_options)
        assert forked.session_id == "python-fork"
        assert forked.goal_definition() is None, "source Goal facts stay at the source"
        with pytest.raises(Exception, match="schema_version"):
            await agent.fork_session_async({}, tempfile.mkdtemp(), SessionOptions())
        forked.close()

    asyncio.run(fork())
    source.close()
