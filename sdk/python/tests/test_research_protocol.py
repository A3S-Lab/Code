from __future__ import annotations

import json
from pathlib import Path

from a3s_code.research_protocol_v1 import (
    RESEARCH_PROTOCOL_MAX_MESSAGE_BYTES,
    RESEARCH_PROTOCOL_SCHEMA_V1,
    RESEARCH_PROTOCOL_VERSION_V1,
    RESEARCH_WIRE_KINDS_V1,
    ResearchWireTypeV1,
)


def _fixture_path() -> Path:
    return Path(__file__).resolve().parents[2] / "research" / "research-wire-v1-fixtures.json"


def test_research_wire_catalog_and_fixtures_match() -> None:
    manifest = json.loads(
        (_fixture_path().parent / "research-wire-v1.json").read_text(encoding="utf-8")
    )
    fixtures = json.loads(_fixture_path().read_text(encoding="utf-8"))

    assert manifest["schema"] == RESEARCH_PROTOCOL_SCHEMA_V1
    assert manifest["version"] == RESEARCH_PROTOCOL_VERSION_V1
    assert manifest["max_message_bytes"] == RESEARCH_PROTOCOL_MAX_MESSAGE_BYTES
    assert tuple(item["wire_name"] for item in manifest["kinds"]) == RESEARCH_WIRE_KINDS_V1
    assert ResearchWireTypeV1.RESEARCH_EVENT == "research_event"
    assert len(RESEARCH_WIRE_KINDS_V1) == 13

    valid = fixtures["valid"]
    assert valid["schema"] == RESEARCH_PROTOCOL_SCHEMA_V1
    assert valid["version"] == RESEARCH_PROTOCOL_VERSION_V1
    assert valid["kind"] in RESEARCH_WIRE_KINDS_V1
    assert valid["payload"]["payloadDigest"].startswith("sha256:")
    assert len(valid["payload"]["payloadDigest"]) == 71
    assert fixtures["unknown_top_level_field"]["future_field"] is True
    assert fixtures["unknown_payload_field"]["payload"]["future_field"] is True
    assert fixtures["unsupported_version"]["version"] == RESEARCH_PROTOCOL_VERSION_V1 + 1
