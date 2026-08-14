package code

import (
	"encoding/json"
	"math"
	"sort"
)

type sdkRealBatchMetric struct {
	DocumentInputs            uint    `json:"documentInputs"`
	DocumentTextBytes         uint    `json:"documentTextBytes"`
	DocumentBatches           uint    `json:"documentBatches"`
	DocumentProviderRequests  uint    `json:"documentProviderRequests"`
	BatchLimitLowerBound      uint    `json:"batchLimitLowerBound"`
	InputLimitFlushes         uint    `json:"inputLimitFlushes"`
	TextByteLimitFlushes      uint    `json:"textByteLimitFlushes"`
	VectorByteLimitFlushes    uint    `json:"vectorByteLimitFlushes"`
	GenerationCompleteFlushes uint    `json:"generationCompleteFlushes"`
	TimeToFirstReadyMS        *uint64 `json:"timeToFirstReadyMs"`
	NonTextInputs             uint    `json:"nonTextInputs"`
}

type sdkRealRunMetric struct {
	Task                  string                  `json:"task"`
	CompletionCorrect     bool                    `json:"completionCorrect"`
	ToolProtocolOK        bool                    `json:"toolProtocolOk"`
	ExpectedPathRank      *uint                   `json:"expectedPathRank"`
	ResultCount           uint                    `json:"resultCount"`
	Algorithm             string                  `json:"algorithm"`
	RerankRequestedMode   string                  `json:"rerankRequestedMode"`
	RerankAppliedMode     string                  `json:"rerankAppliedMode"`
	SessionConstructionMS uint64                  `json:"sessionConstructionMs"`
	IndexReadyMS          uint64                  `json:"indexReadyMs"`
	TurnElapsedMS         uint64                  `json:"turnElapsedMs"`
	CloseMS               uint64                  `json:"closeMs"`
	PromptTokens          uint                    `json:"promptTokens"`
	CompletionTokens      uint                    `json:"completionTokens"`
	TotalTokens           uint                    `json:"totalTokens"`
	Phase                 WorkspaceRetrievalPhase `json:"phase"`
	CoverageBPS           uint16                  `json:"coverageBps"`
	EligibleFiles         uint                    `json:"eligibleFiles"`
	IndexedFiles          uint                    `json:"indexedFiles"`
	IndexedChunks         uint                    `json:"indexedChunks"`
	VectorRecords         uint                    `json:"vectorRecords"`
	VectorBytes           uint                    `json:"vectorBytes"`
	Batching              sdkRealBatchMetric      `json:"batching"`
	Provider              sdkRealProviderMetric   `json:"provider"`
	ReleasedAfterClose    bool                    `json:"releasedAfterClose"`
}

type sdkRealSummary struct {
	TaskAccuracy                 float64 `json:"taskAccuracy"`
	ToolProtocolRate             float64 `json:"toolProtocolRate"`
	PrecisionAt5                 float64 `json:"precisionAt5"`
	ReturnedResultPrecision      float64 `json:"returnedResultPrecision"`
	RecallAt5                    float64 `json:"recallAt5"`
	MRR                          float64 `json:"mrr"`
	NDCGAt5                      float64 `json:"ndcgAt5"`
	DocumentRequestAmplification float64 `json:"documentRequestAmplification"`
	MeanReturnedResults          float64 `json:"meanReturnedResults"`
	SessionConstructionP50MS     uint64  `json:"sessionConstructionP50Ms"`
	SessionConstructionP95MS     uint64  `json:"sessionConstructionP95Ms"`
	IndexReadyP50MS              uint64  `json:"indexReadyP50Ms"`
	IndexReadyP95MS              uint64  `json:"indexReadyP95Ms"`
	TimeToFirstReadyP50MS        uint64  `json:"timeToFirstReadyP50Ms"`
	TimeToFirstReadyP95MS        uint64  `json:"timeToFirstReadyP95Ms"`
	TurnP50MS                    uint64  `json:"turnP50Ms"`
	TurnP95MS                    uint64  `json:"turnP95Ms"`
	CloseP50MS                   uint64  `json:"closeP50Ms"`
	CloseP95MS                   uint64  `json:"closeP95Ms"`
	TotalTokens                  uint    `json:"totalTokens"`
	NonTextProviderInputs        uint    `json:"nonTextProviderInputs"`
	ReleasedAfterCloseRate       float64 `json:"releasedAfterCloseRate"`
}

type sdkRealEvaluationReport struct {
	SchemaVersion  uint               `json:"schemaVersion"`
	FixtureID      string             `json:"fixtureId"`
	FixtureDigest  string             `json:"fixtureDigest"`
	SDK            string             `json:"sdk"`
	ChatModel      string             `json:"chatModel"`
	Chunking       sdkRealChunking    `json:"chunking"`
	Rerank         sdkRealRerank      `json:"rerank"`
	Summary        sdkRealSummary     `json:"summary"`
	Runs           []sdkRealRunMetric `json:"runs"`
	AllGatesPassed bool               `json:"allGatesPassed"`
}

type sdkRealToolCall struct {
	Name     string         `json:"name"`
	Args     map[string]any `json:"args"`
	ExitCode int            `json:"exit_code"`
	Metadata map[string]any `json:"metadata"`
}

func sdkRealToolCalls(events []Event) ([]sdkRealToolCall, error) {
	calls := make([]sdkRealToolCall, 0, 1)
	for _, event := range events {
		if event.Type != EventToolEnd {
			continue
		}
		var call sdkRealToolCall
		if err := json.Unmarshal(event.Payload, &call); err != nil {
			return nil, err
		}
		calls = append(calls, call)
	}
	return calls, nil
}

func sdkRealResultMetrics(call sdkRealToolCall, task sdkRealFixtureTask) (*uint, uint) {
	values, _ := call.Metadata["results"].([]any)
	for index, value := range values {
		result, _ := value.(map[string]any)
		if result["path"] == task.ExpectedPath {
			rank := uint(index + 1)
			return &rank, uint(len(values))
		}
	}
	return nil, uint(len(values))
}

func sdkRealNestedString(value map[string]any, object, field string) string {
	nested, _ := value[object].(map[string]any)
	result, _ := nested[field].(string)
	return result
}

func summarizeSDKRealRuns(runs []sdkRealRunMetric) sdkRealSummary {
	var correct, protocol, relevant, returned, released uint
	var reciprocalRank, discountedGain float64
	var documentRequests, lowerBound, totalTokens, nonTextInputs uint
	firstReady := make([]uint64, 0, len(runs))
	construction := make([]uint64, 0, len(runs))
	indexReady := make([]uint64, 0, len(runs))
	turnElapsed := make([]uint64, 0, len(runs))
	closeElapsed := make([]uint64, 0, len(runs))
	for _, run := range runs {
		if run.CompletionCorrect {
			correct++
		}
		if run.ToolProtocolOK {
			protocol++
		}
		if run.ReleasedAfterClose {
			released++
		}
		if run.ExpectedPathRank != nil {
			rank := *run.ExpectedPathRank
			reciprocalRank += 1 / float64(rank)
			if rank <= 5 {
				relevant++
				discountedGain += 1 / math.Log2(float64(rank+1))
			}
		}
		returned += run.ResultCount
		documentRequests += run.Batching.DocumentProviderRequests
		lowerBound += run.Batching.BatchLimitLowerBound
		if run.Batching.TimeToFirstReadyMS != nil {
			firstReady = append(firstReady, *run.Batching.TimeToFirstReadyMS)
		}
		construction = append(construction, run.SessionConstructionMS)
		indexReady = append(indexReady, run.IndexReadyMS)
		turnElapsed = append(turnElapsed, run.TurnElapsedMS)
		closeElapsed = append(closeElapsed, run.CloseMS)
		totalTokens += run.TotalTokens
		nonTextInputs += run.Provider.NonTextInputs
	}
	count := float64(len(runs))
	return sdkRealSummary{
		TaskAccuracy:                 float64(correct) / count,
		ToolProtocolRate:             float64(protocol) / count,
		PrecisionAt5:                 float64(relevant) / (count * 5),
		ReturnedResultPrecision:      float64(relevant) / float64(returned),
		RecallAt5:                    float64(relevant) / count,
		MRR:                          reciprocalRank / count,
		NDCGAt5:                      discountedGain / count,
		DocumentRequestAmplification: float64(documentRequests) / float64(lowerBound),
		MeanReturnedResults:          float64(returned) / count,
		SessionConstructionP50MS:     sdkRealPercentile(construction, 0.5),
		SessionConstructionP95MS:     sdkRealPercentile(construction, 0.95),
		IndexReadyP50MS:              sdkRealPercentile(indexReady, 0.5),
		IndexReadyP95MS:              sdkRealPercentile(indexReady, 0.95),
		TimeToFirstReadyP50MS:        sdkRealPercentile(firstReady, 0.5),
		TimeToFirstReadyP95MS:        sdkRealPercentile(firstReady, 0.95),
		TurnP50MS:                    sdkRealPercentile(turnElapsed, 0.5),
		TurnP95MS:                    sdkRealPercentile(turnElapsed, 0.95),
		CloseP50MS:                   sdkRealPercentile(closeElapsed, 0.5),
		CloseP95MS:                   sdkRealPercentile(closeElapsed, 0.95),
		TotalTokens:                  totalTokens,
		NonTextProviderInputs:        nonTextInputs,
		ReleasedAfterCloseRate:       float64(released) / count,
	}
}

func sdkRealPercentile(values []uint64, fraction float64) uint64 {
	if len(values) == 0 {
		return 0
	}
	ordered := append([]uint64(nil), values...)
	sort.Slice(ordered, func(left, right int) bool { return ordered[left] < ordered[right] })
	rank := int(math.Ceil(fraction*float64(len(ordered)))) - 1
	if rank < 0 {
		rank = 0
	}
	return ordered[rank]
}
