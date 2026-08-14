package code

const (
	defaultRerankMaxCandidates                    = 100
	defaultRerankMaxFeatureBytesPerCandidate      = 4 * 1024
	defaultRerankMaxFingerprintsPerCandidate      = 128
	defaultRerankMaxScratchBytes                  = 4 * 1024 * 1024
	minimumRerankFeatureBytesPerCandidate    uint = 4
)

// WorkspaceReranker is a sealed typed choice for workspace second-stage
// ranking. Leave WorkspaceRetrievalOptions.Reranker nil to preserve RRF-only.
type WorkspaceReranker interface {
	workspaceRerankerWire() (deterministicWorkspaceRerankerWire, error)
}

// DeterministicWorkspaceReranker enables bounded deterministic MMR v1 after
// RRF. Construct it with NewDeterministicWorkspaceReranker so defaults remain
// aligned with Core.
type DeterministicWorkspaceReranker struct {
	MaxCandidates               uint
	MaxFeatureBytesPerCandidate uint
	MaxFingerprintsPerCandidate uint
	MaxScratchBytes             uint
}

// NewDeterministicWorkspaceReranker returns Core-compatible bounded defaults.
func NewDeterministicWorkspaceReranker() *DeterministicWorkspaceReranker {
	return &DeterministicWorkspaceReranker{
		MaxCandidates:               defaultRerankMaxCandidates,
		MaxFeatureBytesPerCandidate: defaultRerankMaxFeatureBytesPerCandidate,
		MaxFingerprintsPerCandidate: defaultRerankMaxFingerprintsPerCandidate,
		MaxScratchBytes:             defaultRerankMaxScratchBytes,
	}
}

type deterministicWorkspaceRerankerWire struct {
	MaxCandidates               uint `json:"max_candidates"`
	MaxFeatureBytesPerCandidate uint `json:"max_feature_bytes_per_candidate"`
	MaxFingerprintsPerCandidate uint `json:"max_fingerprints_per_candidate"`
	MaxScratchBytes             uint `json:"max_scratch_bytes"`
}

func (reranker *DeterministicWorkspaceReranker) workspaceRerankerWire() (
	deterministicWorkspaceRerankerWire,
	error,
) {
	if reranker == nil {
		return deterministicWorkspaceRerankerWire{}, invalid(
			"workspace_retrieval",
			"reranker cannot be a typed nil",
		)
	}
	if reranker.MaxCandidates < 1 || reranker.MaxCandidates > defaultRerankMaxCandidates {
		return deterministicWorkspaceRerankerWire{}, invalid(
			"workspace_retrieval",
			"reranker max candidates must be from 1 to 100",
		)
	}
	if reranker.MaxFeatureBytesPerCandidate < minimumRerankFeatureBytesPerCandidate ||
		reranker.MaxFeatureBytesPerCandidate > defaultRerankMaxFeatureBytesPerCandidate {
		return deterministicWorkspaceRerankerWire{}, invalid(
			"workspace_retrieval",
			"reranker feature bytes per candidate must be from 4 to 4096",
		)
	}
	if reranker.MaxFingerprintsPerCandidate < 1 ||
		reranker.MaxFingerprintsPerCandidate > defaultRerankMaxFingerprintsPerCandidate {
		return deterministicWorkspaceRerankerWire{}, invalid(
			"workspace_retrieval",
			"reranker fingerprints per candidate must be from 1 to 128",
		)
	}
	if reranker.MaxScratchBytes < 1 || reranker.MaxScratchBytes > defaultRerankMaxScratchBytes {
		return deterministicWorkspaceRerankerWire{}, invalid(
			"workspace_retrieval",
			"reranker scratch bytes must be from 1 to 4194304",
		)
	}
	return deterministicWorkspaceRerankerWire{
		MaxCandidates:               reranker.MaxCandidates,
		MaxFeatureBytesPerCandidate: reranker.MaxFeatureBytesPerCandidate,
		MaxFingerprintsPerCandidate: reranker.MaxFingerprintsPerCandidate,
		MaxScratchBytes:             reranker.MaxScratchBytes,
	}, nil
}
