"""Static contracts for session-bound workspace retrieval.

The runtime objects are implemented by the native extension. These protocols
and ``TypedDict`` declarations make callback, request, status, and result
shapes available to type checkers without duplicating retrieval behavior in
Python.
"""

from typing import Awaitable, List, Literal, Optional, Protocol, TypedDict, Union

EmbeddingNormalization = Literal["none", "unit"]
WorkspaceRetrievalPhase = Literal[
    "disabled", "building", "ready", "degraded", "closed"
]
WorkspaceRetrievalChannel = Literal["exact", "lexical", "structural", "semantic"]
WorkspaceSemanticFallbackReason = Literal[
    "building",
    "degraded",
    "closed",
    "query_embedding_failed",
    "vector_search_failed",
    "revision_changed",
    "filtered_stale_hits",
]
WorkspaceHybridFallbackReason = Literal[
    "unavailable",
    "building",
    "degraded",
    "query_embedding_failed",
    "vector_search_failed",
    "structural_query_failed",
    "revision_changed",
    "filtered_stale_hits",
]


class EmbeddingInput(TypedDict):
    """One stable ID and source text supplied to the host embedder."""

    id: str
    text: str


class EmbeddingBatchRequest(TypedDict):
    """One bounded batch supplied to an asynchronous embedding callback."""

    inputs: List[EmbeddingInput]
    text_bytes: int


class EmbeddingVector(TypedDict):
    id: str
    values: List[float]


class EmbeddingBatchSuccess(TypedDict):
    vectors: List[EmbeddingVector]


class _EmbeddingBatchFailureRequired(TypedDict):
    kind: Literal[
        "cancelled",
        "timeout",
        "rate_limited",
        "unavailable",
        "authentication",
        "invalid_request",
        "other",
    ]


class EmbeddingBatchFailure(_EmbeddingBatchFailureRequired, total=False):
    retry_after_ms: int


EmbeddingBatchResponse = Union[EmbeddingBatchSuccess, EmbeddingBatchFailure]


class EmbeddingCallback(Protocol):
    """Host callback invoked on the asyncio loop that created the session."""

    def __call__(self, request: EmbeddingBatchRequest) -> Awaitable[EmbeddingBatchResponse]:
        ...


class _WorkspaceSearchRequestRequired(TypedDict):
    query: str


class WorkspaceSearchRequest(_WorkspaceSearchRequestRequired, total=False):
    path: str
    include: str
    limit: int


class EmbeddingProviderDescriptor(TypedDict):
    provider: str
    model: str
    revision: Optional[str]
    dimension: int
    normalization: EmbeddingNormalization


class WorkspaceRetrievalStatus(TypedDict):
    phase: WorkspaceRetrievalPhase
    catalog_revision: int
    source_revision: int
    vector_revision: int
    eligible_files: int
    catalog_files: int
    catalog_chunks: int
    indexed_files: int
    indexed_chunks: int
    coverage_bps: int
    queue_depth: int
    failed_files: int
    total_failures: int
    vector_records: int
    vector_bytes: int
    model: Optional[EmbeddingProviderDescriptor]


class WorkspaceChunk(TypedDict):
    id: str
    path: str
    language: Optional[str]
    start_line: int
    end_line: int
    start_byte: int
    end_byte: int
    source_revision: int
    text: str
    digest_verified: bool


class WorkspaceSemanticSearchHit(TypedDict):
    chunk: WorkspaceChunk
    score: float


class WorkspaceSemanticSearchResult(TypedDict):
    hits: List[WorkspaceSemanticSearchHit]
    status: WorkspaceRetrievalStatus
    searched_records: int
    truncated: bool
    fallback: Optional[WorkspaceSemanticFallbackReason]


class WorkspaceHybridChannelRank(TypedDict):
    channel: WorkspaceRetrievalChannel
    rank: int


class WorkspaceHybridChannelStatus(TypedDict):
    channel: WorkspaceRetrievalChannel
    candidate_count: int
    truncated: bool
    fallback: Optional[WorkspaceHybridFallbackReason]


class WorkspaceHybridSearchHit(TypedDict):
    chunk: WorkspaceChunk
    fused_score: float
    exact_identifier: bool
    channels: List[WorkspaceHybridChannelRank]


class WorkspaceHybridSearchResult(TypedDict):
    hits: List[WorkspaceHybridSearchHit]
    semantic_status: WorkspaceRetrievalStatus
    catalog_revision: int
    source_revision: int
    channels: List[WorkspaceHybridChannelStatus]
    truncated: bool
    fallback: Optional[WorkspaceHybridFallbackReason]


__all__ = [
    "EmbeddingBatchFailure",
    "EmbeddingBatchRequest",
    "EmbeddingBatchResponse",
    "EmbeddingBatchSuccess",
    "EmbeddingCallback",
    "EmbeddingInput",
    "EmbeddingNormalization",
    "EmbeddingProviderDescriptor",
    "EmbeddingVector",
    "WorkspaceChunk",
    "WorkspaceHybridChannelRank",
    "WorkspaceHybridChannelStatus",
    "WorkspaceHybridFallbackReason",
    "WorkspaceHybridSearchHit",
    "WorkspaceHybridSearchResult",
    "WorkspaceRetrievalChannel",
    "WorkspaceRetrievalPhase",
    "WorkspaceRetrievalStatus",
    "WorkspaceSearchRequest",
    "WorkspaceSemanticFallbackReason",
    "WorkspaceSemanticSearchHit",
    "WorkspaceSemanticSearchResult",
]
