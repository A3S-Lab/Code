"""Smoke test for the Agent / Session close surface exposed by the
core in steps 1–3 and propagated through the Python SDK in step 4.

Verifies the PyO3 wrappers correctly route to core:
- `session.is_closed` getter
- `session.close()` is idempotent
- `agent.list_sessions()` reflects live sessions
- `agent.close_session(id)` closes one session by ID
- `agent.close()` closes every live session and rejects further
  `agent.session(...)` calls

Run with: python -m sdk/python/tests/test_session_close
(no provider credentials needed — uses inline ACL).
"""

from __future__ import annotations

import tempfile
from typing import cast

from a3s_code import (
    Agent,
    LocalWorkspaceBackend,
    MemoryMaintenanceHealth,
    PermissionPolicy,
    SessionOptions,
)


INLINE_CONFIG = """
default_model = "anthropic/claude-sonnet-4-20250514"

providers "anthropic" {
  api_key = "test-key"
  models "claude-sonnet-4-20250514" {
    name = "Claude Sonnet 4"
  }
}
""".strip()


def _make_session(agent: Agent, workspace: str, session_id: str):
    opts = SessionOptions()
    opts.permission_policy = PermissionPolicy(default_decision="allow")
    opts.workspace_backend = LocalWorkspaceBackend(workspace)
    opts.session_id = session_id
    return agent.session(workspace, opts)


def main() -> None:
    workspace = tempfile.mkdtemp(prefix="a3s-code-python-close-")
    agent = Agent.create(INLINE_CONFIG)

    # 1. Fresh session: is_closed is False, list_sessions sees it.
    session = _make_session(agent, workspace, "py-close-1")
    assert session.is_closed is False, "fresh session should not be closed"
    health = cast(MemoryMaintenanceHealth, session.memory_maintenance_health())
    assert health == {"phase": "disabled", "jobs": []}

    listed = agent.list_sessions()
    assert "py-close-1" in listed, (
        f"agent.list_sessions() should include py-close-1, got {listed!r}"
    )

    # 2. session.close() flips is_closed and is idempotent.
    session.close()
    assert session.is_closed is True, "session.close() must set is_closed = True"
    session.close()  # second close must not raise
    assert session.is_closed is True

    # 3. agent.close_session(id) on a *new* live session closes it.
    session_b = _make_session(agent, workspace, "py-close-2")
    assert session_b.is_closed is False
    was_open = agent.close_session("py-close-2")
    assert was_open is True, (
        f"close_session() on a live session must return True, got {was_open!r}"
    )
    assert session_b.is_closed is True, (
        "close_session() must propagate to the Python wrapper's is_closed view"
    )

    # 4. close_session() on an unknown id returns False, doesn't raise.
    unknown = agent.close_session("does-not-exist")
    assert unknown is False, (
        f"close_session() on unknown id must return False, got {unknown!r}"
    )

    # 5. agent.close() closes every live session and rejects new session().
    session_c = _make_session(agent, workspace, "py-close-3")
    session_d = _make_session(agent, workspace, "py-close-4")
    assert session_c.is_closed is False
    assert session_d.is_closed is False

    agent.close()
    assert agent.is_closed is True, "agent.is_closed must be True after agent.close()"
    assert session_c.is_closed is True, "agent.close() must close session_c"
    assert session_d.is_closed is True, "agent.close() must close session_d"

    # New session() must raise.
    try:
        _ = _make_session(agent, workspace, "py-close-post")
    except Exception as exc:
        msg = str(exc).lower()
        assert "closed" in msg, (
            f"post-close session() error must mention 'closed', got: {exc!r}"
        )
    else:
        raise AssertionError("session() after agent.close() must raise")

    # disconnect_idle_mcp is exposed and returns a list (empty here — the
    # inline config registers no MCP servers). Use a fresh agent since the
    # one above is closed.
    agent2 = Agent.create(INLINE_CONFIG)
    dropped = agent2.disconnect_idle_mcp(5 * 60 * 1000)
    assert isinstance(dropped, list), f"disconnect_idle_mcp must return a list, got {type(dropped)!r}"
    assert dropped == [], f"no MCP servers configured -> nothing dropped, got {dropped!r}"

    print("python sdk session close api ok")


if __name__ == "__main__":
    main()
