#!/usr/bin/env python3
"""
Live verification for the sub-agent event-streaming fix.

This script validates the issue #18 behavior using a real Kimi-backed agent:
  1. Subscribe late via ``SubAgentHandle.events()``
  2. Confirm early events are replayed
  3. Confirm ``tool_execution_started.args`` is populated
  4. Confirm ``tool_execution_completed.duration_ms`` is > 0
  5. Confirm ``text_delta`` events are forwarded

It reads the Kimi endpoint and API key from the repo's ``.a3s/config.hcl`` and
injects them into the SDK example config via ``KIMI_API_KEY`` / ``KIMI_BASE_URL``.
"""

import json
import os
import re
import sys
import time
from pathlib import Path

from a3s_code import Agent, Orchestrator, SubAgentConfig


REPO_ROOT = Path(__file__).resolve().parents[5]
APP_CONFIG = REPO_ROOT / ".a3s" / "config.hcl"
SDK_CONFIG = Path(__file__).parent / "agent_kimi_k2.5.hcl"


def load_kimi_env_from_repo_config() -> None:
    raw = APP_CONFIG.read_text()
    api_key = re.search(
        r'"id"\s*=\s*"kimi-k2\.5"[\s\S]*?"apiKey"\s*=\s*"([^"]+)"', raw
    )
    base_url = re.search(
        r'"id"\s*=\s*"kimi-k2\.5"[\s\S]*?"baseUrl"\s*=\s*"([^"]+)"', raw
    )
    if not api_key or not base_url:
        raise RuntimeError(f"Failed to extract kimi-k2.5 config from {APP_CONFIG}")
    os.environ["KIMI_API_KEY"] = api_key.group(1)
    os.environ["KIMI_BASE_URL"] = base_url.group(1)


def main() -> int:
    print("\n=== Python SDK live sub-agent event-stream test ===\n")
    load_kimi_env_from_repo_config()

    agent = Agent.create(str(SDK_CONFIG))
    orchestrator = Orchestrator.create(agent=agent)
    handle = orchestrator.spawn_subagent(
        SubAgentConfig(
            agent_type="general",
            prompt="Use bash to run: printf 'hello-from-python-sdk'. Then briefly explain the result.",
            description="issue18-python-live-test",
            permissive=True,
            max_steps=5,
        )
    )

    # Subscribe late on purpose to verify history replay.
    time.sleep(2.0)
    events = handle.events()

    counts = {}
    text_deltas = []
    tool_starts = []
    tool_ends = []

    started_at = time.time()
    while time.time() - started_at < 60:
        event = events.recv(timeout_ms=2000)
        if event is None:
            continue

        event_type = event.get("event_type", "unknown")
        counts[event_type] = counts.get(event_type, 0) + 1

        if event_type == "sub_agent_internal_event" and event.get("type") == "text_delta":
            text_deltas.append(event.get("text", ""))
        elif event_type == "tool_execution_started":
            tool_starts.append(
                {"tool_name": event.get("tool_name"), "args": event.get("args")}
            )
        elif event_type == "tool_execution_completed":
            tool_ends.append(
                {
                    "tool_name": event.get("tool_name"),
                    "duration_ms": event.get("duration_ms"),
                    "result": event.get("result", "")[:120],
                }
            )
        elif event_type == "sub_agent_completed":
            break

    result = handle.wait()
    summary = {
        "counts": counts,
        "tool_starts": tool_starts,
        "tool_ends": tool_ends,
        "text_delta_chars": len("".join(text_deltas)),
        "result_preview": result[:200],
    }
    print(json.dumps(summary, ensure_ascii=False, indent=2))

    assert counts.get("sub_agent_started", 0) >= 1, "missing sub_agent_started replay"
    assert (
        counts.get("tool_execution_started", 0) >= 1
    ), "missing tool_execution_started"
    assert (
        counts.get("tool_execution_completed", 0) >= 1
    ), "missing tool_execution_completed"
    assert (
        counts.get("sub_agent_internal_event", 0) >= 1
    ), "missing sub_agent_internal_event"
    assert any(ts.get("args") not in (None, {}, "") for ts in tool_starts), "tool args were empty"
    assert any((te.get("duration_ms") or 0) > 0 for te in tool_ends), "tool duration_ms was not > 0"
    assert len("".join(text_deltas)) > 0, "missing text_delta events"

    print("\nPASS\n")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(f"\nFAIL: {exc}\n")
        raise
