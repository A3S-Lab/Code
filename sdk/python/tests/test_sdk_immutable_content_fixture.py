"""Cross-SDK contract checks for ImmutableContentAdapter SessionOptions (SDK-IMM1)."""

from __future__ import annotations

import asyncio
import base64
import hashlib
import json
import tempfile
from pathlib import Path
from typing import Any

from a3s_code import Agent, SessionOptions


FIXTURE = json.loads(
    (
        Path(__file__).resolve().parents[2]
        / "evaluation"
        / "sdk-immutable-content-v1.json"
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


def test_sdk_immutable_content_fixture() -> None:
    async def scenario() -> None:
        assert FIXTURE["schema_version"] == 1
        assert FIXTURE["fixture_id"] == "sdk-immutable-content-v1"
        sample = FIXTURE["sample_write_request"]
        for field in FIXTURE["required_write_request_fields"]:
            assert field in sample
        for field in FIXTURE["required_reference_fields"]:
            assert field in FIXTURE["sample_reference"]
        _assert_no_forbidden(sample)

        def put(request: dict) -> dict:
            content = base64.b64decode(request["contentBase64"])
            digest = hashlib.sha256(content).hexdigest()
            return {
                "schema": "a3s.code.immutable-content-reference.v1",
                "binding_digest": request["binding"]["binding_digest"],
                "uri": f"a3s+test://python-sdk-imm1/{digest}",
                "content_digest": f"sha256:{digest}",
                "media_type": request["descriptor"]["media_type"],
                "size_bytes": len(content),
                "reference_digest": f"sha256:{'b' * 64}",
            }

        agent = await Agent.create_async(INLINE_CONFIG)
        with tempfile.TemporaryDirectory(prefix="a3s-python-sdk-imm-") as workspace:
            options = SessionOptions()
            options.session_id = "python-sdk-immutable-content-fixture"
            options.immutable_content_adapter = {
                "authority_digest": f"sha256:{'a' * 64}",
                "maximum_bytes": 4096,
                "adapter_name": "python-sdk-imm1",
                "put": put,
                "timeout_ms": 5_000,
            }
            session = await agent.session_async(workspace, options)
            try:
                assert session is not None
            finally:
                await session.close()
                await agent.close()

    asyncio.run(scenario())
