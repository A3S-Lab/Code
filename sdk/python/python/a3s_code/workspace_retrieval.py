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
WorkspaceVectorEngine = Literal["a3s_memory", "a3s_vec"]
WorkspaceVecShadowPhase = Literal["disabled", "ready", "degraded", "closed"]
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
WorkspaceRerankMode = Literal["rrf_only", "deterministic"]
WorkspaceRerankAlgorithm = Literal[
    "rrf_k60", "rrf_k60+deterministic_mmr_v1"
]
WorkspaceRerankFallbackReason = Literal[
    "scratch_budget_exceeded", "invalid_configuration"
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


class WorkspaceEmbeddingBatchMetrics(TypedDict):
    document_inputs: int
    document_text_bytes: int
    document_batches: int
    document_provider_requests: int
    batch_limit_lower_bound: int
    input_limit_flushes: int
    text_byte_limit_flushes: int
    vector_byte_limit_flushes: int
    generation_complete_flushes: int
    time_to_first_ready_ms: Optional[int]
    non_text_inputs: int


class WorkspaceVecShadowStatus(TypedDict):
    phase: WorkspaceVecShadowPhase
    revision: int
    record_count: int
    accounted_bytes: int
    initialization_failures: int
    successful_mutations: int
    failed_mutations: int
    compared_queries: int
    matching_queries: int
    mismatched_queries: int
    failed_queries: int


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
    active_vector_engine: Optional[WorkspaceVectorEngine]
    vec_shadow: WorkspaceVecShadowStatus
    batching: WorkspaceEmbeddingBatchMetrics
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


class WorkspaceRerankStatus(TypedDict):
    requested_mode: WorkspaceRerankMode
    applied_mode: WorkspaceRerankMode
    algorithm: WorkspaceRerankAlgorithm
    input_candidates: int
    evaluated_candidates: int
    selected_candidates: int
    near_duplicate_candidates: int
    selected_near_duplicates: int
    feature_bytes: int
    accounted_scratch_bytes: int
    candidate_truncated: bool
    fallback: Optional[WorkspaceRerankFallbackReason]


class WorkspaceHybridSearchHit(TypedDict):
    chunk: WorkspaceChunk
    fused_score: float
    rerank_score: float
    redundancy_score: float
    exact_identifier: bool
    channels: List[WorkspaceHybridChannelRank]


class WorkspaceHybridSearchResult(TypedDict):
    hits: List[WorkspaceHybridSearchHit]
    semantic_status: WorkspaceRetrievalStatus
    catalog_revision: int
    source_revision: int
    channels: List[WorkspaceHybridChannelStatus]
    rerank: WorkspaceRerankStatus
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
    "WorkspaceEmbeddingBatchMetrics",
    "WorkspaceRetrievalStatus",
    "WorkspaceVecShadowPhase",
    "WorkspaceVecShadowStatus",
    "WorkspaceVectorEngine",
    "WorkspaceRerankFallbackReason",
    "WorkspaceRerankAlgorithm",
    "WorkspaceRerankMode",
    "WorkspaceRerankStatus",
    "WorkspaceSearchRequest",
    "WorkspaceSemanticFallbackReason",
    "WorkspaceSemanticSearchHit",
    "WorkspaceSemanticSearchResult",
]
