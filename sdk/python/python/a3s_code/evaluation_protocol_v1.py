"""Generated evaluation wire protocol declarations.

Generated from core/src/evaluation/protocol.rs. Run
node scripts/generate_evaluation_protocol_artifacts.mjs to update.

Payload values intentionally remain mappings: Core is the single authority for
closed payload validation, while hosts own transport and business semantics.
"""

from typing import Final, Literal, Mapping, Tuple, TypedDict

EVALUATION_PROTOCOL_VERSION_V1: Final[int] = 1
EVALUATION_PROTOCOL_SCHEMA_V1: Final[str] = "a3s.code.evaluation-wire.v1"
EVALUATION_PROTOCOL_MAX_MESSAGE_BYTES: Final[int] = 33554432

KnownEvaluationWireKindV1 = Literal[
    "evidence_read_request",
    "evidence_snapshot",
    "auxiliary_run_spec",
    "auxiliary_run_snapshot",
    "auxiliary_run_output",
    "evaluation_result",
    "evaluation_record",
]
EvaluationWireKindV1 = KnownEvaluationWireKindV1

EVALUATION_WIRE_KINDS_V1: Final[Tuple[KnownEvaluationWireKindV1, ...]] = (
    "evidence_read_request",
    "evidence_snapshot",
    "auxiliary_run_spec",
    "auxiliary_run_snapshot",
    "auxiliary_run_output",
    "evaluation_result",
    "evaluation_record",
)


class EvaluationWireTypeV1:
    """Canonical string constants for evaluation wire version 1."""

    EVIDENCE_READ_REQUEST: Final[str] = "evidence_read_request"
    EVIDENCE_SNAPSHOT: Final[str] = "evidence_snapshot"
    AUXILIARY_RUN_SPEC: Final[str] = "auxiliary_run_spec"
    AUXILIARY_RUN_SNAPSHOT: Final[str] = "auxiliary_run_snapshot"
    AUXILIARY_RUN_OUTPUT: Final[str] = "auxiliary_run_output"
    EVALUATION_RESULT: Final[str] = "evaluation_result"
    EVALUATION_RECORD: Final[str] = "evaluation_record"


EvaluationWirePayloadV1 = Mapping[str, object]
EvidenceReadRequestPayloadV1 = EvaluationWirePayloadV1
EvidenceSnapshotPayloadV1 = EvaluationWirePayloadV1
AuxiliaryRunSpecPayloadV1 = EvaluationWirePayloadV1
AuxiliaryRunSnapshotPayloadV1 = EvaluationWirePayloadV1
AuxiliaryRunOutputPayloadV1 = EvaluationWirePayloadV1
EvaluationResultPayloadV1 = EvaluationWirePayloadV1
EvaluationRecordPayloadV1 = EvaluationWirePayloadV1


class EvaluationWireEnvelopeV1(TypedDict):
    """Strict top-level envelope shape emitted by Code Core."""

    schema: str
    version: int
    kind: KnownEvaluationWireKindV1
    payload: EvaluationWirePayloadV1


__all__ = [
    "EVALUATION_PROTOCOL_MAX_MESSAGE_BYTES",
    "EVALUATION_PROTOCOL_SCHEMA_V1",
    "EVALUATION_PROTOCOL_VERSION_V1",
    "EVALUATION_WIRE_KINDS_V1",
    "EvaluationWireEnvelopeV1",
    "EvaluationWireKindV1",
    "EvaluationWirePayloadV1",
    "EvaluationWireTypeV1",
    "KnownEvaluationWireKindV1",
    "EvidenceReadRequestPayloadV1",
    "EvidenceSnapshotPayloadV1",
    "AuxiliaryRunSpecPayloadV1",
    "AuxiliaryRunSnapshotPayloadV1",
    "AuxiliaryRunOutputPayloadV1",
    "EvaluationResultPayloadV1",
    "EvaluationRecordPayloadV1",
]
