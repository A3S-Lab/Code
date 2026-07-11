"""MemorySessionStore identity round-trip across the public Python SDK."""

from __future__ import annotations

import tempfile

import pytest

from a3s_code import Agent, MemorySessionStore, SessionOptions


INLINE_CONFIG = """
default_model = "anthropic/claude-sonnet-4-20250514"

providers "anthropic" {
  api_key = "test-key"
  models "claude-sonnet-4-20250514" {
    name = "Claude Sonnet 4"
  }
}
""".strip()


def test_memory_session_store_save_resume_roundtrip() -> None:
    with tempfile.TemporaryDirectory(prefix="a3s-code-python-memory-store-") as workspace:
        agent = Agent.create(INLINE_CONFIG)
        store = MemorySessionStore()

        create_options = SessionOptions()
        create_options.session_id = "python-memory-store-sdk-roundtrip"
        create_options.session_store = store
        session = agent.session(workspace, create_options)
        session.save()
        session.close()

        isolated_options = SessionOptions()
        isolated_options.session_store = MemorySessionStore()
        with pytest.raises(RuntimeError, match="Session not found"):
            agent.resume_session("python-memory-store-sdk-roundtrip", isolated_options)

        resume_options = SessionOptions()
        resume_options.session_store = store
        resumed = agent.resume_session("python-memory-store-sdk-roundtrip", resume_options)

        assert resumed.session_id == "python-memory-store-sdk-roundtrip"
