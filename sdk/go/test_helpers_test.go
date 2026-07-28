package code

import (
	"context"
	"encoding/json"
	"sync"
)

type recordedRequest struct {
	operation string
	params    map[string]any
}

type fakeRuntime struct {
	mu           sync.Mutex
	requests     []recordedRequest
	request      func(context.Context, string, map[string]any) (any, error)
	stream       func(context.Context, string, map[string]any) (*EventStream, error)
	closed       bool
	capabilities *Capabilities
}

func (runtime *fakeRuntime) Request(
	ctx context.Context,
	operation string,
	params map[string]any,
	result any,
) error {
	runtime.mu.Lock()
	runtime.requests = append(runtime.requests, recordedRequest{
		operation: operation,
		params:    cloneMap(params),
	})
	runtime.mu.Unlock()

	var value any
	var err error
	if operation == "sdk_capabilities" {
		if runtime.capabilities != nil {
			value = *runtime.capabilities
		} else {
			value = Capabilities{
				ProtocolVersion:      1,
				EventProtocolVersion: 1,
				Operations:           SupportedOperations(),
			}
		}
	} else if runtime.request != nil {
		value, err = runtime.request(ctx, operation, params)
	}
	if err != nil || result == nil {
		return err
	}
	encoded, err := json.Marshal(value)
	if err != nil {
		return err
	}
	return json.Unmarshal(encoded, result)
}

func (runtime *fakeRuntime) Stream(
	ctx context.Context,
	operation string,
	params map[string]any,
) (*EventStream, error) {
	runtime.mu.Lock()
	runtime.requests = append(runtime.requests, recordedRequest{
		operation: operation,
		params:    cloneMap(params),
	})
	runtime.mu.Unlock()
	if runtime.stream != nil {
		return runtime.stream(ctx, operation, params)
	}
	events := make(chan Event)
	done := make(chan error, 1)
	close(events)
	done <- nil
	close(done)
	return &EventStream{Events: events, Done: done}, nil
}

func (runtime *fakeRuntime) Close() error {
	runtime.mu.Lock()
	defer runtime.mu.Unlock()
	runtime.closed = true
	return nil
}

func (runtime *fakeRuntime) operations() []string {
	runtime.mu.Lock()
	defer runtime.mu.Unlock()
	operations := make([]string, len(runtime.requests))
	for index, request := range runtime.requests {
		operations[index] = request.operation
	}
	return operations
}

func cloneMap(input map[string]any) map[string]any {
	if input == nil {
		return nil
	}
	output := make(map[string]any, len(input))
	for key, value := range input {
		output[key] = value
	}
	return output
}

func testSession(runtime Runtime) *Session {
	return &Session{
		runtime:   runtime,
		handle:    "session-handle",
		id:        "session-id",
		workspace: "C:/workspace",
	}
}
