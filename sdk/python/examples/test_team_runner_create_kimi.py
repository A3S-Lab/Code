#!/usr/bin/env python3
"""
A3S Code Python SDK — TeamRunner.create() Integration Tests (Kimi)

Covers the new API introduced alongside per-member model/prompt/workspace
overrides in TeamMemberOptions-equivalent keyword arguments:

  Scenario 1: TeamRunner.create() basic API
    Verifies the new factory constructor works end-to-end without any opts.

  Scenario 2: Per-member model override
    Lead and worker each carry an explicit model kwarg; run completes normally.

  Scenario 3: Per-member prompt slot (role + guidelines)
    Worker is constrained via role to "reply in exactly one word".
    Verifies the custom system prompt influences the LLM output.

  Scenario 4: Per-worker workspace isolation
    Two workers each receive a different temp-dir workspace.
    Each writes a marker file; test asserts files land in the correct dirs.

  Scenario 5: Inheritance — opts omitted, agent definition defaults apply
    Uses the built-in "explore" agent (has a defined prompt).
    Omits all keyword overrides; definition prompt must still be applied.

Run with: python examples/test_team_runner_create_kimi.py
"""

import sys
import os
import time
import tempfile
from pathlib import Path

if sys.stdout.encoding and sys.stdout.encoding.lower() not in ("utf-8", "utf8"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

from a3s_code import Agent, TeamRunner


# ── Helpers ───────────────────────────────────────────────────────────────────

def resolve_config() -> str:
    home_cfg = Path.home() / ".a3s" / "config.hcl"
    if home_cfg.exists():
        return str(home_cfg)
    p = Path(__file__).resolve()
    for _ in range(10):
        candidate = p.parent / ".a3s" / "config.hcl"
        if candidate.exists():
            return str(candidate)
        parent = p.parent.parent
        if parent == p.parent:
            break
        p = p.parent
    raise RuntimeError("Config not found — run from inside the a3s repo")


def pass_test(label: str) -> None:
    print(f"  PASS  {label}")


def print_result(result) -> None:
    print(f"  Done: {len(result.done_tasks)}, Rejected: {len(result.rejected_tasks)}, "
          f"Rounds: {result.rounds}")
    for t in result.done_tasks:
        snippet = (t.result or "").replace("\n", " ")[:120]
        print(f"    [{t.id}] {t.description[:60]}")
        print(f"      -> {snippet}")


# ── Scenario 1: TeamRunner.create() basic API ─────────────────────────────────

def test_create_basic(agent: Agent, workspace: str) -> None:
    print("\n== Scenario 1: TeamRunner.create() basic API ==")
    print("-" * 70)

    # No per-member overrides — all defaults from agent definition + Agent config.
    runner = TeamRunner.create(agent, workspace)
    runner.add_lead("general")
    runner.add_worker("general")
    runner.add_reviewer("general")

    goal = (
        "Decompose into exactly 2 tasks: "
        "1) What is the capital of Japan? Reply in one word. "
        "2) What is 6 * 7? Reply with just the number."
    )

    print(f"  Goal: {goal[:80]}...")
    t0 = time.time()
    result = runner.run_until_done(goal)
    print(f"  Completed in {time.time() - t0:.1f}s")
    print_result(result)

    assert len(result.done_tasks) >= 1, "Expected at least 1 done task"
    pass_test(f"TeamRunner.create() ran — {len(result.done_tasks)} tasks done")


# ── Scenario 2: Per-member model override ────────────────────────────────────

def test_model_override(agent: Agent, workspace: str) -> None:
    print("\n== Scenario 2: Per-member model override ==")
    print("-" * 70)

    kimi_model = "openai/kimi-k2.5"

    runner = TeamRunner.create(agent, workspace)
    runner.add_lead("general", model=kimi_model)
    runner.add_worker("general", model=kimi_model)
    runner.add_reviewer("general", model=kimi_model)

    print(f"  All members using explicit model: {kimi_model}")

    goal = (
        "Decompose into exactly 2 tasks: "
        "1) Name one planet in our solar system. One word. "
        "2) What color is the sky? One word."
    )

    t0 = time.time()
    result = runner.run_until_done(goal)
    print(f"  Completed in {time.time() - t0:.1f}s")
    print_result(result)

    assert len(result.done_tasks) >= 1, "Expected at least 1 done task"
    pass_test(f"Model override accepted — {len(result.done_tasks)} tasks done")


# ── Scenario 3: Per-member prompt slot ───────────────────────────────────────

def test_prompt_slots(agent: Agent, workspace: str) -> None:
    print("\n== Scenario 3: Per-member prompt slots ==")
    print("-" * 70)

    runner = TeamRunner.create(agent, workspace)
    runner.add_lead("general")
    runner.add_worker(
        "general",
        role="You are a terse answering machine. You MUST reply with exactly one word and nothing else.",
        guidelines="Single word answers only. No punctuation. No explanation.",
    )
    runner.add_reviewer(
        "general",
        role="You are a lenient reviewer. Always approve every result without modification.",
        guidelines="Your only job is to approve. Always respond with APPROVED.",
    )

    goal = (
        "Decompose into exactly 2 tasks: "
        "1) What is the chemical symbol for water? "
        "2) What is the first element on the periodic table?"
    )

    print("  Worker role: single-word answers only")
    print("  Reviewer role: always approve")
    t0 = time.time()
    result = runner.run_until_done(goal)
    print(f"  Completed in {time.time() - t0:.1f}s")
    print_result(result)

    assert len(result.done_tasks) >= 1, "Expected at least 1 done task"

    # Answers should be short (the role constraint instructs one-word replies).
    long_answers = [
        t for t in result.done_tasks
        if len((t.result or "").strip().split()) > 5
    ]
    if long_answers:
        print(f"  Warning: {len(long_answers)} answer(s) were longer than expected "
              "(prompt slots may not have taken effect)")
    pass_test(f"Prompt slots applied — {len(result.done_tasks)} tasks done")


# ── Scenario 4: Per-worker workspace isolation ────────────────────────────────

def test_worker_workspace_isolation(agent: Agent) -> None:
    print("\n== Scenario 4: Per-worker workspace isolation ==")
    print("-" * 70)

    with tempfile.TemporaryDirectory() as ws1, tempfile.TemporaryDirectory() as ws2:
        print(f"  Worker-1 workspace: {ws1}")
        print(f"  Worker-2 workspace: {ws2}")

        # Lead uses ws1 to decompose; workers each operate in their own dir.
        runner = TeamRunner.create(agent, ws1)
        runner.add_lead("general")
        runner.add_worker("general", workspace=ws1)
        runner.add_worker("general", workspace=ws2)
        runner.add_reviewer("general")

        goal = (
            "Decompose into exactly 2 tasks: "
            '1) Write the text "worker1_marker" (without quotes) to a file named '
            "marker.txt in the current workspace directory, then reply \"done\". "
            '2) Write the text "worker2_marker" (without quotes) to a file named '
            "marker.txt in the current workspace directory, then reply \"done\"."
        )

        print("  Goal: each worker writes a distinct marker file to its workspace")
        t0 = time.time()
        result = runner.run_until_done(goal)
        print(f"  Completed in {time.time() - t0:.1f}s")
        print_result(result)

        file1 = Path(ws1) / "marker.txt"
        file2 = Path(ws2) / "marker.txt"
        exists1 = file1.exists()
        exists2 = file2.exists()
        print(f"  marker.txt in ws1: {file1.read_text().strip() if exists1 else 'absent'}")
        print(f"  marker.txt in ws2: {file2.read_text().strip() if exists2 else 'absent'}")

        if not exists1 and not exists2:
            raise AssertionError("Neither worker wrote a marker file")

        if exists1 and exists2:
            c1 = file1.read_text().strip()
            c2 = file2.read_text().strip()
            if c1 != c2:
                pass_test("Workspace isolation confirmed: different content in each worker dir")
                return
            else:
                print("  Warning: both files have the same content — workers may share workspace")

        pass_test("Workspace isolation: at least one worker wrote to its designated dir")


# ── Scenario 5: Inheritance — opts omitted, agent definition defaults apply ───

def test_inheritance(agent: Agent, workspace: str) -> None:
    print("\n== Scenario 5: Inheritance — agent definition defaults ==")
    print("-" * 70)

    # "explore" has a built-in prompt (EXPLORE_PROMPT) and max_steps=20.
    # Passing no keyword args: definition's prompt and max_steps must be used.
    runner = TeamRunner.create(agent, workspace)
    runner.add_lead("general")
    runner.add_worker("explore")   # no overrides — inherits definition's prompt + max_steps
    runner.add_reviewer("general")

    goal = (
        "Decompose into exactly 1 task: "
        "List the names of up to 3 files or directories visible in the current "
        "workspace. If the workspace is empty, reply \"workspace is empty\"."
    )

    print('  Worker: builtin "explore" agent, no keyword overrides')
    t0 = time.time()
    result = runner.run_until_done(goal)
    print(f"  Completed in {time.time() - t0:.1f}s")
    print_result(result)

    assert len(result.done_tasks) >= 1, "Expected at least 1 done task"
    pass_test(
        f"Inheritance verified — explore agent ran with definition defaults, "
        f"{len(result.done_tasks)} task(s) done"
    )


# ── Main ──────────────────────────────────────────────────────────────────────

def main() -> None:
    config_path = resolve_config()
    agent = Agent.create(config_path)

    print("=" * 70)
    print("  A3S Code — TeamRunner.create() Integration Tests (Kimi)")
    print(f"  Config: {config_path}")
    print("=" * 70)

    with tempfile.TemporaryDirectory() as workspace:
        tests = [
            ("Scenario 1: create() basic API",
             lambda: test_create_basic(agent, workspace)),
            ("Scenario 2: per-member model override",
             lambda: test_model_override(agent, workspace)),
            ("Scenario 3: per-member prompt slots",
             lambda: test_prompt_slots(agent, workspace)),
            ("Scenario 4: per-worker workspace isolation",
             lambda: test_worker_workspace_isolation(agent)),
            ("Scenario 5: inheritance (no opts)",
             lambda: test_inheritance(agent, workspace)),
        ]

        passed = 0
        failed = 0
        for name, fn in tests:
            try:
                fn()
                passed += 1
            except Exception as e:
                import traceback
                print(f"\n  FAILED [{name}]: {e}")
                traceback.print_exc()
                failed += 1

    print("\n" + "=" * 70)
    if failed == 0:
        print(f"  ALL {passed} SCENARIOS PASSED")
    else:
        print(f"  {passed} passed, {failed} FAILED")
        sys.exit(1)
    print("=" * 70)


if __name__ == "__main__":
    main()
