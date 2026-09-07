"""Cross-SDK contract checks for secret-free middleware health."""

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
        / "model-middleware-health-v1.json"
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


def _assert_no_forbidden_fields(value: Any) -> None:
    forbidden = set(FIXTURE["forbidden_fields"])
    if isinstance(value, dict):
        for key, child in value.items():
            assert key not in forbidden, f"forbidden diagnostic field {key}"
            _assert_no_forbidden_fields(child)
    elif isinstance(value, list):
        for child in value:
            _assert_no_forbidden_fields(child)


def _assert_snapshot(health: dict[str, Any]) -> None:
    for field in FIXTURE["required_snapshot_fields"]:
        assert field in health, f"missing snapshot field {field}"
        assert health[field] == 0, f"{field} must start at zero"
    _assert_no_forbidden_fields(health)


def test_model_middleware_health_fixture() -> None:
    async def scenario() -> None:
        agent = await Agent.create_async(INLINE_CONFIG)
        with tempfile.TemporaryDirectory(prefix="a3s-python-middleware-health-") as workspace:
            options = SessionOptions()
            options.session_id = "python-middleware-health-fixture"
            session = await agent.session_async(workspace, options)
            try:
                health = session.model_middleware_health()
                assert isinstance(health, dict)
                _assert_snapshot(health)
            finally:
                await session.close_async()
                await agent.close_async()

    asyncio.run(scenario())
