from __future__ import annotations

import json
from pathlib import Path

from a3s_code.evaluation_protocol_v1 import (
    EVALUATION_PROTOCOL_MAX_MESSAGE_BYTES,
    EVALUATION_PROTOCOL_SCHEMA_V1,
    EVALUATION_PROTOCOL_VERSION_V1,
    EVALUATION_WIRE_KINDS_V1,
    EvaluationWireTypeV1,
)


def _fixture_path() -> Path:
    return Path(__file__).resolve().parents[2] / "evaluation" / "evaluation-wire-v1-fixtures.json"


def test_evaluation_wire_catalog_and_fixtures_match() -> None:
    manifest = json.loads(
        (_fixture_path().parent / "evaluation-wire-v1.json").read_text(encoding="utf-8")
    )
    fixtures = json.loads(_fixture_path().read_text(encoding="utf-8"))

    assert manifest["schema"] == EVALUATION_PROTOCOL_SCHEMA_V1
    assert manifest["version"] == EVALUATION_PROTOCOL_VERSION_V1
    assert manifest["max_message_bytes"] == EVALUATION_PROTOCOL_MAX_MESSAGE_BYTES
    assert tuple(item["wire_name"] for item in manifest["kinds"]) == EVALUATION_WIRE_KINDS_V1
    assert EvaluationWireTypeV1.EVALUATION_RECORD == "evaluation_record"

    valid = fixtures["valid"]
    assert valid["schema"] == EVALUATION_PROTOCOL_SCHEMA_V1
    assert valid["version"] == EVALUATION_PROTOCOL_VERSION_V1
    assert valid["kind"] in EVALUATION_WIRE_KINDS_V1
    assert fixtures["unknown_top_level_field"]["future_field"] is True
    assert fixtures["unknown_payload_field"]["payload"]["future_field"] is True
    assert fixtures["unsupported_version"]["version"] == EVALUATION_PROTOCOL_VERSION_V1 + 1
