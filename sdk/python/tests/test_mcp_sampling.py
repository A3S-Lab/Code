"""Exercise actual MCP dispatch through the native asyncio sampling binding."""

import asyncio
import json
import sys
from pathlib import Path

import pytest

from a3s_code import Agent


CONFIG = '''default_model = "fixture/model"
providers "fixture" {
  api_key = "fixture-unused"
  models "model" { name = "Fixture" }
}'''
RESULT = {"role": "assistant", "model": "fixture",
          "content": {"type": "text", "text": "host-answer"}}
PEER = Path(__file__).parents[2] / "fixtures/mcp_sampling_server.py"


@pytest.mark.parametrize("case", ["unset", "async", "raise", "primitive", "timeout", "ceiling"])
def test_sampling_host_bridge(case, tmp_path):
    async def scenario():
        agent = await Agent.create_async(CONFIG)
        calls = 0
        cancelled = asyncio.Event()

        async def sample(server, params):
            nonlocal calls
            calls += 1
            assert server == case
            assert params["messages"][0]["content"]["text"] == "fixture-only"
            if case == "raise":
                raise RuntimeError("fixture refusal")
            if case == "primitive":
                return 42
            if case == "timeout":
                try:
                    await asyncio.Event().wait()
                finally:
                    cancelled.set()
            await asyncio.sleep(0.002)
            return RESULT

        try:
            assert not agent.has_sampling_handler_factory()
            assert agent.global_mcp_configs() == []
            for timeout in [0, 300001]:
                with pytest.raises(ValueError, match="timeout_ms"):
                    agent.set_sampling_handler_factory(sample, timeout)
            with pytest.raises(TypeError, match="callable"):
                agent.set_sampling_handler_factory(42)
            assert not agent.has_sampling_handler_factory()
            if case != "unset":
                agent.set_sampling_handler_factory(sample, 30 if case == "timeout" else 1000)
                assert agent.has_sampling_handler_factory()
            marker = tmp_path / "replies.json"
            server = {"name": case, "enabled": True, "transport": {
                "type": "stdio", "command": sys.executable,
                "args": [str(PEER), str(marker), "65" if case == "ceiling" else "1"],
            }, "env": {}}
            # Leave the callback's owning asyncio loop free during native I/O.
            await asyncio.to_thread(agent.sync_global_mcp_servers, [server])
            assert agent.global_mcp_configs()[0]["name"] == case
            replies = json.loads(marker.read_text())
            if case in ("async", "ceiling"):
                assert replies[0]["result"] == RESULT
                assert calls == (64 if case == "ceiling" else 1)
                if case == "ceiling":
                    assert "error" in replies[64]
            else:
                assert "error" in replies[0]
                assert calls == (0 if case == "unset" else 1)
                if case == "unset":
                    assert replies[0]["error"]["code"] == -32601
                if case == "timeout":
                    await asyncio.wait_for(cancelled.wait(), 1)
        finally:
            await asyncio.to_thread(agent.close)

    asyncio.run(scenario())
