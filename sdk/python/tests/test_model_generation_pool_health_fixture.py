"""Cross-SDK contract checks for secret-free provider-pool health."""

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
        / "model-generation-pool-health-v1.json"
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
    assert 0 < health["pool"]["maxConcurrency"] <= FIXTURE["max_concurrency"]
    assert health["localReserved"] + health["localAvailable"] == health[
        "localMaxConcurrency"
    ]
    assert health["localMaxConcurrency"] <= health["pool"]["maxConcurrency"]
    identity = health["pool"]["identity"]
    for field in FIXTURE["required_identity_fields"]:
        assert field in identity, f"missing identity field {field}"
    assert identity["domain"] == "a3s.code.model-generation-pool.identity.v1"
    assert identity["digest"].startswith("sha256:")
    scheduler = health.get("scheduler")
    if scheduler is not None:
        assert scheduler["identity"] == identity
        assert scheduler["maxActive"] == health["pool"]["maxConcurrency"]
        assert scheduler["active"] <= scheduler["maxActive"]
        assert scheduler["pending"] <= scheduler["maxActive"]
    _assert_no_forbidden_fields(health)


def test_model_generation_pool_health_fixture() -> None:
    async def scenario() -> None:
        agent = await Agent.create_async(INLINE_CONFIG)
        with tempfile.TemporaryDirectory(prefix="a3s-python-pool-health-") as workspace:
            options = SessionOptions()
            options.session_id = "python-pool-health-fixture"
            session = await agent.session_async(workspace, options)
            try:
                snapshots = []
                for _ in range(min(FIXTURE["sample_limit"], 3)):
                    health = session.model_generation_pool_health()
                    assert health is not None
                    _assert_snapshot(health)
                    snapshots.append(health)
                aggregate = {
                    "sampleCount": len(snapshots),
                    "maxLocalReserved": max(value["localReserved"] for value in snapshots),
                    "maxSchedulerActive": max(
                        (value.get("scheduler") or {}).get("active", 0)
                        for value in snapshots
                    ),
                    "maxSchedulerPending": max(
                        (value.get("scheduler") or {}).get("pending", 0)
                        for value in snapshots
                    ),
                    "admitted": max(
                        (value.get("scheduler") or {}).get("admitted", 0)
                        for value in snapshots
                    ),
                    "released": max(
                        (value.get("scheduler") or {}).get("released", 0)
                        for value in snapshots
                    ),
                    "cancelled": max(
                        (value.get("scheduler") or {}).get("cancelled", 0)
                        for value in snapshots
                    ),
                    "rejected": max(
                        (value.get("scheduler") or {}).get("rejected", 0)
                        for value in snapshots
                    ),
                }
                assert set(aggregate) == set(FIXTURE["aggregate_fields"])
                _assert_no_forbidden_fields(aggregate)
            finally:
                await session.close_async()
        await agent.close_async()

    asyncio.run(scenario())
