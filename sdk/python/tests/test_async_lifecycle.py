"""Async lifecycle APIs must remain awaitable and preserve store identity."""

from __future__ import annotations

import asyncio
import tempfile
from pathlib import Path

import pytest

from a3s_code import Agent, MemorySessionStore, PermissionPolicy, SessionOptions


INLINE_CONFIG = """
default_model = "anthropic/claude-sonnet-4-20250514"

providers "anthropic" {
  api_key = "test-key"
  models "claude-sonnet-4-20250514" {
    name = "Claude Sonnet 4"
  }
}
""".strip()


def test_async_session_lifecycle_and_event_loop_progress() -> None:
    async def scenario() -> None:
        ticks = 0
        running = True

        async def heartbeat() -> None:
            nonlocal ticks
            while running:
                ticks += 1
                await asyncio.sleep(0)

        heartbeat_task = asyncio.create_task(heartbeat())
        agent = await Agent.create_async(INLINE_CONFIG)

        with tempfile.TemporaryDirectory(
            prefix="a3s-code-python-async-lifecycle-"
        ) as workspace:
            store = MemorySessionStore()
            create_options = SessionOptions()
            create_options.session_id = "python-async-lifecycle"
            create_options.session_store = store

            session = await agent.session_async(workspace, create_options)
            result = await session.send_async("/help")
            assert "/help" in result.text
            assert await session.runs_async() == []
            assert await session.run_snapshot_async("missing-run") is None
            assert await session.run_events_async("missing-run") == []
            assert await session.run_event_page_async("missing-run") is None
            tool_result = await session.tool_async(
                "write",
                {"file_path": "async-tool.txt", "content": "async tool output\n"},
            )
            assert tool_result.exit_code == 0
            assert Path(workspace, "async-tool.txt").read_text() == "async tool output\n"

            governed_options = SessionOptions()
            governed_options.permission_policy = PermissionPolicy(
                deny=["write"], default_decision="allow"
            )
            governed_session = await agent.session_async(workspace, governed_options)
            trusted_result = await governed_session.tool_async(
                "write",
                {
                    "file_path": "trusted-host-write.txt",
                    "content": "trusted\n",
                },
            )
            assert trusted_result.exit_code == 0
            governed_result = await governed_session.governed_tool_async(
                "write",
                {
                    "file_path": "denied-governed-write.txt",
                    "content": "must not exist\n",
                },
            )
            assert governed_result.exit_code != 0
            assert not Path(workspace, "denied-governed-write.txt").exists()
            await governed_session.close_async()

            assert await session.cancel_async() is False
            await session.save_async()

            replacement_options = SessionOptions()
            replacement_options.session_store = store
            replacement = await agent.replace_session_async(
                session, replacement_options
            )
            assert replacement.session_id == "python-async-lifecycle"
            with pytest.raises(RuntimeError, match="is closed"):
                await session.send_async("/help")
            session = replacement

            await session.close_async()
            with pytest.raises(RuntimeError, match="is closed"):
                await session.send_async("/help")
            with pytest.raises(RuntimeError, match="is closed"):
                await session.tool_async("read", {"file_path": "async-tool.txt"})
            with pytest.raises(RuntimeError, match="is closed"):
                session.register_dynamic_workflow_runtime()
            with pytest.raises(RuntimeError, match="is closed"):
                session.unregister_dynamic_tool("dynamic_workflow")
            with pytest.raises(RuntimeError, match="is closed"):
                session.register_agent_dir(workspace)

            isolated_options = SessionOptions()
            isolated_options.session_store = MemorySessionStore()
            with pytest.raises(RuntimeError, match="Session not found"):
                await agent.resume_session_async(
                    "python-async-lifecycle", isolated_options
                )

            resume_options = SessionOptions()
            resume_options.session_store = store
            resumed = await agent.resume_session_async(
                "python-async-lifecycle", resume_options
            )
            assert resumed.session_id == "python-async-lifecycle"
            await resumed.close_async()

        await agent.close_async()
        running = False
        await heartbeat_task
        assert ticks > 0, "lifecycle waits must yield control to the asyncio loop"

    asyncio.run(scenario())
