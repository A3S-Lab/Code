package code

import (
	"context"
	"errors"
	"slices"
	"testing"
)

func TestCreateSessionAndCloseWithInjectedRuntime(t *testing.T) {
	runtime := &fakeRuntime{
		request: func(
			_ context.Context,
			operation string,
			params map[string]any,
		) (any, error) {
			switch operation {
			case "agent_create":
				if params["config_source"] != "inline acl" {
					t.Fatalf("unexpected config source: %#v", params)
				}
				return map[string]any{"agent_id": "agent-1"}, nil
			case "session_create":
				options, ok := params["options"].(*SessionOptions)
				if !ok || options.HostEnv == nil ||
					options.HostEnv.SequentialIDPrefix == nil ||
					*options.HostEnv.SequentialIDPrefix != "replay" {
					t.Fatalf("deterministic host env was not forwarded: %#v", params)
				}
				return map[string]any{
					"session_handle": "handle-1",
					"session_id":     "session-1",
					"workspace":      "C:/repo",
					"init_warning":   nil,
				}, nil
			case "session_send":
				return AgentResult{
					Text:  "done",
					Usage: TokenUsage{TotalTokens: 3},
				}, nil
			case "session_close", "agent_close":
				return map[string]any{}, nil
			default:
				t.Fatalf("unexpected operation %q", operation)
				return nil, nil
			}
		},
	}
	ctx := context.Background()
	agent, err := Create(ctx, "inline acl", WithRuntime(runtime))
	if err != nil {
		t.Fatal(err)
	}
	prefix := "replay"
	fixedTime := uint64(1_700_000_000_000)
	session, err := agent.Session(ctx, "C:/repo", &SessionOptions{
		SessionID: "session-1",
		HostEnv: &HostEnvConfig{
			SequentialIDPrefix: &prefix,
			FixedTimeMS:        &fixedTime,
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	result, err := session.Run(ctx, "finish the task")
	if err != nil {
		t.Fatal(err)
	}
	if result.Text != "done" || result.Usage.TotalTokens != 3 {
		t.Fatalf("unexpected result: %#v", result)
	}
	if err := session.Close(ctx); err != nil {
		t.Fatal(err)
	}
	if err := agent.Close(ctx); err != nil {
		t.Fatal(err)
	}
	if runtime.closed {
		t.Fatal("Agent must not close an injected runtime")
	}

	want := []string{
		"sdk_capabilities",
		"agent_create",
		"session_create",
		"session_send",
		"session_close",
		"agent_close",
	}
	if got := runtime.operations(); !slices.Equal(got, want) {
		t.Fatalf("operations = %v, want %v", got, want)
	}
}

func TestCreateFailsClosedOnCapabilityDrift(t *testing.T) {
	tests := []struct {
		name         string
		capabilities Capabilities
		want         error
	}{
		{
			name: "protocol",
			capabilities: Capabilities{
				ProtocolVersion:      99,
				EventProtocolVersion: 1,
				Operations:           SupportedOperations(),
			},
			want: ErrProtocol,
		},
		{
			name: "event protocol",
			capabilities: Capabilities{
				ProtocolVersion:      1,
				EventProtocolVersion: 99,
				Operations:           SupportedOperations(),
			},
			want: ErrProtocol,
		},
		{
			name: "missing operation",
			capabilities: Capabilities{
				ProtocolVersion:      1,
				EventProtocolVersion: 1,
				Operations:           SupportedOperations()[1:],
			},
			want: ErrUnavailable,
		},
		{
			name: "duplicate operation",
			capabilities: Capabilities{
				ProtocolVersion:      1,
				EventProtocolVersion: 1,
				Operations: append(
					SupportedOperations(),
					SupportedOperations()[0],
				),
			},
			want: ErrProtocol,
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			runtime := &fakeRuntime{capabilities: &test.capabilities}
			_, err := Create(
				context.Background(),
				"inline acl",
				WithRuntime(runtime),
			)
			if !errors.Is(err, test.want) {
				t.Fatalf("error = %v, want category %v", err, test.want)
			}
			if len(runtime.operations()) != 1 {
				t.Fatalf("mutation happened before handshake: %v", runtime.operations())
			}
		})
	}
}

func TestCreateValidatesInputs(t *testing.T) {
	if _, err := Create(nil, "acl"); !errors.Is(err, ErrInvalidRequest) {
		t.Fatalf("nil context error = %v", err)
	}
	if _, err := Create(context.Background(), " "); !errors.Is(err, ErrInvalidRequest) {
		t.Fatalf("empty source error = %v", err)
	}

	var nilAgent *Agent
	if err := nilAgent.Close(context.Background()); err != nil {
		t.Fatalf("nil Agent.Close error = %v", err)
	}
	if err := (&Agent{}).Close(context.Background()); !errors.Is(err, ErrInvalidRequest) {
		t.Fatalf("zero Agent.Close error = %v", err)
	}
}

func TestEventTypeCatalogReturnsACopy(t *testing.T) {
	first := AgentEventTypesV1()
	if len(first) == 0 {
		t.Fatal("event catalog is empty")
	}
	want := first[0]
	first[0] = "mutated"
	if got := AgentEventTypesV1()[0]; got != want {
		t.Fatalf("event catalog was mutable: got %q, want %q", got, want)
	}
}
