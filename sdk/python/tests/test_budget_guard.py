"""Smoke test for the Python BudgetGuard wrapper.

Verifies that a Python BudgetGuard whose `check_before_llm` returns
``{"decision": "deny", ...}`` aborts ``session.send`` with a
RuntimeError that mentions "Budget exhausted" — the framework's
canonical denial signal — without the LLM ever being touched.

Run with:
    PYTHONPATH=python python tests/test_budget_guard.py
"""

from __future__ import annotations

import tempfile

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


class DenyingGuard:
    """BudgetGuard that always denies the first LLM call and records
    everything the framework hands it for post-hoc assertions."""

    def __init__(self) -> None:
        self.llm_checks: list[tuple[str, int]] = []
        self.tool_checks: list[tuple[str, str]] = []
        self.llm_records: list[tuple[str, dict]] = []

    def check_before_llm(self, session_id: str, estimated_tokens: int) -> dict:
        self.llm_checks.append((session_id, estimated_tokens))
        return {
            "decision": "deny",
            "resource": "llm_tokens",
            "reason": "test cap exceeded",
        }

    def check_before_tool(self, session_id: str, tool_name: str) -> dict | None:
        self.tool_checks.append((session_id, tool_name))
        return None  # allow

    def record_after_llm(self, session_id: str, usage: dict) -> None:
        self.llm_records.append((session_id, usage))


class AllowingGuard:
    """BudgetGuard that returns None / no-op for every method. Verifies
    that "shape with no real methods" still works (the wrapper looks
    up by name at call time)."""

    def check_before_llm(self, session_id: str, estimated_tokens: int):
        return None

    def record_after_llm(self, session_id: str, usage: dict) -> None:
        pass


def main() -> None:
    workspace = tempfile.mkdtemp(prefix="a3s-budget-")
    agent = Agent.create(INLINE_CONFIG)

    # ----- Phase A: Deny -----
    guard = DenyingGuard()
    opts = SessionOptions()
    opts.permission_policy = PermissionPolicy(default_decision="allow")
    opts.workspace_backend = LocalWorkspaceBackend(workspace)
    opts.session_id = "budget-deny-test"
    opts.budget_guard = guard

    # Also exercise the getter — read-back must match what we wrote.
    assert opts.budget_guard is guard, "BudgetGuard getter must round-trip"

    session = agent.session(workspace, opts)

    try:
        _ = session.send("hello")
    except RuntimeError as exc:
        msg = str(exc)
        assert (
            "Budget exhausted" in msg or "llm_tokens" in msg
        ), f"expected budget-exhausted error, got: {exc!r}"
    else:
        raise AssertionError("send() must raise when BudgetGuard denies")

    assert len(guard.llm_checks) == 1, (
        f"check_before_llm must be consulted exactly once, got {guard.llm_checks!r}"
    )
    assert guard.llm_checks[0][0] == "budget-deny-test", (
        f"session_id must propagate, got {guard.llm_checks[0]!r}"
    )
    assert len(guard.llm_records) == 0, (
        f"record_after_llm must not fire when call was denied, got {guard.llm_records!r}"
    )

    # ----- Phase B: Allow / no-op shape -----
    # A guard with only allow-style methods must not break send().
    # We can't actually send without provider credentials, so we just
    # verify the SessionOptions roundtrip and that constructing a
    # session succeeds.
    allow_opts = SessionOptions()
    allow_opts.permission_policy = PermissionPolicy(default_decision="allow")
    allow_opts.workspace_backend = LocalWorkspaceBackend(workspace)
    allow_opts.session_id = "budget-allow-test"
    allow_opts.budget_guard = AllowingGuard()
    _ = agent.session(workspace, allow_opts)

    # ----- Phase C: clear back to None -----
    opts.budget_guard = None
    assert opts.budget_guard is None

    print("python sdk budget guard ok")


if __name__ == "__main__":
    main()
