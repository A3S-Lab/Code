"""Serve layer (filesystem-first agents) — Python SDK tests.

Verifies the PyO3 serve binding added alongside the Rust serve daemon:
- `agent.serve_agent_dir(dir, workspace[, options]) -> ServeHandle`
- `handle.is_ready()` / `handle.state()` / `handle.failure_code()`
- `handle.stop()` / `handle.is_stopped()`

Unit (hermetic, inline ACL, no provider credentials): the ServeHandle lifecycle.
Integration (real provider, skipped without `.a3s/config.acl`): a real cron
schedule firing full harness turns through the serve daemon without aborting the
FFI boundary, then a clean stop.

Run with: python sdk/python/tests/test_serve.py
"""

from __future__ import annotations

import os
import pathlib
import tempfile
import time

from a3s_code import Agent


INLINE_CONFIG = """
default_model = "anthropic/claude-sonnet-4-20250514"

providers "anthropic" {
  api_key = "test-key"
  models "claude-sonnet-4-20250514" {
    name = "Claude Sonnet 4"
  }
}
""".strip()


def _write_agent_dir(
    *, with_schedule: bool, with_tools: bool = False, invalid_cron: bool = False
) -> str:
    base = tempfile.mkdtemp(prefix="a3s-code-serve-")
    pathlib.Path(base, "instructions.md").write_text(
        "You are a terse test agent. Answer in one word."
    )
    if with_schedule:
        sched = pathlib.Path(base, "schedules")
        sched.mkdir()
        cron = "not a cron" if invalid_cron else "* * * * * *"
        (sched / "tick.md").write_text(
            f'---\ncron: "{cron}"\nname: tick\n---\nReply with exactly one word: PONG'
        )
    if with_tools:
        tools = pathlib.Path(base, "tools")
        tools.mkdir()
        (tools / "echo.md").write_text(
            "---\nkind: script\nname: echo-tool\npath: scripts/echo.js\n---\nEcho tool.\n"
        )
    return base


def test_serve_with_script_tool() -> None:
    """Unit (hermetic): serving a dir that contains a `tools/` `kind: script`
    spec succeeds. serve_agent_dir runs AgentDir::load synchronously (parsing
    tools/), so a malformed tool spec would raise here; a valid one yields a
    healthy handle that stops cleanly. No provider call is made."""
    agent = Agent.create(INLINE_CONFIG)
    agent_dir = _write_agent_dir(with_schedule=False, with_tools=True)
    workspace = tempfile.mkdtemp(prefix="a3s-code-serve-tools-ws-")

    handle = agent.serve_agent_dir(agent_dir, workspace)
    assert handle.is_stopped() is False, "a tools/ kind:script agent dir should serve"
    handle.stop()
    assert handle.is_stopped() is True
    print("python sdk serve with kind:script tool ok")


def test_serve_handle_lifecycle() -> None:
    """Unit (hermetic): serving a dir with no schedules returns a ServeHandle
    that reports not-stopped, and stop() is idempotent and reflected by
    is_stopped(). No provider call is made."""
    agent = Agent.create(INLINE_CONFIG)
    agent_dir = _write_agent_dir(with_schedule=False)
    workspace = tempfile.mkdtemp(prefix="a3s-code-serve-ws-")

    handle = agent.serve_agent_dir(agent_dir, workspace)
    assert handle.is_ready() is True
    assert handle.state() == "ready"
    assert handle.failure_code() is None
    assert handle.is_stopped() is False, "handle should not be stopped before stop()"

    handle.stop()
    assert handle.is_stopped() is True, "stop() must set is_stopped() to True"
    assert handle.is_ready() is False
    assert handle.state() == "stopped"
    handle.stop()  # idempotent — must not raise
    assert handle.is_stopped() is True

    print("python sdk serve handle lifecycle ok")


def test_serve_startup_failure() -> None:
    """Invalid schedules fail before a ready handle is returned."""
    agent = Agent.create(INLINE_CONFIG)
    agent_dir = _write_agent_dir(with_schedule=True, invalid_cron=True)
    workspace = tempfile.mkdtemp(prefix="a3s-code-serve-invalid-ws-")

    try:
        agent.serve_agent_dir(agent_dir, workspace)
    except RuntimeError as error:
        assert getattr(error, "code", None) == "SERVE_STARTUP_FAILED"
    else:
        raise AssertionError("invalid schedule must fail before activation")


def _repo_config() -> str | None:
    env = os.environ.get("A3S_CONFIG_FILE")
    if env and os.path.isfile(env):
        return env
    here = pathlib.Path(__file__).resolve()
    for parent in here.parents:
        cand = parent / ".a3s" / "config.acl"
        if cand.is_file():
            return str(cand)
    return None


def test_serve_real_schedule() -> None:
    """Integration (real provider): serve a dir with an every-second schedule so
    real harness turns fire through the daemon; the FFI boundary must survive the
    real turns (a PyO3 panic would abort the process) and stop() must shut it
    down. Skipped when no `.a3s/config.acl` is available."""
    config = _repo_config()
    if config is None:
        print("python sdk serve real-schedule SKIPPED (no .a3s/config.acl)")
        return

    agent = Agent.create(config)
    agent_dir = _write_agent_dir(with_schedule=True)
    workspace = tempfile.mkdtemp(prefix="a3s-code-serve-real-ws-")

    handle = agent.serve_agent_dir(agent_dir, workspace)
    assert handle.is_stopped() is False
    # Let the every-second schedule fire at least one real harness turn. The
    # process surviving this window is the FFI integration assertion.
    time.sleep(8)
    handle.stop()
    assert handle.is_stopped() is True
    print("python sdk serve real-schedule ok (daemon fired real turns, stopped clean)")


def main() -> None:
    test_serve_handle_lifecycle()
    test_serve_startup_failure()
    test_serve_with_script_tool()
    test_serve_real_schedule()


if __name__ == "__main__":
    main()
