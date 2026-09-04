package code

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"time"
)

const (
	defaultEmbeddingProviderTimeout = 30 * time.Second
	maxEmbeddingProviderTimeout     = 5 * time.Minute
	defaultRetrievalMaxRecords      = 100_000
	defaultRetrievalMaxBytes        = 128 * 1024 * 1024
	defaultRetrievalShutdownTimeout = 5 * time.Second
	maxRetrievalShutdownTimeout     = 30 * time.Second
	maxWorkspaceSearchLimit         = 25
)

type EmbeddingNormalization string

const (
	EmbeddingNormalizationNone EmbeddingNormalization = "none"
	EmbeddingNormalizationUnit EmbeddingNormalization = "unit"
)

// EmbeddingProviderDescriptor is the immutable identity and output shape of
// one provider generation.
type EmbeddingProviderDescriptor struct {
	Provider      string                 `json:"provider"`
	Model         string                 `json:"model"`
	Revision      string                 `json:"revision,omitempty"`
	Dimension     uint                   `json:"dimension"`
	Normalization EmbeddingNormalization `json:"normalization"`
}

type EmbeddingInput struct {
	ID   string `json:"id"`
	Text string `json:"text"`
}

type EmbeddingBatchRequest struct {
	Inputs    []EmbeddingInput `json:"inputs"`
	TextBytes uint             `json:"text_bytes"`
}

type EmbeddingVector struct {
	ID     string    `json:"id"`
	Values []float32 `json:"values"`
}

type EmbeddingBatchResponse struct {
	Vectors []EmbeddingVector `json:"vectors"`
}

type EmbeddingFailureKind string

const (
	EmbeddingFailureCancelled      EmbeddingFailureKind = "cancelled"
	EmbeddingFailureTimeout        EmbeddingFailureKind = "timeout"
	EmbeddingFailureRateLimited    EmbeddingFailureKind = "rate_limited"
	EmbeddingFailureUnavailable    EmbeddingFailureKind = "unavailable"
	EmbeddingFailureAuthentication EmbeddingFailureKind = "authentication"
	EmbeddingFailureInvalidRequest EmbeddingFailureKind = "invalid_request"
	EmbeddingFailureOther          EmbeddingFailureKind = "other"
)

// EmbeddingError lets a provider report retry-safe failure categories without
// exposing remote response bodies to Rust diagnostics.
type EmbeddingError struct {
	Kind       EmbeddingFailureKind
	RetryAfter time.Duration
	Err        error
}

func (err *EmbeddingError) Error() string {
	if err == nil {
		return "<nil>"
	}
	if err.Err != nil {
		return err.Err.Error()
	}
	return fmt.Sprintf("embedding provider failed: %s", err.Kind)
}

func (err *EmbeddingError) Unwrap() error {
	if err == nil {
		return nil
	}
	return err.Err
}

// EmbeddingProvider is a host-owned, context-aware source of vectors. Code
// owns batching, validation, retries, indexing, ranking, and source checks.
type EmbeddingProvider interface {
	Descriptor() EmbeddingProviderDescriptor
	Embed(context.Context, EmbeddingBatchRequest) (EmbeddingBatchResponse, error)
}

// WorkspaceRetrievalOptions enables a bounded, session-owned semantic index
// and its local lexical catalog. Semantic vectors are owned by the Memory
// adapter; an omitted lexical engine lets the bridge select its compiled
// product default (zvec-rust for native builds, portable BM25 otherwise).
type WorkspaceRetrievalOptions struct {
	Provider         EmbeddingProvider
	LexicalEngine    WorkspaceLexicalEngine
	Reranker         WorkspaceReranker
	ChunkingStrategy WorkspaceChunkingStrategy
	ProviderTimeout  time.Duration
	MaxRecords       uint
	MaxBytes         uint
	ShutdownTimeout  time.Duration
}

func NewWorkspaceRetrievalOptions(provider EmbeddingProvider) *WorkspaceRetrievalOptions {
	return &WorkspaceRetrievalOptions{
		Provider:        provider,
		ProviderTimeout: defaultEmbeddingProviderTimeout,
		MaxRecords:      defaultRetrievalMaxRecords,
		MaxBytes:        defaultRetrievalMaxBytes,
		ShutdownTimeout: defaultRetrievalShutdownTimeout,
	}
}

type embeddingBatchFailure struct {
	Kind         EmbeddingFailureKind `json:"kind"`
	RetryAfterMS *uint64              `json:"retry_after_ms,omitempty"`
}

type workspaceRetrievalWireOptions struct {
	HandlerID             string                              `json:"handler_id"`
	Provider              string                              `json:"provider"`
	Model                 string                              `json:"model"`
	Revision              string                              `json:"revision,omitempty"`
	Dimension             uint                                `json:"dimension"`
	Normalization         EmbeddingNormalization              `json:"normalization"`
	LexicalEngine         WorkspaceLexicalEngine              `json:"lexical_engine,omitempty"`
	ProviderTimeout       uint64                              `json:"provider_timeout_ms"`
	MaxRecords            uint                                `json:"max_records"`
	MaxBytes              uint                                `json:"max_bytes"`
	ShutdownTimeout       uint64                              `json:"shutdown_timeout_ms"`
	DeterministicReranker *deterministicWorkspaceRerankerWire `json:"deterministic_reranker,omitempty"`
	ChunkingStrategy      *workspaceChunkingStrategyWire      `json:"chunking_strategy,omitempty"`
}

func prepareWorkspaceRetrievalOptions(
	runtime Runtime,
	options *SessionOptions,
) (any, string, error) {
	if options == nil || options.WorkspaceRetrieval == nil {
		return options, "", nil
	}
	retrieval := options.WorkspaceRetrieval
	var chunkingStrategy *workspaceChunkingStrategyWire
	if retrieval.ChunkingStrategy != nil {
		value, err := retrieval.ChunkingStrategy.workspaceChunkingStrategyWire()
		if err != nil {
			return nil, "", err
		}
		chunkingStrategy = &value
	}
	var deterministicReranker *deterministicWorkspaceRerankerWire
	if retrieval.Reranker != nil {
		value, err := retrieval.Reranker.workspaceRerankerWire()
		if err != nil {
			return nil, "", err
		}
		deterministicReranker = &value
	}
	provider := retrieval.Provider
	if provider == nil {
		return nil, "", invalid("workspace_retrieval", "provider cannot be nil")
	}
	descriptor, err := safeProviderDescriptor(provider)
	if err != nil {
		return nil, "", err
	}
	providerTimeout := retrieval.ProviderTimeout
	if providerTimeout == 0 {
		providerTimeout = defaultEmbeddingProviderTimeout
	}
	shutdownTimeout := retrieval.ShutdownTimeout
	if shutdownTimeout == 0 {
		shutdownTimeout = defaultRetrievalShutdownTimeout
	}
	maxRecords := retrieval.MaxRecords
	if maxRecords == 0 {
		maxRecords = defaultRetrievalMaxRecords
	}
	maxBytes := retrieval.MaxBytes
	if maxBytes == 0 {
		maxBytes = defaultRetrievalMaxBytes
	}
	lexicalEngine := retrieval.LexicalEngine
	if lexicalEngine != "" && lexicalEngine != WorkspaceLexicalEnginePortable && lexicalEngine != WorkspaceLexicalEngineZvecRust {
		return nil, "", invalid("workspace_retrieval", "lexical engine must be a supported typed value")
	}
	if providerTimeout < time.Millisecond || providerTimeout > maxEmbeddingProviderTimeout {
		return nil, "", invalid(
			"workspace_retrieval",
			"provider timeout must be positive and no more than five minutes",
		)
	}
	if shutdownTimeout < time.Millisecond || shutdownTimeout > maxRetrievalShutdownTimeout {
		return nil, "", invalid(
			"workspace_retrieval",
			"shutdown timeout must be positive and no more than thirty seconds",
		)
	}
	callbacks, ok := runtime.(callbackRuntime)
	if !ok {
		return nil, "", sdkError(
			"workspace_retrieval",
			CodeUnavailable,
			"runtime does not support Go callbacks",
			nil,
		)
	}
	handlerID, err := callbacks.registerCallback(
		func(ctx context.Context, method string, payload json.RawMessage) (any, error) {
			if method != "embedding" {
				return nil, fmt.Errorf("unexpected embedding callback method %q", method)
			}
			var request EmbeddingBatchRequest
			if err := json.Unmarshal(payload, &request); err != nil {
				return nil, err
			}
			response, embedErr := safeProviderEmbed(ctx, provider, request)
			if embedErr != nil {
				return embeddingFailure(ctx, embedErr), nil
			}
			for _, vector := range response.Vectors {
				if !finiteVector(vector.Values) {
					return embeddingBatchFailure{
						Kind: EmbeddingFailureInvalidRequest,
					}, nil
				}
			}
			return response, nil
		},
	)
	if err != nil {
		return nil, "", err
	}
	encoded, err := json.Marshal(options)
	if err != nil {
		callbacks.unregisterCallback(handlerID)
		return nil, "", sdkError(
			"workspace_retrieval",
			CodeSerialization,
			"cannot encode session options",
			err,
		)
	}
	var wire map[string]any
	if err := json.Unmarshal(encoded, &wire); err != nil {
		callbacks.unregisterCallback(handlerID)
		return nil, "", sdkError(
			"workspace_retrieval",
			CodeSerialization,
			"cannot prepare session options",
			err,
		)
	}
	wire["workspace_retrieval"] = workspaceRetrievalWireOptions{
		HandlerID:             handlerID,
		Provider:              descriptor.Provider,
		Model:                 descriptor.Model,
		Revision:              descriptor.Revision,
		Dimension:             descriptor.Dimension,
		Normalization:         descriptor.Normalization,
		LexicalEngine:         lexicalEngine,
		ProviderTimeout:       uint64(providerTimeout / time.Millisecond),
		MaxRecords:            maxRecords,
		MaxBytes:              maxBytes,
		ShutdownTimeout:       uint64(shutdownTimeout / time.Millisecond),
		DeterministicReranker: deterministicReranker,
		ChunkingStrategy:      chunkingStrategy,
	}
	return wire, handlerID, nil
}

func prepareSessionOptions(runtime Runtime, value any) (any, string, error) {
	if value == nil {
		return nil, "", nil
	}
	options, ok := value.(*SessionOptions)
	if !ok {
		return value, "", nil
	}
	return prepareWorkspaceRetrievalOptions(runtime, options)
}

func (agent *Agent) trackRetrievalCallback(sessionID, callbackID string) {
	if agent == nil || callbackID == "" {
		return
	}
	agent.callbackMu.Lock()
	defer agent.callbackMu.Unlock()
	if agent.retrievalCallbacks == nil {
		agent.retrievalCallbacks = make(map[string]struct{})
	}
	agent.retrievalCallbacks[callbackID] = struct{}{}
	if sessionID == "" {
		return
	}
	if agent.sessionRetrievalCallbacks == nil {
		agent.sessionRetrievalCallbacks = make(map[string]map[string]struct{})
	}
	callbacks := agent.sessionRetrievalCallbacks[sessionID]
	if callbacks == nil {
		callbacks = make(map[string]struct{})
		agent.sessionRetrievalCallbacks[sessionID] = callbacks
	}
	callbacks[callbackID] = struct{}{}
}

func (agent *Agent) unregisterRetrievalCallback(callbackID string) {
	if agent == nil || callbackID == "" {
		return
	}
	if callbacks, ok := agent.runtime.(callbackRuntime); ok {
		callbacks.unregisterCallback(callbackID)
	}
}

func (agent *Agent) forgetRetrievalCallback(callbackID string) {
	if agent == nil || callbackID == "" {
		return
	}
	agent.callbackMu.Lock()
	delete(agent.retrievalCallbacks, callbackID)
	for sessionID, callbacks := range agent.sessionRetrievalCallbacks {
		delete(callbacks, callbackID)
		if len(callbacks) == 0 {
			delete(agent.sessionRetrievalCallbacks, sessionID)
		}
	}
	agent.callbackMu.Unlock()
}

func (agent *Agent) releaseRetrievalCallback(callbackID string) {
	agent.forgetRetrievalCallback(callbackID)
	agent.unregisterRetrievalCallback(callbackID)
}

func (agent *Agent) releaseSessionRetrievalCallbacks(sessionID string) {
	if agent == nil || sessionID == "" {
		return
	}
	agent.callbackMu.Lock()
	callbacks := agent.sessionRetrievalCallbacks[sessionID]
	delete(agent.sessionRetrievalCallbacks, sessionID)
	for callbackID := range callbacks {
		delete(agent.retrievalCallbacks, callbackID)
	}
	agent.callbackMu.Unlock()
	for callbackID := range callbacks {
		agent.unregisterRetrievalCallback(callbackID)
	}
}

func (agent *Agent) releaseAllRetrievalCallbacks() {
	if agent == nil {
		return
	}
	agent.callbackMu.Lock()
	callbacks := agent.retrievalCallbacks
	agent.retrievalCallbacks = nil
	agent.sessionRetrievalCallbacks = nil
	agent.callbackMu.Unlock()
	for callbackID := range callbacks {
		agent.unregisterRetrievalCallback(callbackID)
	}
}

func (handle *ServeHandle) releaseRetrievalCallback() {
	if handle == nil || handle.retrievalCallback == "" {
		return
	}
	callbackID := handle.retrievalCallback
	handle.retrievalCallback = ""
	if handle.owner != nil {
		handle.owner.releaseRetrievalCallback(callbackID)
	}
}

func safeProviderDescriptor(provider EmbeddingProvider) (
	descriptor EmbeddingProviderDescriptor,
	err error,
) {
	defer func() {
		if recovered := recover(); recovered != nil {
			err = invalid("workspace_retrieval", "embedding provider descriptor panicked")
		}
	}()
	descriptor = provider.Descriptor()
	if strings.TrimSpace(descriptor.Provider) == "" ||
		strings.TrimSpace(descriptor.Model) == "" ||
		descriptor.Dimension == 0 || descriptor.Dimension > 65_536 {
		return EmbeddingProviderDescriptor{}, invalid(
			"workspace_retrieval",
			"provider, model, and a dimension from 1 to 65536 are required",
		)
	}
	if descriptor.Normalization == "" {
		descriptor.Normalization = EmbeddingNormalizationNone
	}
	if descriptor.Normalization != EmbeddingNormalizationNone &&
		descriptor.Normalization != EmbeddingNormalizationUnit {
		return EmbeddingProviderDescriptor{}, invalid(
			"workspace_retrieval",
			"normalization must be none or unit",
		)
	}
	return descriptor, nil
}

func safeProviderEmbed(
	ctx context.Context,
	provider EmbeddingProvider,
	request EmbeddingBatchRequest,
) (response EmbeddingBatchResponse, err error) {
	defer func() {
		if recovered := recover(); recovered != nil {
			err = &EmbeddingError{
				Kind: EmbeddingFailureOther,
				Err:  errors.New("embedding provider panicked"),
			}
		}
	}()
	return provider.Embed(ctx, request)
}

func embeddingFailure(ctx context.Context, err error) embeddingBatchFailure {
	kind := EmbeddingFailureOther
	var retryAfter *uint64
	var providerError *EmbeddingError
	switch {
	case errors.Is(ctx.Err(), context.Canceled), errors.Is(err, context.Canceled):
		kind = EmbeddingFailureCancelled
	case errors.Is(ctx.Err(), context.DeadlineExceeded), errors.Is(err, context.DeadlineExceeded):
		kind = EmbeddingFailureTimeout
	case errors.As(err, &providerError):
		kind = providerError.Kind
		if kind == "" {
			kind = EmbeddingFailureOther
		}
		if providerError.RetryAfter > 0 {
			value := uint64(providerError.RetryAfter / time.Millisecond)
			retryAfter = &value
		}
	}
	return embeddingBatchFailure{Kind: kind, RetryAfterMS: retryAfter}
}
