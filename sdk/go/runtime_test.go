package code

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	goruntime "runtime"
	"testing"
	"time"

	"github.com/A3S-Lab/Code/sdk/go/v6/internal/bridge"
)

func TestBridgeHelperProcess(t *testing.T) {
	if os.Getenv("A3S_CODE_GO_TEST_HELPER") != "1" {
		return
	}
	scanner := bufio.NewScanner(os.Stdin)
	encoder := json.NewEncoder(os.Stdout)
	var callbackRequestID uint64
	for scanner.Scan() {
		var request bridge.Request
		if json.Unmarshal(scanner.Bytes(), &request) != nil {
			os.Exit(2)
		}
		if os.Getenv("A3S_CODE_GO_TEST_MODE") == "invalid-json" {
			fmt.Fprintln(os.Stdout, "{")
			return
		}
		if request.Operation == "stream" {
			_ = encoder.Encode(map[string]any{
				"protocol_version": bridge.ProtocolVersion,
				"id":               request.ID,
				"kind":             "event",
				"ok":               true,
				"event": map[string]any{
					"version": 1,
					"type":    "future_event",
					"payload": map[string]any{"value": 7},
				},
			})
		}
		if request.Operation == "trigger_callback" {
			callbackRequestID = request.ID
			_ = encoder.Encode(map[string]any{
				"protocol_version": bridge.ProtocolVersion,
				"id":               91,
				"kind":             "callback",
				"ok":               true,
				"callback": map[string]any{
					"callback_id": 91,
					"handler_id":  request.Params["handler_id"],
					"method":      "test",
					"payload":     map[string]any{"value": 7},
					"timeout_ms":  5000,
				},
			})
			continue
		}
		if request.Operation == "trigger_cancel_callback" {
			_ = encoder.Encode(map[string]any{
				"protocol_version": bridge.ProtocolVersion,
				"id":               92,
				"kind":             "callback",
				"ok":               true,
				"callback": map[string]any{
					"callback_id": 92,
					"handler_id":  request.Params["handler_id"],
					"method":      "embedding",
					"payload":     map[string]any{},
					"timeout_ms":  5000,
				},
			})
			_ = encoder.Encode(map[string]any{
				"protocol_version": bridge.ProtocolVersion,
				"id":               92,
				"kind":             "callback_cancel",
				"ok":               true,
				"callback_cancel": map[string]any{
					"callback_id": 92,
				},
			})
			_ = encoder.Encode(map[string]any{
				"protocol_version": bridge.ProtocolVersion,
				"id":               request.ID,
				"kind":             "response",
				"ok":               true,
				"result":           map[string]any{"cancelled": true},
			})
			continue
		}
		if request.Operation == "callback_response" && callbackRequestID != 0 {
			_ = encoder.Encode(map[string]any{
				"protocol_version": bridge.ProtocolVersion,
				"id":               request.ID,
				"kind":             "response",
				"ok":               true,
				"result":           map[string]any{"accepted": true},
			})
			_ = encoder.Encode(map[string]any{
				"protocol_version": bridge.ProtocolVersion,
				"id":               callbackRequestID,
				"kind":             "response",
				"ok":               true,
				"result":           request.Params["result"],
			})
			callbackRequestID = 0
			continue
		}
		_ = encoder.Encode(map[string]any{
			"protocol_version": bridge.ProtocolVersion,
			"id":               request.ID,
			"kind":             "response",
			"ok":               true,
			"result": map[string]any{
				"operation": request.Operation,
			},
		})
	}
	os.Exit(0)
}

func TestLocalRuntimeMultiplexesCallbacks(t *testing.T) {
	runtime, err := NewLocalRuntime(
		context.Background(),
		WithBridgePath(os.Args[0]),
		withBridgeArguments("-test.run=TestBridgeHelperProcess"),
		WithBridgeEnvironment("A3S_CODE_GO_TEST_HELPER=1"),
	)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = runtime.Close() })

	handlerID, err := runtime.registerCallback(
		func(_ context.Context, method string, payload json.RawMessage) (any, error) {
			if method != "test" {
				t.Fatalf("method = %q", method)
			}
			var input struct {
				Value int `json:"value"`
			}
			if err := json.Unmarshal(payload, &input); err != nil {
				return nil, err
			}
			return map[string]any{"value": input.Value * 2}, nil
		},
	)
	if err != nil {
		t.Fatal(err)
	}
	var result struct {
		Value int `json:"value"`
	}
	if err := runtime.Request(
		context.Background(),
		"trigger_callback",
		map[string]any{"handler_id": handlerID},
		&result,
	); err != nil {
		t.Fatal(err)
	}
	if result.Value != 14 {
		t.Fatalf("callback result = %#v", result)
	}
}

func TestLocalRuntimeCancelsCallbackContext(t *testing.T) {
	runtime, err := NewLocalRuntime(
		context.Background(),
		WithBridgePath(os.Args[0]),
		withBridgeArguments("-test.run=TestBridgeHelperProcess"),
		WithBridgeEnvironment("A3S_CODE_GO_TEST_HELPER=1"),
	)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = runtime.Close() })

	cancelled := make(chan struct{})
	handlerID, err := runtime.registerCallback(
		func(ctx context.Context, method string, _ json.RawMessage) (any, error) {
			if method != "embedding" {
				t.Errorf("method = %q", method)
			}
			<-ctx.Done()
			close(cancelled)
			return nil, ctx.Err()
		},
	)
	if err != nil {
		t.Fatal(err)
	}
	var result struct {
		Cancelled bool `json:"cancelled"`
	}
	if err := runtime.Request(
		context.Background(),
		"trigger_cancel_callback",
		map[string]any{"handler_id": handlerID},
		&result,
	); err != nil {
		t.Fatal(err)
	}
	if !result.Cancelled {
		t.Fatal("helper did not acknowledge cancellation")
	}
	select {
	case <-cancelled:
	case <-time.After(time.Second):
		t.Fatal("callback context was not cancelled")
	}
}

func TestLocalRuntimeMultiplexesRequestAndStream(t *testing.T) {
	runtime, err := NewLocalRuntime(
		context.Background(),
		WithBridgePath(os.Args[0]),
		withBridgeArguments("-test.run=TestBridgeHelperProcess"),
		WithBridgeEnvironment("A3S_CODE_GO_TEST_HELPER=1"),
	)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := runtime.Close(); err != nil {
			t.Errorf("Close: %v", err)
		}
	})

	var response struct {
		Operation string `json:"operation"`
	}
	if err := runtime.Request(
		context.Background(),
		"echo",
		map[string]any{"value": 1},
		&response,
	); err != nil {
		t.Fatal(err)
	}
	if response.Operation != "echo" {
		t.Fatalf("response = %#v", response)
	}

	stream, err := runtime.Stream(context.Background(), "stream", nil)
	if err != nil {
		t.Fatal(err)
	}
	event := <-stream.Events
	if event.Type != "future_event" {
		t.Fatalf("event = %#v", event)
	}
	if _, open := <-stream.Events; open {
		t.Fatal("event stream should be closed")
	}
	if err := <-stream.Done; err != nil {
		t.Fatal(err)
	}
}

func TestLocalRuntimeFailsClosedOnProtocolError(t *testing.T) {
	runtime, err := NewLocalRuntime(
		context.Background(),
		WithBridgePath(os.Args[0]),
		withBridgeArguments("-test.run=TestBridgeHelperProcess"),
		WithBridgeEnvironment(
			"A3S_CODE_GO_TEST_HELPER=1",
			"A3S_CODE_GO_TEST_MODE=invalid-json",
		),
	)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = runtime.Close() })

	var response map[string]any
	err = runtime.Request(context.Background(), "echo", nil, &response)
	if !errors.Is(err, ErrProtocol) {
		t.Fatalf("invalid JSON error = %v", err)
	}
	err = runtime.Request(context.Background(), "echo", nil, &response)
	if !errors.Is(err, ErrBridgeClosed) {
		t.Fatalf("request after protocol failure = %v", err)
	}
}

func TestMergeEnvironmentUsesPlatformKeySemantics(t *testing.T) {
	merged := mergeEnvironment(
		[]string{"A3S_ONE=old", "A3S_Mixed=first", "=C:=C:\\workspace"},
		[]string{"A3S_ONE=new", "A3S_MIXED=second"},
	)
	entries := make(map[string]string, len(merged))
	for _, entry := range merged {
		key, ok := environmentKey(entry)
		if !ok {
			t.Fatalf("invalid merged environment entry %q", entry)
		}
		entries[key] = entry
	}
	if entries["A3S_ONE"] != "A3S_ONE=new" {
		t.Fatalf("same-case override was not applied: %#v", merged)
	}
	if entries["=C:"] != "=C:=C:\\workspace" {
		t.Fatalf("Windows drive entry was not preserved: %#v", merged)
	}
	if goruntime.GOOS == "windows" {
		if len(merged) != 3 || entries["A3S_MIXED"] != "A3S_MIXED=second" {
			t.Fatalf("Windows environment keys should be case-insensitive: %#v", merged)
		}
		return
	}
	if len(merged) != 4 ||
		entries["A3S_Mixed"] != "A3S_Mixed=first" ||
		entries["A3S_MIXED"] != "A3S_MIXED=second" {
		t.Fatalf("Unix environment keys should be case-sensitive: %#v", merged)
	}
}

func TestLocalRuntimeContextAndInstallationErrors(t *testing.T) {
	_, err := NewLocalRuntime(
		context.Background(),
		WithBridgePath("Z:/definitely-missing/a3s-code-go-bridge.exe"),
	)
	if !errors.Is(err, ErrNotInstalled) {
		t.Fatalf("missing binary error = %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	_, err = NewLocalRuntime(ctx, WithBridgePath(os.Args[0]))
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("canceled startup error = %v", err)
	}
}

func TestRustBridgeIntegration(t *testing.T) {
	binary := os.Getenv("A3S_CODE_GO_BRIDGE_TEST_BINARY")
	if binary == "" {
		t.Skip("set A3S_CODE_GO_BRIDGE_TEST_BINARY to run the Rust bridge integration")
	}
	ctx := context.Background()
	agent, err := Create(
		ctx,
		`
			default_model = "anthropic/test-model"
			providers "anthropic" {
				apiKey = "test-key"
				models "test-model" {
					name = "Test Model"
				}
			}
		`,
		WithLocalRuntimeOptions(WithBridgePath(binary)),
	)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := agent.Close(context.Background()); err != nil {
			t.Errorf("Agent.Close: %v", err)
		}
	})
	session, err := agent.Session(ctx, t.TempDir(), &SessionOptions{
		SessionID: "go-rust-integration",
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := session.WriteFile(ctx, "hello.txt", "hello from Go"); err != nil {
		t.Fatal(err)
	}
	content, err := session.ReadFile(ctx, "hello.txt", nil)
	if err != nil {
		t.Fatal(err)
	}
	if content == "" {
		t.Fatal("real bridge returned empty file content")
	}
	names, err := session.ToolNames(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if len(names) == 0 {
		t.Fatal("real bridge returned no tool names")
	}
	if err := session.RegisterCommand(
		ctx,
		"bridge-status",
		"Verify the Go callback transport",
		"/bridge-status <value>",
		func(_ context.Context, args string, commandContext CommandContext) (string, error) {
			return commandContext.SessionID + ":" + args, nil
		},
		time.Second,
	); err != nil {
		t.Fatal(err)
	}
	commandResult, err := session.Send(ctx, "/bridge-status ready", nil)
	if err != nil {
		t.Fatal(err)
	}
	if commandResult.Text != "go-rust-integration:ready" {
		t.Fatalf("callback command result = %q", commandResult.Text)
	}
	if err := session.Close(ctx); err != nil {
		t.Fatal(err)
	}
	closed, err := session.IsClosed(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if !closed {
		t.Fatal("real Rust session did not close")
	}
	fmt.Fprintln(os.Stderr, "Go/Rust bridge integration passed")
}
