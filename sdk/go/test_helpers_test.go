package code

import (
	"context"
	"encoding/json"
	"fmt"
	"sync"

	"github.com/A3S-Lab/Code/sdk/go/v7/internal/bridge"
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
	callbacks    map[string]callbackHandler
	nextCallback uint64
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
				ProtocolVersion:      bridge.ProtocolVersion,
				EventProtocolVersion: bridge.EventProtocolVersion,
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

func (runtime *fakeRuntime) registerCallback(handler callbackHandler) (string, error) {
	runtime.mu.Lock()
	defer runtime.mu.Unlock()
	runtime.nextCallback++
	id := fmt.Sprintf("fake-callback-%d", runtime.nextCallback)
	if runtime.callbacks == nil {
		runtime.callbacks = make(map[string]callbackHandler)
	}
	runtime.callbacks[id] = handler
	return id, nil
}

func (runtime *fakeRuntime) unregisterCallback(id string) {
	runtime.mu.Lock()
	delete(runtime.callbacks, id)
	runtime.mu.Unlock()
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
