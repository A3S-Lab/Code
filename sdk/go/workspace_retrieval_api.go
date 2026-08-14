package code

import (
	"context"
	"math"
	"strings"
)

type WorkspaceSearchRequest struct {
	Query   string `json:"query"`
	Path    string `json:"path,omitempty"`
	Include string `json:"include,omitempty"`
	Limit   uint   `json:"limit,omitempty"`
}

type WorkspaceRetrievalPhase string

const (
	WorkspaceRetrievalDisabled WorkspaceRetrievalPhase = "disabled"
	WorkspaceRetrievalBuilding WorkspaceRetrievalPhase = "building"
	WorkspaceRetrievalReady    WorkspaceRetrievalPhase = "ready"
	WorkspaceRetrievalDegraded WorkspaceRetrievalPhase = "degraded"
	WorkspaceRetrievalClosed   WorkspaceRetrievalPhase = "closed"
)

type WorkspaceEmbeddingBatchMetrics struct {
	DocumentInputs             uint    `json:"document_inputs"`
	DocumentTextBytes          uint    `json:"document_text_bytes"`
	DocumentBatches            uint    `json:"document_batches"`
	DocumentProviderRequests   uint    `json:"document_provider_requests"`
	BatchLimitLowerBound       uint    `json:"batch_limit_lower_bound"`
	InputLimitFlushes          uint    `json:"input_limit_flushes"`
	TextByteLimitFlushes       uint    `json:"text_byte_limit_flushes"`
	VectorByteLimitFlushes     uint    `json:"vector_byte_limit_flushes"`
	GenerationCompleteFlushes uint    `json:"generation_complete_flushes"`
	TimeToFirstReadyMS         *uint64 `json:"time_to_first_ready_ms"`
	NonTextInputs              uint    `json:"non_text_inputs"`
}

type WorkspaceRetrievalStatus struct {
	Phase           WorkspaceRetrievalPhase      `json:"phase"`
	CatalogRevision uint64                       `json:"catalog_revision"`
	SourceRevision  uint64                       `json:"source_revision"`
	VectorRevision  uint64                       `json:"vector_revision"`
	EligibleFiles   uint                         `json:"eligible_files"`
	CatalogFiles    uint                         `json:"catalog_files"`
	CatalogChunks   uint                         `json:"catalog_chunks"`
	IndexedFiles    uint                         `json:"indexed_files"`
	IndexedChunks   uint                         `json:"indexed_chunks"`
	CoverageBPS     uint16                       `json:"coverage_bps"`
	QueueDepth      uint                         `json:"queue_depth"`
	FailedFiles     uint                         `json:"failed_files"`
	TotalFailures   uint64                       `json:"total_failures"`
	VectorRecords   uint                         `json:"vector_records"`
	VectorBytes     uint                         `json:"vector_bytes"`
	Batching        WorkspaceEmbeddingBatchMetrics `json:"batching"`
	Model           *EmbeddingProviderDescriptor `json:"model"`
}

type WorkspaceChunk struct {
	ID             string  `json:"id"`
	Path           string  `json:"path"`
	Language       *string `json:"language"`
	StartLine      uint    `json:"start_line"`
	EndLine        uint    `json:"end_line"`
	StartByte      uint    `json:"start_byte"`
	EndByte        uint    `json:"end_byte"`
	SourceRevision uint64  `json:"source_revision"`
	Text           string  `json:"text"`
	DigestVerified bool    `json:"digest_verified"`
}

type WorkspaceSemanticSearchHit struct {
	Chunk WorkspaceChunk `json:"chunk"`
	Score float32        `json:"score"`
}

type WorkspaceSemanticSearchResult struct {
	Hits            []WorkspaceSemanticSearchHit `json:"hits"`
	Status          WorkspaceRetrievalStatus     `json:"status"`
	SearchedRecords uint                         `json:"searched_records"`
	Truncated       bool                         `json:"truncated"`
	Fallback        *string                      `json:"fallback"`
}

type WorkspaceRetrievalChannel string

const (
	WorkspaceRetrievalExact      WorkspaceRetrievalChannel = "exact"
	WorkspaceRetrievalLexical    WorkspaceRetrievalChannel = "lexical"
	WorkspaceRetrievalStructural WorkspaceRetrievalChannel = "structural"
	WorkspaceRetrievalSemantic   WorkspaceRetrievalChannel = "semantic"
)

type WorkspaceHybridChannelRank struct {
	Channel WorkspaceRetrievalChannel `json:"channel"`
	Rank    uint                      `json:"rank"`
}

type WorkspaceHybridChannelStatus struct {
	Channel        WorkspaceRetrievalChannel `json:"channel"`
	CandidateCount uint                      `json:"candidate_count"`
	Truncated      bool                      `json:"truncated"`
	Fallback       *string                   `json:"fallback"`
}

type WorkspaceRerankMode string

const (
	WorkspaceRerankRRFOnly       WorkspaceRerankMode = "rrf_only"
	WorkspaceRerankDeterministic WorkspaceRerankMode = "deterministic"
)

type WorkspaceRerankAlgorithm string

const (
	WorkspaceRerankAlgorithmRRFK60             WorkspaceRerankAlgorithm = "rrf_k60"
	WorkspaceRerankAlgorithmDeterministicMMRV1 WorkspaceRerankAlgorithm = "rrf_k60+deterministic_mmr_v1"
)

type WorkspaceRerankFallbackReason string

const (
	WorkspaceRerankScratchBudgetExceeded WorkspaceRerankFallbackReason = "scratch_budget_exceeded"
	WorkspaceRerankInvalidConfiguration   WorkspaceRerankFallbackReason = "invalid_configuration"
)

type WorkspaceRerankStatus struct {
	RequestedMode           WorkspaceRerankMode            `json:"requested_mode"`
	AppliedMode             WorkspaceRerankMode            `json:"applied_mode"`
	Algorithm               WorkspaceRerankAlgorithm       `json:"algorithm"`
	InputCandidates         uint                           `json:"input_candidates"`
	EvaluatedCandidates     uint                           `json:"evaluated_candidates"`
	SelectedCandidates      uint                           `json:"selected_candidates"`
	NearDuplicateCandidates uint                           `json:"near_duplicate_candidates"`
	SelectedNearDuplicates  uint                           `json:"selected_near_duplicates"`
	FeatureBytes            uint                           `json:"feature_bytes"`
	AccountedScratchBytes   uint                           `json:"accounted_scratch_bytes"`
	CandidateTruncated      bool                           `json:"candidate_truncated"`
	Fallback                *WorkspaceRerankFallbackReason `json:"fallback"`
}

type WorkspaceHybridSearchHit struct {
	Chunk           WorkspaceChunk               `json:"chunk"`
	FusedScore      float64                      `json:"fused_score"`
	RerankScore     float64                      `json:"rerank_score"`
	RedundancyScore float64                      `json:"redundancy_score"`
	ExactIdentifier bool                         `json:"exact_identifier"`
	Channels        []WorkspaceHybridChannelRank `json:"channels"`
}

type WorkspaceHybridSearchResult struct {
	Hits            []WorkspaceHybridSearchHit     `json:"hits"`
	SemanticStatus  WorkspaceRetrievalStatus       `json:"semantic_status"`
	CatalogRevision uint64                         `json:"catalog_revision"`
	SourceRevision  uint64                         `json:"source_revision"`
	Channels        []WorkspaceHybridChannelStatus `json:"channels"`
	Rerank          WorkspaceRerankStatus           `json:"rerank"`
	Truncated       bool                           `json:"truncated"`
	Fallback        *string                        `json:"fallback"`
}

func validateWorkspaceSearch(operation string, request WorkspaceSearchRequest) error {
	if strings.TrimSpace(request.Query) == "" {
		return invalid(operation, "query cannot be empty")
	}
	if request.Limit > maxWorkspaceSearchLimit {
		return invalid(operation, "limit must be from 1 to 25 when set")
	}
	return nil
}

func (session *Session) WorkspaceRetrievalStatus(
	ctx context.Context,
) (WorkspaceRetrievalStatus, error) {
	const op = "session_workspace_retrieval_status"
	if err := validateSession(session, ctx, op); err != nil {
		return WorkspaceRetrievalStatus{}, err
	}
	var status WorkspaceRetrievalStatus
	err := session.runtime.Request(ctx, op, session.params(), &status)
	return status, err
}

func (session *Session) SemanticSearch(
	ctx context.Context,
	request WorkspaceSearchRequest,
) (WorkspaceSemanticSearchResult, error) {
	const op = "session_semantic_search"
	if err := validateSession(session, ctx, op); err != nil {
		return WorkspaceSemanticSearchResult{}, err
	}
	if err := validateWorkspaceSearch(op, request); err != nil {
		return WorkspaceSemanticSearchResult{}, err
	}
	var result WorkspaceSemanticSearchResult
	params := session.params()
	params["request"] = request
	err := session.runtime.Request(ctx, op, params, &result)
	return result, err
}

func (session *Session) HybridSearch(
	ctx context.Context,
	request WorkspaceSearchRequest,
) (WorkspaceHybridSearchResult, error) {
	const op = "session_hybrid_search"
	if err := validateSession(session, ctx, op); err != nil {
		return WorkspaceHybridSearchResult{}, err
	}
	if err := validateWorkspaceSearch(op, request); err != nil {
		return WorkspaceHybridSearchResult{}, err
	}
	var result WorkspaceHybridSearchResult
	params := session.params()
	params["request"] = request
	err := session.runtime.Request(ctx, op, params, &result)
	return result, err
}

func finiteVector(values []float32) bool {
	for _, value := range values {
		if math.IsNaN(float64(value)) || math.IsInf(float64(value), 0) {
			return false
		}
	}
	return true
}
