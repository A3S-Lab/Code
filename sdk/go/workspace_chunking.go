package code

import "strings"

const (
	minimumWorkspaceChunkTargetBytes = 4
	maximumWorkspaceChunkTargetBytes = 64 * 1024
	maximumWorkspaceSeparators       = 16
	maximumWorkspaceSeparatorBytes   = 64
)

func recursiveWorkspaceDefaultSeparators() []string {
	return []string{"\n\n", "\n", ". ", "。", " "}
}

// WorkspaceChunkingStrategy is a sealed typed choice for session-owned text
// catalogs. Leave WorkspaceRetrievalOptions.ChunkingStrategy nil to preserve
// the compatible line strategy.
type WorkspaceChunkingStrategy interface {
	workspaceChunkingStrategyWire() (workspaceChunkingStrategyWire, error)
}

// LineWorkspaceChunkingStrategy explicitly selects the compatible line and
// byte ceilings owned by Core.
type LineWorkspaceChunkingStrategy struct{}

// NewLineWorkspaceChunkingStrategy returns the explicit compatibility choice.
func NewLineWorkspaceChunkingStrategy() *LineWorkspaceChunkingStrategy {
	return &LineWorkspaceChunkingStrategy{}
}

// FixedWindowWorkspaceChunkingStrategy selects UTF-8-safe byte windows with
// bounded overlap.
type FixedWindowWorkspaceChunkingStrategy struct {
	TargetBytes  uint
	OverlapBytes uint
}

// NewFixedWindowWorkspaceChunkingStrategy validates and returns fixed windows.
func NewFixedWindowWorkspaceChunkingStrategy(
	targetBytes uint,
	overlapBytes uint,
) (*FixedWindowWorkspaceChunkingStrategy, error) {
	strategy := &FixedWindowWorkspaceChunkingStrategy{
		TargetBytes:  targetBytes,
		OverlapBytes: overlapBytes,
	}
	if err := validateWorkspaceChunkWindow(targetBytes, overlapBytes); err != nil {
		return nil, err
	}
	return strategy, nil
}

// RecursiveWorkspaceChunkingStrategy selects prioritized separators with a
// UTF-8-safe hard-boundary fallback. Omit separators to use Core defaults.
type RecursiveWorkspaceChunkingStrategy struct {
	TargetBytes  uint
	OverlapBytes uint
	Separators   []string
}

// NewRecursiveWorkspaceChunkingStrategy validates and returns recursive windows.
func NewRecursiveWorkspaceChunkingStrategy(
	targetBytes uint,
	overlapBytes uint,
	separators ...string,
) (*RecursiveWorkspaceChunkingStrategy, error) {
	if err := validateWorkspaceChunkWindow(targetBytes, overlapBytes); err != nil {
		return nil, err
	}
	if len(separators) == 0 {
		separators = recursiveWorkspaceDefaultSeparators()
	}
	if err := validateWorkspaceSeparators(separators); err != nil {
		return nil, err
	}
	return &RecursiveWorkspaceChunkingStrategy{
		TargetBytes:  targetBytes,
		OverlapBytes: overlapBytes,
		Separators:   append([]string(nil), separators...),
	}, nil
}

type workspaceChunkingStrategyWire struct {
	Line        *lineWorkspaceChunkingWire        `json:"line,omitempty"`
	FixedWindow *fixedWindowWorkspaceChunkingWire `json:"fixed_window,omitempty"`
	Recursive   *recursiveWorkspaceChunkingWire   `json:"recursive,omitempty"`
}

type lineWorkspaceChunkingWire struct{}

type fixedWindowWorkspaceChunkingWire struct {
	TargetBytes  uint `json:"target_bytes"`
	OverlapBytes uint `json:"overlap_bytes"`
}

type recursiveWorkspaceChunkingWire struct {
	TargetBytes  uint     `json:"target_bytes"`
	OverlapBytes uint     `json:"overlap_bytes"`
	Separators   []string `json:"separators"`
}

func (strategy *LineWorkspaceChunkingStrategy) workspaceChunkingStrategyWire() (
	workspaceChunkingStrategyWire,
	error,
) {
	if strategy == nil {
		return workspaceChunkingStrategyWire{}, invalid(
			"workspace_retrieval",
			"chunking strategy cannot be a typed nil",
		)
	}
	return workspaceChunkingStrategyWire{Line: &lineWorkspaceChunkingWire{}}, nil
}

func (strategy *FixedWindowWorkspaceChunkingStrategy) workspaceChunkingStrategyWire() (
	workspaceChunkingStrategyWire,
	error,
) {
	if strategy == nil {
		return workspaceChunkingStrategyWire{}, invalid(
			"workspace_retrieval",
			"chunking strategy cannot be a typed nil",
		)
	}
	if err := validateWorkspaceChunkWindow(strategy.TargetBytes, strategy.OverlapBytes); err != nil {
		return workspaceChunkingStrategyWire{}, err
	}
	return workspaceChunkingStrategyWire{
		FixedWindow: &fixedWindowWorkspaceChunkingWire{
			TargetBytes:  strategy.TargetBytes,
			OverlapBytes: strategy.OverlapBytes,
		},
	}, nil
}

func (strategy *RecursiveWorkspaceChunkingStrategy) workspaceChunkingStrategyWire() (
	workspaceChunkingStrategyWire,
	error,
) {
	if strategy == nil {
		return workspaceChunkingStrategyWire{}, invalid(
			"workspace_retrieval",
			"chunking strategy cannot be a typed nil",
		)
	}
	if err := validateWorkspaceChunkWindow(strategy.TargetBytes, strategy.OverlapBytes); err != nil {
		return workspaceChunkingStrategyWire{}, err
	}
	if err := validateWorkspaceSeparators(strategy.Separators); err != nil {
		return workspaceChunkingStrategyWire{}, err
	}
	return workspaceChunkingStrategyWire{
		Recursive: &recursiveWorkspaceChunkingWire{
			TargetBytes:  strategy.TargetBytes,
			OverlapBytes: strategy.OverlapBytes,
			Separators:   append([]string(nil), strategy.Separators...),
		},
	}, nil
}

func validateWorkspaceChunkWindow(targetBytes uint, overlapBytes uint) error {
	if targetBytes < minimumWorkspaceChunkTargetBytes ||
		targetBytes > maximumWorkspaceChunkTargetBytes {
		return invalid(
			"workspace_retrieval",
			"chunk target bytes must be from 4 to 65536",
		)
	}
	if overlapBytes >= targetBytes {
		return invalid(
			"workspace_retrieval",
			"chunk overlap bytes must be smaller than target bytes",
		)
	}
	return nil
}

func validateWorkspaceSeparators(separators []string) error {
	if len(separators) < 1 || len(separators) > maximumWorkspaceSeparators {
		return invalid(
			"workspace_retrieval",
			"recursive separators must contain from 1 to 16 entries",
		)
	}
	unique := make(map[string]struct{}, len(separators))
	for _, separator := range separators {
		if len(separator) < 1 || len(separator) > maximumWorkspaceSeparatorBytes ||
			strings.ContainsRune(separator, '\x00') {
			return invalid(
				"workspace_retrieval",
				"recursive separators must contain from 1 to 64 bytes and no NUL",
			)
		}
		if _, exists := unique[separator]; exists {
			return invalid(
				"workspace_retrieval",
				"recursive separators must be unique",
			)
		}
		unique[separator] = struct{}{}
	}
	return nil
}

var (
	_ WorkspaceChunkingStrategy = (*LineWorkspaceChunkingStrategy)(nil)
	_ WorkspaceChunkingStrategy = (*FixedWindowWorkspaceChunkingStrategy)(nil)
	_ WorkspaceChunkingStrategy = (*RecursiveWorkspaceChunkingStrategy)(nil)
)
