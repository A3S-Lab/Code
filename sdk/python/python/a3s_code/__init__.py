"""A3S Code Python SDK."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any


def _configure_bundled_moli() -> None:
    """Point the native core at the wheel's verified Moli sidecar.

    The Rust runtime manager remains the source of truth for validation and
    shared-cache locking. This small bridge only supplies the package-local
    path early enough that every Python process uses its bundled browser and
    never starts a second installation.
    """

    if os.environ.get("A3S_CODE_MOLI_EXECUTABLE"):
        return
    executable_name = "moli.exe" if os.name == "nt" else "moli"
    package_dir = Path(__file__).resolve().parent
    for candidate in (
        package_dir / executable_name,
        package_dir / "moli" / executable_name,
        package_dir / "resources" / "moli" / executable_name,
    ):
        try:
            if candidate.is_file() and (os.name == "nt" or os.access(candidate, os.X_OK)):
                os.environ["A3S_CODE_MOLI_EXECUTABLE"] = str(candidate)
                os.environ.setdefault("A3S_CODE_MOLI_DIR", str(candidate.parent))
                return
        except OSError:
            continue


_configure_bundled_moli()
del _configure_bundled_moli

from ._native_artifacts import ensure_unambiguous_native_extension as _ensure_native

_ensure_native()
del _ensure_native

from ._native import *
# Typed workspace vector authority selector exported by the native bridge.
from ._native import WorkspaceVectorEngineOption
from .event_protocol_v1 import (
    AGENT_EVENT_TYPES_V1,
    AgentEventTypeV1,
    EVENT_ENVELOPE_V1_VERSION,
    EventType,
    KnownAgentEventTypeV1,
)
from .evaluation_protocol_v1 import (
    EVALUATION_PROTOCOL_MAX_MESSAGE_BYTES,
    EVALUATION_PROTOCOL_SCHEMA_V1,
    EVALUATION_PROTOCOL_VERSION_V1,
    EVALUATION_WIRE_KINDS_V1,
    EvaluationWireEnvelopeV1,
    EvaluationWireKindV1,
    EvaluationWirePayloadV1,
    EvaluationWireTypeV1,
    EvidenceReadRequestPayloadV1,
    EvidenceSnapshotPayloadV1,
    AuxiliaryRunSpecPayloadV1,
    AuxiliaryRunSnapshotPayloadV1,
    AuxiliaryRunOutputPayloadV1,
    EvaluationResultPayloadV1,
    EvaluationRecordPayloadV1,
    KnownEvaluationWireKindV1,
)
from .errors import CodeErrorCode
from .memory_maintenance import (
    MemoryMaintenanceHealth,
    MemoryMaintenanceJobHealth,
    MemoryMaintenancePhase,
)
from .workspace_retrieval import (
    EmbeddingBatchFailure,
    EmbeddingBatchRequest,
    EmbeddingBatchResponse,
    EmbeddingBatchSuccess,
    EmbeddingCallback,
    EmbeddingInput,
    EmbeddingNormalization,
    EmbeddingProviderDescriptor,
    EmbeddingVector,
    WorkspaceChunk,
    WorkspaceEmbeddingBatchMetrics,
    WorkspaceHybridChannelRank,
    WorkspaceHybridChannelStatus,
    WorkspaceHybridFallbackReason,
    WorkspaceHybridSearchHit,
    WorkspaceHybridSearchResult,
    WorkspaceRetrievalChannel,
    WorkspaceRetrievalPhase,
    WorkspaceRetrievalStatus,
    WorkspaceVecShadowPhase,
    WorkspaceVecShadowStatus,
    WorkspaceVectorEngine,
    WorkspaceRerankAlgorithm,
    WorkspaceRerankFallbackReason,
    WorkspaceRerankMode,
    WorkspaceRerankStatus,
    WorkspaceSearchRequest,
    WorkspaceSemanticFallbackReason,
    WorkspaceSemanticSearchHit,
    WorkspaceSemanticSearchResult,
)


async def ensure_moli_async(config: Any | None = None) -> str:
    """Ensure the verified Moli runtime without blocking the asyncio loop."""

    import asyncio

    return await asyncio.to_thread(ensure_moli, config)
