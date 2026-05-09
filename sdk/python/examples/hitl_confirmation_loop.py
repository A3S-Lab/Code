#!/usr/bin/env python3
"""Minimal streaming HITL confirmation loop.

Usage:
    A3S_CONFIG_FILE=./agent.acl python examples/hitl_confirmation_loop.py
    A3S_CONFIG_FILE=./agent.acl A3S_CODE_HITL_AUTO_APPROVE=1 python examples/hitl_confirmation_loop.py
"""

from __future__ import annotations

import json
import os
import sys
from typing import Any

from a3s_code import Agent, ConfirmationPolicy, PermissionPolicy, SessionOptions


def event_data(event: Any) -> dict[str, Any]:
    if not event.data:
        return {}
    try:
        value = json.loads(event.data)
        return value if isinstance(value, dict) else {}
    except json.JSONDecodeError:
        return {}


def pending_by_tool_id(session: Any, tool_id: str | None) -> dict[str, Any] | None:
    pending = session.pending_confirmations()
    for item in pending:
        if item.get("tool_id") == tool_id:
            return item
    return pending[0] if pending else None


def confirm_from_cli(session: Any, event: Any) -> None:
    pending = pending_by_tool_id(session, event.tool_id)
    data = event_data(event)
    tool_id = (pending or {}).get("tool_id") or event.tool_id

    if not tool_id:
        print("[hitl] confirmation_required event has no tool id; skipping", file=sys.stderr)
        return

    tool_name = (pending or {}).get("tool_name") or event.tool_name or "unknown"
    args = (pending or {}).get("args") or data.get("args") or {}
    remaining_ms = (pending or {}).get("remaining_ms") or data.get("timeout_ms")

    print("\n[hitl] Tool requires confirmation")
    print(f"tool: {tool_name}")
    print(f"tool_id: {tool_id}")
    if remaining_ms is not None:
        print(f"remaining_ms: {remaining_ms}")
    print(json.dumps(args, indent=2, ensure_ascii=False))

    auto_approve = os.environ.get("A3S_CODE_HITL_AUTO_APPROVE") == "1"
    answer = "y" if auto_approve else input("Approve this tool call? [y/N] ")
    approved = answer.strip().lower().startswith("y")
    completed = session.confirm_tool_use(
        tool_id,
        approved,
        "Approved from Python HITL example" if approved else "Rejected from Python HITL example",
    )
    state = "approved" if approved else "rejected"
    print(f"[hitl] confirmation {'resolved' if completed else 'not found'} ({state})")


def main() -> None:
    config_path = os.environ.get("A3S_CONFIG_FILE") or (sys.argv[1] if len(sys.argv) > 1 else None)
    if not config_path:
        raise RuntimeError("Set A3S_CONFIG_FILE or pass an agent.acl path as the first argument.")

    workspace = os.environ.get("A3S_CODE_WORKSPACE") or os.getcwd()
    prompt = os.environ.get("A3S_CODE_HITL_PROMPT") or (
        "Use the bash tool to run: echo A3S_CODE_HITL_OK. Then summarize the result."
    )

    opts = SessionOptions()
    opts.planning_mode = "disabled"
    opts.permission_policy = PermissionPolicy(ask=["bash*"], default_decision="allow")
    opts.confirmation_policy = ConfirmationPolicy(
        enabled=True,
        default_timeout_ms=120_000,
        timeout_action="reject",
        yolo_lanes=["query"],
    )

    agent = Agent.create(config_path)
    session = agent.session(workspace, opts)

    try:
        for event in session.stream(prompt):
            if event.event_type == "text_delta" and event.text:
                print(event.text, end="", flush=True)
            elif event.event_type == "tool_start":
                print(f"\n[tool:start] {event.tool_name or 'unknown'}")
            elif event.event_type == "tool_end":
                print(f"\n[tool:end] {event.tool_name or 'unknown'} exit={event.exit_code or 0}")
            elif event.event_type == "confirmation_required":
                confirm_from_cli(session, event)
            elif event.event_type == "confirmation_received":
                print(f"\n[hitl] confirmation_received {event.tool_id or ''}")
            elif event.event_type == "confirmation_timeout":
                print(f"\n[hitl] confirmation_timeout {event.tool_id or ''}")
            elif event.event_type == "error":
                raise RuntimeError(event.error or "stream error")

        print("\n[hitl] stream complete")
    finally:
        session.cancel_confirmations()


if __name__ == "__main__":
    main()
