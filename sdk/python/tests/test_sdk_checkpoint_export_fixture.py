"""Cross-SDK contract checks for live checkpoint export sinks (SDK-CP1)."""

from __future__ import annotations

import asyncio
import json
import tempfile
from pathlib import Path
from typing import Any

from a3s_code import Agent, SessionOptions


FIXTURE = json.loads(
    (
        Path(__file__).resolve().parents[2]
        / "evaluation"
        / "sdk-checkpoint-export-v1.json"
    ).read_text(encoding="utf-8")
)

INLINE_CONFIG = """
default_model = "openai/fixture-model"
providers "openai" {
  apiKey = "fixture-key-never-sent"
  baseUrl = "https://fixture.invalid/v1"
  models "fixture-model" {
    name = "Fixture Model"
  }
}
""".strip()


def _assert_no_forbidden(value: Any) -> None:
    forbidden = set(FIXTURE["forbidden_fields"])
    if isinstance(value, list):
        for child in value:
            _assert_no_forbidden(child)
        return
    if isinstance(value, dict):
        for key, child in value.items():
            assert key not in forbidden, f"forbidden diagnostic field {key}"
            _assert_no_forbidden(child)


def test_sdk_checkpoint_export_fixture() -> None:
    async def scenario() -> None:
        assert FIXTURE["schema_version"] == 1
        assert FIXTURE["fixture_id"] == "sdk-checkpoint-export-v1"
        sample = FIXTURE["sample_export"]
        for field in FIXTURE["required_export_fields"]:
            assert field in sample
        for field in FIXTURE["required_descriptor_fields"]:
            assert field in sample["descriptor"]
        _assert_no_forbidden(sample)

        agent = await Agent.create_async(INLINE_CONFIG)
        with tempfile.TemporaryDirectory(prefix="a3s-python-sdk-cp-export-") as workspace:
            options = SessionOptions()
            options.session_id = "python-sdk-checkpoint-export-fixture"
            session = await agent.session_async(workspace, options)
            try:
                assert hasattr(session, "set_session_checkpoint_export_sink")
                invoked = {"count": 0}

                def handler(export_payload: dict) -> dict:
                    invoked["count"] += 1
                    assert "descriptor" in export_payload
                    assert isinstance(export_payload.get("contentBase64"), str)
                    return {"ok": True}

                session.set_session_checkpoint_export_sink(handler, timeout_ms=5_000)
                assert invoked["count"] == 0
                session.set_session_checkpoint_export_sink(None)
            finally:
                await session.close()
                await agent.close()

    asyncio.run(scenario())
