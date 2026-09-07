"""Cross-SDK contract checks for Skill-only capability batches (SDK-CAP1)."""

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
        / "sdk-capability-batch-v1.json"
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


def test_sdk_capability_batch_fixture() -> None:
    async def scenario() -> None:
        assert FIXTURE["schema_version"] == 1
        assert FIXTURE["fixture_id"] == "sdk-capability-batch-v1"
        batch = FIXTURE["sample_batch"]
        for field in FIXTURE["required_batch_fields"]:
            assert field in batch

        agent = await Agent.create_async(INLINE_CONFIG)
        with tempfile.TemporaryDirectory(prefix="a3s-python-sdk-cap-batch-") as workspace:
            options = SessionOptions()
            options.session_id = "python-sdk-capability-batch-fixture"
            session = await agent.session_async(workspace, options)
            try:
                receipt = session.apply_capability_batch(batch)
                assert (
                    receipt["previousGeneration"]
                    == FIXTURE["expected_receipt"]["previousGeneration"]
                )
                assert (
                    receipt["committedGeneration"]
                    == FIXTURE["expected_receipt"]["committedGeneration"]
                )
                for field in FIXTURE["required_receipt_fields"]:
                    assert field in receipt
                _assert_no_forbidden_fields(receipt)
                stamp = session.capability_catalog_stamp()
                assert stamp["generation"] == 1
            finally:
                await session.close()
                await agent.close()

    asyncio.run(scenario())
