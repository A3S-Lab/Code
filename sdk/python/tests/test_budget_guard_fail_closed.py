"""Budget callbacks must never disable their own enforcement on failure."""

from __future__ import annotations

import time
from pathlib import Path

from a3s_code import Agent, LocalWorkspaceBackend, PermissionPolicy, SessionOptions


INLINE_CONFIG = """
default_model = "anthropic/claude-sonnet-4-20250514"

providers "anthropic" {
  api_key = "test-key"
  models "claude-sonnet-4-20250514" {
    name = "Claude Sonnet 4"
  }
}
""".strip()


class RaisingToolGuard:
    def check_before_tool(self, session_id: str, tool_name: str) -> dict:
        raise RuntimeError("policy backend unavailable")


class MalformedToolGuard:
    def check_before_tool(self, session_id: str, tool_name: str) -> str:
        return "allow"


class SlowToolGuard:
    def check_before_tool(self, session_id: str, tool_name: str) -> dict:
        time.sleep(0.5)
        return {"decision": "allow"}


def _session(workspace: Path, guard: object, *, timeout_ms: int = 5_000):
    agent = Agent.create(INLINE_CONFIG)
    options = SessionOptions()
    options.permission_policy = PermissionPolicy(default_decision="allow")
    options.workspace_backend = LocalWorkspaceBackend(str(workspace))
    options.budget_guard = guard
    if timeout_ms != 5_000:
        options.budget_guard_timeout_ms = timeout_ms
    return agent, agent.session(str(workspace), options)


def _assert_write_denied_without_side_effect(
    workspace: Path, guard: object, *, timeout_ms: int = 5_000
) -> None:
    agent, session = _session(workspace, guard, timeout_ms=timeout_ms)
    target = workspace / "must-not-exist.txt"
    try:
        result = session.governed_tool(
            "write",
            {"file_path": target.name, "content": "budget guard must authorize this"},
        )
        assert result.exit_code != 0, result.output
        assert "budget" in result.output.lower(), result.output
        assert not target.exists(), "a failed budget callback must not permit a write"
    finally:
        session.close()
        agent.close()


def test_budget_guard_exception_fails_closed(tmp_path: Path) -> None:
    _assert_write_denied_without_side_effect(tmp_path, RaisingToolGuard())


def test_budget_guard_malformed_return_fails_closed(tmp_path: Path) -> None:
    _assert_write_denied_without_side_effect(tmp_path, MalformedToolGuard())


def test_budget_guard_timeout_is_bounded_and_fails_closed(tmp_path: Path) -> None:
    started = time.monotonic()
    _assert_write_denied_without_side_effect(tmp_path, SlowToolGuard(), timeout_ms=25)
    elapsed = time.monotonic() - started
    assert elapsed < 0.25, f"budget timeout was not bounded: {elapsed:.3f}s"
