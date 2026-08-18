"""Deterministic Python SDK coverage for confirmation inheritance.

The public wrapper contract is hermetic by default. Set ``A3S_CONFIG_FILE``
and ``A3S_CODE_SDK_REAL_AGENT_SMOKE=1`` to add a real delegated LLM turn.
"""

from __future__ import annotations

import os
import tempfile
from pathlib import Path

from a3s_code import Agent, PermissionPolicy, SessionOptions, WorkerAgentSpec


INLINE_CONFIG = """
default_model = "openai/confirmation-test"

providers "openai" {
  api_key = "hermetic-test-key"
  models "confirmation-test" {
    name = "Confirmation Test"
    tool_call = true
  }
}
""".strip()


def config_source() -> str:
    configured = os.environ.get("A3S_CONFIG_FILE")
    if configured:
        return Path(configured).read_text(encoding="utf-8")
    return INLINE_CONFIG


def test_confirmation_inheritance_surface() -> None:
    agent = Agent.create(config_source())
    try:
        workspace = tempfile.mkdtemp(prefix="a3s-python-confirmation-")

        worker_spec = WorkerAgentSpec(
            "test-writer",
            "Test worker with auto-approve confirmation",
            "implementer",
        )
        worker_spec.confirmation_inheritance = "auto_approve"
        worker_spec.max_steps = 3

        options = SessionOptions()
        options.permission_policy = PermissionPolicy(default_decision="allow")
        options.worker_agents = [worker_spec]
        session = agent.session(workspace, options)

        assert "task" in session.tool_names()

        reader_spec = WorkerAgentSpec(
            "test-reader",
            "Test worker with deny-on-ask confirmation",
            "read_only",
        )
        reader_spec.confirmation_inheritance = "deny_on_ask"
        reader_spec.max_steps = 2

        definition = session.register_worker_agent(reader_spec)
        assert definition.name == "test-reader"
        assert definition.confirmation_inheritance == "deny_on_ask"

        if os.environ.get("A3S_CODE_SDK_REAL_AGENT_SMOKE") == "1":
            test_file = Path(workspace) / "test.txt"
            test_file.write_text("CONFIRMATION_TEST_CONTENT", encoding="utf-8")
            result = session.task(
                {
                    "agent": "test-reader",
                    "description": "Read test file",
                    "prompt": "Read test.txt and reply with its content",
                    "max_steps": 2,
                }
            )
            assert result.exit_code == 0, result.output
            assert (
                "CONFIRMATION_TEST_CONTENT" in result.output
                or "test.txt" in result.output
            )
    finally:
        agent.close()


if __name__ == "__main__":
    test_confirmation_inheritance_surface()
    print("python sdk confirmation inheritance ok")
