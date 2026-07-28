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

	"github.com/A3S-Lab/Code/sdk/go/v6/internal/bridge"
)

func TestBridgeHelperProcess(t *testing.T) {
	if os.Getenv("A3S_CODE_GO_TEST_HELPER") != "1" {
		return
	}
	scanner := bufio.NewScanner(os.Stdin)
	encoder := json.NewEncoder(os.Stdout)
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
