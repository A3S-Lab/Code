package code

import (
	"context"
	"encoding/json"
	"reflect"
	"strings"
	"sync"
	"sync/atomic"
)

// GraphObject is one materialized state-graph object.
type GraphObject struct {
	ID         string          `json:"id"`
	ObjectType string          `json:"type"`
	Data       json.RawMessage `json:"data"`
	Version    uint64          `json:"version"`
}

// GraphRelation is one materialized state-graph relation.
type GraphRelation struct {
	ID           string          `json:"id"`
	RelationType string          `json:"type"`
	Source       string          `json:"source"`
	Target       string          `json:"target"`
	Data         json.RawMessage `json:"data"`
	Version      uint64          `json:"version"`
}

// GraphProjection is the serializable state projection exposed by Core.
type GraphProjection struct {
	Version   uint64                   `json:"version"`
	Objects   map[string]GraphObject   `json:"objects"`
	Relations map[string]GraphRelation `json:"relations"`
}

// PatchOperation is a JSON object with the Core operation discriminator `op`.
// The map keeps the SDK forward-compatible when Core adds a validated
// operation field; helper constructors below cover the common mutations.
type PatchOperation map[string]any

func AddObjectOperation(id, objectType string, data any) PatchOperation {
	return PatchOperation{"op": "add_object", "id": id, "object_type": objectType, "data": data}
}

func UpdateObjectOperation(id string, expectedVersion uint64, data any) PatchOperation {
	return PatchOperation{"op": "update_object", "id": id, "expected_version": expectedVersion, "data": data}
}

func RemoveObjectOperation(id string, expectedVersion uint64) PatchOperation {
	return PatchOperation{"op": "remove_object", "id": id, "expected_version": expectedVersion}
}

func AddRelationOperation(id, relationType, source, target string, data any) PatchOperation {
	return PatchOperation{
		"op": "add_relation", "id": id, "relation_type": relationType,
		"source": source, "target": target, "data": data,
	}
}

func UpdateRelationOperation(id string, expectedVersion uint64, data any) PatchOperation {
	return PatchOperation{"op": "update_relation", "id": id, "expected_version": expectedVersion, "data": data}
}

func RemoveRelationOperation(id string, expectedVersion uint64) PatchOperation {
	return PatchOperation{"op": "remove_relation", "id": id, "expected_version": expectedVersion}
}

// GraphPatch is an optimistic, version-checked graph mutation.
type GraphPatch struct {
	ExpectedGraphVersion uint64           `json:"expected_graph_version"`
	Operations           []PatchOperation `json:"operations"`
}

// GraphEventRecord is an append-only, hash-linked graph event. Event is kept
// open as JSON so future Core event variants remain consumable by old SDKs.
type GraphEventRecord struct {
	SchemaVersion      uint32          `json:"schema_version"`
	ID                 string          `json:"id"`
	Sequence           uint64          `json:"sequence"`
	TimestampMS        uint64          `json:"timestamp_ms"`
	BranchID           string          `json:"branch_id"`
	CausationID        *string         `json:"causation_id,omitempty"`
	CorrelationID      *string         `json:"correlation_id,omitempty"`
	StateVersionBefore uint64          `json:"state_version_before"`
	StateVersionAfter  uint64          `json:"state_version_after"`
	StateHashAfter     string          `json:"state_hash_after"`
	PreviousRecordHash *string         `json:"previous_record_hash,omitempty"`
	RecordHash         string          `json:"record_hash"`
	Event              json.RawMessage `json:"event"`
}

// GraphDiff describes the deterministic projection difference between two
// graph runtimes.
type GraphDiff struct {
	ObjectsAdded     []GraphObject      `json:"objects_added"`
	ObjectsRemoved   []GraphObject      `json:"objects_removed"`
	ObjectsChanged   [][2]GraphObject   `json:"objects_changed"`
	RelationsAdded   []GraphRelation    `json:"relations_added"`
	RelationsRemoved []GraphRelation    `json:"relations_removed"`
	RelationsChanged [][2]GraphRelation `json:"relations_changed"`
}

// ExternalGraphEvent is an ordered event from a host-owned stream. The
// runtime validates sequence continuity and event-id idempotency before a
// patch is committed.
type ExternalGraphEvent struct {
	Source   string `json:"source"`
	StreamID string `json:"stream_id"`
	Sequence uint64 `json:"sequence"`
	EventID  string `json:"event_id"`
	Name     string `json:"name"`
	Payload  any    `json:"payload"`
}

// ExternalProjectionOutcome is the result of checking or projecting a host
// event. Duplicate means the exact event was already durably observed.
type ExternalProjectionOutcome string

const (
	ExternalProjectionApplied   ExternalProjectionOutcome = "applied"
	ExternalProjectionDuplicate ExternalProjectionOutcome = "duplicate"
)

// StateGraphLimits bounds event retention and reactive behavior depth.
// Omitted values use Core's release defaults.
type StateGraphLimits struct {
	MaxEvents        *uint `json:"max_events,omitempty"`
	MaxBehaviorDepth *uint `json:"max_behavior_depth,omitempty"`
}

type StateGraphOption interface {
	applyStateGraph(*stateGraphConfig)
}

type stateGraphOptionFunc func(*stateGraphConfig)

func (option stateGraphOptionFunc) applyStateGraph(config *stateGraphConfig) { option(config) }

type stateGraphConfig struct {
	runtime             Runtime
	localRuntimeOptions []LocalRuntimeOption
	correlationID       string
	limits              *StateGraphLimits
}

// WithStateGraphRuntime injects an existing runtime. Its lifecycle remains
// owned by the caller.
func WithStateGraphRuntime(runtime Runtime) StateGraphOption {
	return stateGraphOptionFunc(func(config *stateGraphConfig) { config.runtime = runtime })
}

// WithStateGraphLocalRuntimeOptions configures an automatically-created
// bridge for a standalone graph runtime.
func WithStateGraphLocalRuntimeOptions(options ...LocalRuntimeOption) StateGraphOption {
	values := append([]LocalRuntimeOption(nil), options...)
	return stateGraphOptionFunc(func(config *stateGraphConfig) {
		config.localRuntimeOptions = append(config.localRuntimeOptions, values...)
	})
}

// WithStateGraphCorrelationID associates newly-created graph events with a
// host-provided correlation id. The id is preserved across forks and restore
// operations (restore uses the id carried by the event log).
func WithStateGraphCorrelationID(correlationID string) StateGraphOption {
	return stateGraphOptionFunc(func(config *stateGraphConfig) {
		config.correlationID = strings.TrimSpace(correlationID)
	})
}

// WithStateGraphLimits applies explicit Core event/behavior limits to a new
// standalone graph. Restore keeps the limits embedded in the Core defaults,
// matching the Rust runtime's restore contract.
func WithStateGraphLimits(limits StateGraphLimits) StateGraphOption {
	return stateGraphOptionFunc(func(config *stateGraphConfig) {
		copy := limits
		config.limits = &copy
	})
}

// StateGraphRuntime is a concurrency-safe handle to Core's event-sourced
// state graph. Calls are serialized by Core for one handle; independent graph
// handles can be used concurrently.
type StateGraphRuntime struct {
	runtime   *stateGraphRuntimeTransport
	handle    string
	closed    atomic.Bool
	closeOnce sync.Once
	closeErr  error
}

// stateGraphRuntimeTransport keeps a standalone bridge alive while any graph
// fork still references it. A parent graph may therefore be closed before its
// fork without invalidating the fork's transport.
type stateGraphRuntimeTransport struct {
	runtime Runtime
	owned   bool
	mu      sync.Mutex
	refs    uint32
}

func newStateGraphRuntimeTransport(runtime Runtime, owned bool) *stateGraphRuntimeTransport {
	return &stateGraphRuntimeTransport{runtime: runtime, owned: owned, refs: 1}
}

func (transport *stateGraphRuntimeTransport) acquire() bool {
	transport.mu.Lock()
	defer transport.mu.Unlock()
	if transport.refs == 0 {
		return false
	}
	transport.refs++
	return true
}

func (transport *stateGraphRuntimeTransport) release() error {
	transport.mu.Lock()
	if transport.refs == 0 {
		transport.mu.Unlock()
		return nil
	}
	transport.refs--
	last := transport.refs == 0
	runtime := transport.runtime
	transport.mu.Unlock()
	if last && transport.owned && runtime != nil {
		return runtime.Close()
	}
	return nil
}

func (transport *stateGraphRuntimeTransport) request(
	ctx context.Context,
	operation string,
	params map[string]any,
	result any,
) error {
	if transport == nil || transport.runtime == nil {
		return invalid(operation, "state graph runtime is not initialized")
	}
	return transport.runtime.Request(ctx, operation, params, result)
}

// NewStateGraphRuntime creates an empty graph.
func NewStateGraphRuntime(ctx context.Context, options ...StateGraphOption) (*StateGraphRuntime, error) {
	transport, err := openStateGraphRuntime(ctx, options...)
	if err != nil {
		return nil, err
	}
	var result struct {
		Handle string `json:"graph_handle"`
	}
	params := map[string]any{}
	config := stateGraphOptions(options)
	if config.correlationID != "" {
		params["correlation_id"] = config.correlationID
	}
	if config.limits != nil {
		if config.limits.MaxEvents != nil {
			params["max_events"] = *config.limits.MaxEvents
		}
		if config.limits.MaxBehaviorDepth != nil {
			params["max_behavior_depth"] = *config.limits.MaxBehaviorDepth
		}
	}
	if err := transport.request(ctx, "state_graph_create", params, &result); err != nil {
		if transport != nil {
			_ = transport.release()
		}
		return nil, err
	}
	if result.Handle == "" {
		_ = transport.release()
		return nil, sdkError("state_graph_create", CodeProtocol, "bridge returned an empty graph handle", nil)
	}
	return &StateGraphRuntime{runtime: transport, handle: result.Handle}, nil
}

// stateGraphOptions applies options once for request-specific values while
// keeping openStateGraphRuntime responsible for runtime selection.
func stateGraphOptions(options []StateGraphOption) stateGraphConfig {
	config := stateGraphConfig{}
	for _, option := range options {
		if option != nil {
			option.applyStateGraph(&config)
		}
	}
	return config
}

// RestoreStateGraphRuntime replays a previously persisted event log.
func RestoreStateGraphRuntime(ctx context.Context, events []GraphEventRecord, options ...StateGraphOption) (*StateGraphRuntime, error) {
	transport, err := openStateGraphRuntime(ctx, options...)
	if err != nil {
		return nil, err
	}
	var result struct {
		Handle string `json:"graph_handle"`
	}
	if err := transport.request(ctx, "state_graph_restore", map[string]any{"events": events}, &result); err != nil {
		_ = transport.release()
		return nil, err
	}
	if result.Handle == "" {
		_ = transport.release()
		return nil, sdkError("state_graph_restore", CodeProtocol, "bridge returned an empty graph handle", nil)
	}
	return &StateGraphRuntime{runtime: transport, handle: result.Handle}, nil
}

// StrictReplay validates a persisted event log and returns its deterministic
// projection without allocating a graph handle that callers must close.
func StrictReplay(ctx context.Context, events []GraphEventRecord, options ...LocalRuntimeOption) (GraphProjection, error) {
	const op = "state_graph_strict_replay"
	if ctx == nil {
		return GraphProjection{}, invalid(op, "context cannot be nil")
	}
	runtime, err := NewLocalRuntime(ctx, options...)
	if err != nil {
		return GraphProjection{}, err
	}
	defer runtime.Close()
	var result GraphProjection
	if err := runtime.Request(ctx, op, map[string]any{"events": events}, &result); err != nil {
		return GraphProjection{}, err
	}
	return result, nil
}

// StateGraph creates a graph handle on an existing Agent bridge.
func (agent *Agent) StateGraph(ctx context.Context) (*StateGraphRuntime, error) {
	const op = "state_graph_create"
	if err := validateAgent(agent, ctx, op); err != nil {
		return nil, err
	}
	var result struct {
		Handle string `json:"graph_handle"`
	}
	if err := agent.runtime.Request(ctx, op, nil, &result); err != nil {
		return nil, err
	}
	if result.Handle == "" {
		return nil, sdkError(op, CodeProtocol, "bridge returned an empty graph handle", nil)
	}
	return &StateGraphRuntime{
		runtime: newStateGraphRuntimeTransport(agent.runtime, false),
		handle:  result.Handle,
	}, nil
}

func openStateGraphRuntime(ctx context.Context, options ...StateGraphOption) (*stateGraphRuntimeTransport, error) {
	const op = "state_graph_runtime"
	if ctx == nil {
		return nil, invalid(op, "context cannot be nil")
	}
	if err := ctx.Err(); err != nil {
		return nil, contextError(op, err)
	}
	config := stateGraphOptions(options)
	runtime := config.runtime
	if runtime == nil {
		local, err := NewLocalRuntime(ctx, config.localRuntimeOptions...)
		if err != nil {
			return nil, err
		}
		runtime = local
	}
	if _, err := handshake(ctx, runtime); err != nil {
		if config.runtime == nil {
			_ = runtime.Close()
		}
		return nil, err
	}
	return newStateGraphRuntimeTransport(runtime, config.runtime == nil), nil
}

func (graph *StateGraphRuntime) validate(ctx context.Context, operation string) error {
	if graph == nil || graph.runtime == nil || graph.runtime.runtime == nil || graph.handle == "" {
		return invalid(operation, "state graph runtime is not initialized")
	}
	if graph.closed.Load() {
		return sdkError(operation, CodeStateGraphClosed, "state graph runtime is closed", nil)
	}
	if ctx == nil {
		return invalid(operation, "context cannot be nil")
	}
	if err := ctx.Err(); err != nil {
		return contextError(operation, err)
	}
	return nil
}

func (graph *StateGraphRuntime) BranchID(ctx context.Context) (string, error) {
	info, err := graph.Info(ctx)
	return info.BranchID, err
}

func (graph *StateGraphRuntime) Version(ctx context.Context) (uint64, error) {
	info, err := graph.Info(ctx)
	return info.Version, err
}

type StateGraphInfo struct {
	BranchID   string `json:"branch_id"`
	Version    uint64 `json:"version"`
	EventCount uint64 `json:"event_count"`
}

func (graph *StateGraphRuntime) Info(ctx context.Context) (StateGraphInfo, error) {
	const op = "state_graph_info"
	if err := graph.validate(ctx, op); err != nil {
		return StateGraphInfo{}, err
	}
	var result StateGraphInfo
	err := graph.runtime.request(ctx, op, map[string]any{"graph_handle": graph.handle}, &result)
	return result, err
}

func (graph *StateGraphRuntime) ProposePatch(ctx context.Context, patch GraphPatch, causationID ...string) (bool, error) {
	const op = "state_graph_propose_patch"
	if err := graph.validate(ctx, op); err != nil {
		return false, err
	}
	if patch.Operations == nil {
		return false, invalid(op, "patch operations cannot be nil")
	}
	params := map[string]any{"graph_handle": graph.handle, "patch": patch}
	if len(causationID) > 0 && strings.TrimSpace(causationID[0]) != "" {
		params["causation_id"] = causationID[0]
	}
	var result struct {
		Applied bool `json:"applied"`
	}
	if err := graph.runtime.request(ctx, op, params, &result); err != nil {
		return false, err
	}
	return result.Applied, nil
}

func (graph *StateGraphRuntime) RunGoal(ctx context.Context, goal string) (GraphEventRecord, error) {
	const op = "state_graph_run_goal"
	if err := graph.validate(ctx, op); err != nil {
		return GraphEventRecord{}, err
	}
	if strings.TrimSpace(goal) == "" {
		return GraphEventRecord{}, invalid(op, "goal cannot be empty")
	}
	return graph.eventRequest(ctx, op, map[string]any{"goal": goal})
}

func (graph *StateGraphRuntime) EmitCustom(ctx context.Context, name string, payload any) (GraphEventRecord, error) {
	const op = "state_graph_emit_custom"
	if err := graph.validate(ctx, op); err != nil {
		return GraphEventRecord{}, err
	}
	if strings.TrimSpace(name) == "" {
		return GraphEventRecord{}, invalid(op, "event name cannot be empty")
	}
	return graph.eventRequest(ctx, op, map[string]any{"name": name, "payload": payload})
}

func (graph *StateGraphRuntime) eventRequest(ctx context.Context, operation string, extra map[string]any) (GraphEventRecord, error) {
	params := map[string]any{"graph_handle": graph.handle}
	for key, value := range extra {
		params[key] = value
	}
	var result GraphEventRecord
	if err := graph.runtime.request(ctx, operation, params, &result); err != nil {
		return GraphEventRecord{}, err
	}
	return result, nil
}

func (graph *StateGraphRuntime) Graph(ctx context.Context) (GraphProjection, error) {
	const op = "state_graph_graph"
	if err := graph.validate(ctx, op); err != nil {
		return GraphProjection{}, err
	}
	var result GraphProjection
	err := graph.runtime.request(ctx, op, map[string]any{"graph_handle": graph.handle}, &result)
	return result, err
}

func (graph *StateGraphRuntime) Events(ctx context.Context) ([]GraphEventRecord, error) {
	const op = "state_graph_events"
	if err := graph.validate(ctx, op); err != nil {
		return nil, err
	}
	var result []GraphEventRecord
	if err := graph.runtime.request(ctx, op, map[string]any{"graph_handle": graph.handle}, &result); err != nil {
		return nil, err
	}
	return result, nil
}

// CheckExternal validates an ordered host event without mutating the graph.
// A nil outcome means the event is the next expected sequence number.
func (graph *StateGraphRuntime) CheckExternal(
	ctx context.Context,
	event ExternalGraphEvent,
) (*ExternalProjectionOutcome, error) {
	const op = "state_graph_check_external"
	if err := graph.validate(ctx, op); err != nil {
		return nil, err
	}
	var result struct {
		Outcome *ExternalProjectionOutcome `json:"outcome"`
	}
	if err := graph.runtime.request(ctx, op, map[string]any{
		"graph_handle": graph.handle,
		"event":        event,
	}, &result); err != nil {
		return nil, err
	}
	return result.Outcome, nil
}

// ProjectExternal atomically records a host event and applies its graph patch.
func (graph *StateGraphRuntime) ProjectExternal(
	ctx context.Context,
	event ExternalGraphEvent,
	patch GraphPatch,
) (ExternalProjectionOutcome, error) {
	const op = "state_graph_project_external"
	if err := graph.validate(ctx, op); err != nil {
		return "", err
	}
	if patch.Operations == nil {
		return "", invalid(op, "patch operations cannot be nil")
	}
	var result struct {
		Outcome ExternalProjectionOutcome `json:"outcome"`
	}
	if err := graph.runtime.request(ctx, op, map[string]any{
		"graph_handle": graph.handle,
		"event":        event,
		"patch":        patch,
	}, &result); err != nil {
		return "", err
	}
	if result.Outcome != ExternalProjectionApplied && result.Outcome != ExternalProjectionDuplicate {
		return "", sdkError(op, CodeProtocol, "bridge returned an unknown external projection outcome", nil)
	}
	return result.Outcome, nil
}

func (graph *StateGraphRuntime) ForkAt(ctx context.Context, sequenceExclusive uint64) (*StateGraphRuntime, error) {
	const op = "state_graph_fork"
	if err := graph.validate(ctx, op); err != nil {
		return nil, err
	}
	var result struct {
		Handle string `json:"graph_handle"`
	}
	if err := graph.runtime.request(ctx, op, map[string]any{"graph_handle": graph.handle, "sequence_exclusive": sequenceExclusive}, &result); err != nil {
		return nil, err
	}
	if result.Handle == "" {
		return nil, sdkError(op, CodeProtocol, "bridge returned an empty fork handle", nil)
	}
	if !graph.runtime.acquire() {
		// The bridge fork was created before the transport reference could be
		// acquired. Close that remote handle on the losing race so a concurrent
		// parent Close cannot leak a graph in the bridge process.
		_ = graph.runtime.request(ctx, opClose, map[string]any{"graph_handle": result.Handle}, nil)
		return nil, sdkError(op, CodeStateGraphClosed, "state graph runtime transport is closed", nil)
	}
	return &StateGraphRuntime{runtime: graph.runtime, handle: result.Handle}, nil
}

func (graph *StateGraphRuntime) Diff(ctx context.Context, other *StateGraphRuntime) (GraphDiff, error) {
	const op = "state_graph_diff"
	if err := graph.validate(ctx, op); err != nil {
		return GraphDiff{}, err
	}
	if other == nil || other.runtime == nil || other.runtime.runtime == nil || other.handle == "" {
		return GraphDiff{}, invalid(op, "other state graph runtime is not initialized")
	}
	if !sameStateGraphRuntime(graph.runtime, other.runtime) {
		return GraphDiff{}, invalid(op, "both graphs must use the same runtime")
	}
	if graph.handle == other.handle {
		return GraphDiff{}, nil
	}
	var result GraphDiff
	err := graph.runtime.request(ctx, op, map[string]any{
		"left_graph_handle": graph.handle, "right_graph_handle": other.handle,
	}, &result)
	return result, err
}

const opClose = "state_graph_close"

// Close releases the remote graph handle and closes a standalone bridge.
func (graph *StateGraphRuntime) Close(ctx context.Context) error {
	const op = "state_graph_close"
	if graph == nil {
		return nil
	}
	if graph.runtime == nil || graph.runtime.runtime == nil || graph.handle == "" {
		return nil
	}
	if ctx == nil {
		return invalid(op, "context cannot be nil")
	}
	graph.closeOnce.Do(func() {
		remoteErr := graph.runtime.request(ctx, op, map[string]any{"graph_handle": graph.handle}, nil)
		runtimeErr := graph.runtime.release()
		graph.closeErr = errorsJoin(remoteErr, runtimeErr)
		graph.closed.Store(true)
	})
	return graph.closeErr
}

func sameStateGraphRuntime(left, right *stateGraphRuntimeTransport) bool {
	if left == right {
		return true
	}
	if left == nil || right == nil || left.runtime == nil || right.runtime == nil {
		return false
	}
	leftValue := reflect.ValueOf(left.runtime)
	rightValue := reflect.ValueOf(right.runtime)
	if !leftValue.IsValid() || !rightValue.IsValid() || !leftValue.Type().AssignableTo(rightValue.Type()) {
		return false
	}
	if !leftValue.Type().Comparable() {
		return false
	}
	return leftValue.Interface() == rightValue.Interface()
}

// errorsJoin is kept local so this SDK remains source-compatible with Go
// versions that predate errors.Join in downstream build systems.
func errorsJoin(first, second error) error {
	if first == nil {
		return second
	}
	if second == nil {
		return first
	}
	return &joinedError{first: first, second: second}
}

type joinedError struct{ first, second error }

func (err *joinedError) Error() string { return err.first.Error() + "; " + err.second.Error() }
func (err *joinedError) Unwrap() error { return err.first }
