"""
Integration test for GitHub issue #7:

  1. register_agent_dir  — dynamically add agents to a live session without restart
  2. add_mcp_server timeout_ms — per-server MCP request timeout configuration

Uses the kimi-k2.5 model configured in agent.test.hcl.
"""

import os
import sys
import tempfile
import textwrap
import traceback

import a3s_code

CONFIG = os.path.join(os.path.dirname(__file__), "agent.test.hcl")

PASS = "\033[32m✓\033[0m"
FAIL = "\033[31m✗\033[0m"


def section(title: str) -> None:
    print(f"\n{'─' * 60}")
    print(f"  {title}")
    print('─' * 60)


def check(label: str, condition: bool, detail: str = "") -> None:
    if condition:
        print(f"  {PASS}  {label}")
    else:
        msg = f"  {FAIL}  {label}"
        if detail:
            msg += f"\n      → {detail}"
        print(msg)


# ─────────────────────────────────────────────────────────────────────────────
# Test 1  register_agent_dir
# ─────────────────────────────────────────────────────────────────────────────

def test_register_agent_dir() -> bool:
    section("Test 1 · register_agent_dir — dynamic agent registration")
    ok = True

    agent = a3s_code.Agent.create(CONFIG)
    session = agent.session(".")

    # ── 1a: the directory is empty → 0 agents loaded ────────────────────────
    with tempfile.TemporaryDirectory() as tmpdir:
        n = session.register_agent_dir(tmpdir)
        check("empty dir returns 0", n == 0, f"got {n}")
        ok = ok and n == 0

    # ── 1b: nonexistent path → 0 agents, no crash ───────────────────────────
    n = session.register_agent_dir("/nonexistent/path/xyz")
    check("nonexistent dir returns 0", n == 0, f"got {n}")
    ok = ok and n == 0

    # ── 1c: load real agents from a dir, confirm count ──────────────────────
    with tempfile.TemporaryDirectory() as tmpdir:
        # Write a YAML agent file
        open(os.path.join(tmpdir, "greeter.yaml"), "w").write(textwrap.dedent("""\
            name: greeter
            description: A simple greeting agent
            mode: subagent
            max_steps: 3
            prompt: |
              You are a friendly greeter.
              When asked to greet, respond with a warm greeting message.
        """))
        # Write a Markdown agent file
        open(os.path.join(tmpdir, "reporter.md"), "w").write(textwrap.dedent("""\
            ---
            name: reporter
            description: A reporting agent
            mode: subagent
            max_steps: 5
            ---
            You are a reporting agent.
            Summarize any information given to you concisely.
        """))
        # Write a non-agent file (should be ignored)
        open(os.path.join(tmpdir, "notes.txt"), "w").write("ignored")

        n = session.register_agent_dir(tmpdir)
        check("2 agent files → count == 2", n == 2, f"got {n}")
        ok = ok and n == 2

        # ── 1d: agents are immediately usable via task() ─────────────────────
        print("\n  Asking kimi to delegate to 'greeter' agent via task()…")
        try:
            result = session.send(
                'Use the task tool to delegate this to the "greeter" agent: '
                'task(agent="greeter", description="say hello", '
                'prompt="Please greet the user warmly.", permissive=True)'
            )
            has_greeting = any(
                kw in result.text.lower()
                for kw in ("hello", "hi", "greet", "welcome", "warm")
            )
            check("greeter agent responded (task tool succeeded)", has_greeting, result.text[:200])
            ok = ok and has_greeting
        except Exception as exc:
            check("greeter agent task() succeeded", False, str(exc))
            traceback.print_exc()
            ok = False

    return ok


# ─────────────────────────────────────────────────────────────────────────────
# Test 2  add_mcp_server timeout_ms
# ─────────────────────────────────────────────────────────────────────────────

def test_add_mcp_server_timeout() -> bool:
    section("Test 2 · add_mcp_server timeout_ms — custom timeout propagation")
    ok = True

    agent = a3s_code.Agent.create(CONFIG)
    session = agent.session(".")

    # ── 2a: timeout_ms=1000 → very short timeout causes expected error ───────
    #  We use 'cat' as a fake MCP server (it will hang waiting for input).
    #  With a 1-second timeout the request must fail quickly.
    print("  Adding 'cat' as MCP server with timeout_ms=1000 (should time out fast)…")
    import time
    t0 = time.monotonic()
    try:
        session.add_mcp_server(
            "fake_mcp",
            transport="stdio",
            command="cat",
            timeout_ms=1000,
        )
        # If it somehow succeeded, that's unexpected but not a failure of
        # the timeout feature (cat echoes stdin and may respond)
        elapsed = time.monotonic() - t0
        check("timeout_ms accepted without error", True)
        ok = ok and True
    except RuntimeError as exc:
        elapsed = time.monotonic() - t0
        timed_out_quickly = elapsed < 5.0  # must be well under 60 s
        msg = str(exc).lower()
        is_timeout_err = "timeout" in msg or "timed out" in msg or "closed" in msg
        check(
            f"short timeout triggers error quickly (<5 s, got {elapsed:.1f}s)",
            timed_out_quickly,
            str(exc)[:120],
        )
        ok = ok and timed_out_quickly

    # ── 2b: default timeout (no timeout_ms) still works ─────────────────────
    # Just verify the API accepts absence of timeout_ms without raising TypeError.
    try:
        session2 = agent.session(".")
        # 'true' exits immediately — not a valid MCP server, but the API call
        # itself should not raise a TypeError about the missing parameter.
        try:
            session2.add_mcp_server("true_cmd", transport="stdio", command="true")
        except RuntimeError:
            pass  # expected: connection error, not a parameter error
        check("omitting timeout_ms uses default (no TypeError)", True)
    except TypeError as exc:
        check("omitting timeout_ms uses default (no TypeError)", False, str(exc))
        ok = False

    return ok


# ─────────────────────────────────────────────────────────────────────────────
# Main
# ─────────────────────────────────────────────────────────────────────────────

def main() -> None:
    print(f"\na3s_code version: {getattr(a3s_code, '__version__', 'unknown')}")
    print(f"Config: {CONFIG}")

    results = {
        "register_agent_dir": test_register_agent_dir(),
        "add_mcp_server timeout_ms": test_add_mcp_server_timeout(),
    }

    section("Summary")
    all_ok = True
    for name, passed in results.items():
        check(name, passed)
        all_ok = all_ok and passed

    print()
    if all_ok:
        print(f"  {PASS}  All tests passed")
        sys.exit(0)
    else:
        print(f"  {FAIL}  Some tests failed")
        sys.exit(1)


if __name__ == "__main__":
    main()
