package code

import (
	"context"
	"encoding/json"
	"errors"
	"testing"
	"time"
)

func TestSessionStreamPreservesUnknownEvents(t *testing.T) {
	runtime := &fakeRuntime{
		stream: func(
			_ context.Context,
			operation string,
			params map[string]any,
		) (*EventStream, error) {
			if operation != "session_stream" || params["prompt"] != "stream this" {
				t.Fatalf("unexpected stream request: %s %#v", operation, params)
			}
			events := make(chan Event, 2)
			done := make(chan error, 1)
			events <- Event{
				Version: 1,
				Type:    EventTextDelta,
				Payload: json.RawMessage(`{"text":"hello"}`),
			}
			events <- Event{
				Version: 1,
				Type:    "future_event",
				Payload: json.RawMessage(`{"new_field":42}`),
			}
			close(events)
			done <- nil
			close(done)
			return &EventStream{Events: events, Done: done}, nil
		},
	}
	stream, err := testSession(runtime).Stream(
		context.Background(),
		"stream this",
		nil,
	)
	if err != nil {
		t.Fatal(err)
	}
	var events []Event
	for event := range stream.Events {
		events = append(events, event)
	}
	if err := <-stream.Done; err != nil {
		t.Fatal(err)
	}
	if len(events) != 2 || events[1].Type != "future_event" {
		t.Fatalf("events were not preserved: %#v", events)
	}
	var payload struct {
		NewField int `json:"new_field"`
	}
	if err := events[1].DecodePayload(&payload); err != nil {
		t.Fatal(err)
	}
	if payload.NewField != 42 {
		t.Fatalf("future payload = %#v", payload)
	}
}

func TestSessionSendCancelsRemoteRunWhenContextEnds(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	runtime := &fakeRuntime{
		request: func(
			requestContext context.Context,
			operation string,
			_ map[string]any,
		) (any, error) {
			switch operation {
			case "session_send":
				cancel()
				<-requestContext.Done()
				return nil, contextError(operation, requestContext.Err())
			case "session_cancel":
				return map[string]any{"cancelled": true}, nil
			default:
				t.Fatalf("unexpected operation %q", operation)
				return nil, nil
			}
		},
	}
	_, err := testSession(runtime).Run(ctx, "cancel me")
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("error = %v, want context.Canceled", err)
	}
	operations := runtime.operations()
	if len(operations) != 2 || operations[1] != "session_cancel" {
		t.Fatalf("operations = %v, want send then cancel", operations)
	}
}

func TestDirectToolConveniencesUseStableOperations(t *testing.T) {
	runtime := &fakeRuntime{
		request: func(
			_ context.Context,
			operation string,
			params map[string]any,
		) (any, error) {
			switch operation {
			case "session_read_file":
				return map[string]any{"content": "hello"}, nil
			case "session_bash", "session_grep":
				return map[string]any{"output": "ok"}, nil
			case "session_glob":
				return map[string]any{"paths": []string{"a.go"}}, nil
			default:
				return ToolCallResult{Name: operation, Output: "ok"}, nil
			}
		},
	}
	session := testSession(runtime)
	ctx := context.Background()
	if content, err := session.ReadFile(ctx, "a.go", nil); err != nil || content != "hello" {
		t.Fatalf("ReadFile = %q, %v", content, err)
	}
	if _, err := session.WriteFile(ctx, "a.go", "package a"); err != nil {
		t.Fatal(err)
	}
	if _, err := session.Program(ctx, map[string]any{"script": "return 1"}); err != nil {
		t.Fatal(err)
	}
	if _, err := session.Task(ctx, DelegateTaskOptions{Description: "review"}); err != nil {
		t.Fatal(err)
	}
	if _, err := session.Tasks(ctx, []DelegateTaskOptions{{Description: "one"}}); err != nil {
		t.Fatal(err)
	}
	if _, err := session.Git(ctx, GitOptions{Command: "status"}); err != nil {
		t.Fatal(err)
	}
	if _, err := session.WebSearch(ctx, WebSearchOptions{Query: "A3S Code"}); err != nil {
		t.Fatal(err)
	}

	want := []string{
		"session_read_file",
		"session_write_file",
		"session_tool",
		"session_tool",
		"session_tool",
		"session_tool",
		"session_tool",
	}
	got := runtime.operations()
	if len(got) != len(want) {
		t.Fatalf("operations = %v, want %v", got, want)
	}
	for index := range want {
		if got[index] != want[index] {
			t.Fatalf("operations = %v, want %v", got, want)
		}
	}
}

func TestGovernedToolUsesGovernedBridgeOperation(t *testing.T) {
	runtime := &fakeRuntime{
		request: func(
			_ context.Context,
			operation string,
			params map[string]any,
		) (any, error) {
			if operation != "session_governed_tool" {
				t.Fatalf("operation = %q, want session_governed_tool", operation)
			}
			if params["name"] != "write" {
				t.Fatalf("tool name = %#v, want write", params["name"])
			}
			return ToolCallResult{Name: "write", Output: "denied", ExitCode: 1}, nil
		},
	}

	result, err := testSession(runtime).GovernedTool(
		context.Background(),
		"write",
		map[string]any{"file_path": "denied.txt"},
	)
	if err != nil {
		t.Fatal(err)
	}
	if result.ExitCode == 0 {
		t.Fatalf("GovernedTool result = %#v, want denied result", result)
	}
}

func TestSessionCloseValidatesBeforeConsumingClose(t *testing.T) {
	var nilSession *Session
	if err := nilSession.Close(context.Background()); err != nil {
		t.Fatalf("nil Session.Close error = %v", err)
	}
	if err := (&Session{}).Close(context.Background()); !errors.Is(err, ErrInvalidRequest) {
		t.Fatalf("zero Session.Close error = %v", err)
	}

	runtime := &fakeRuntime{
		request: func(
			_ context.Context,
			operation string,
			_ map[string]any,
		) (any, error) {
			if operation != "session_close" {
				t.Fatalf("unexpected operation %q", operation)
			}
			return map[string]any{}, nil
		},
	}
	session := testSession(runtime)
	cancelled, cancel := context.WithCancel(context.Background())
	cancel()
	if err := session.Close(cancelled); !errors.Is(err, context.Canceled) {
		t.Fatalf("canceled Session.Close error = %v", err)
	}
	if err := session.Close(context.Background()); err != nil {
		t.Fatalf("retry Session.Close error = %v", err)
	}
	if got := runtime.operations(); len(got) != 1 || got[0] != "session_close" {
		t.Fatalf("close operations = %v", got)
	}
}

func TestSessionCallbackAPIsRegisterAndReleaseHandlers(t *testing.T) {
	runtime := &fakeRuntime{
		request: func(
			_ context.Context,
			operation string,
			_ map[string]any,
		) (any, error) {
			switch operation {
			case "session_unregister_hook":
				return map[string]any{"removed": true}, nil
			case "session_list_commands":
				return map[string]any{"commands": []any{}}, nil
			default:
				return map[string]any{}, nil
			}
		},
	}
	session := testSession(runtime)
	ctx := context.Background()

	if err := session.RegisterHookWithHandler(ctx, Hook{
		ID:        "guard",
		EventType: "pre_tool_use",
	}, func(_ context.Context, event json.RawMessage) (*HookResponse, error) {
		var value map[string]any
		if err := json.Unmarshal(event, &value); err != nil {
			return nil, err
		}
		return &HookResponse{Action: "block", Reason: "denied"}, nil
	}); err != nil {
		t.Fatal(err)
	}

	if err := session.SetBudgetGuard(ctx, &BudgetGuardHandlers{
		CheckBeforeTool: func(
			_ context.Context,
			value BudgetToolContext,
		) (*BudgetDecision, error) {
			if value.ToolName != "bash" {
				t.Fatalf("tool = %q", value.ToolName)
			}
			return &BudgetDecision{
				Decision: "deny",
				Resource: "tools",
				Reason:   "limit reached",
			}, nil
		},
		Timeout: time.Second,
	}); err != nil {
		t.Fatal(err)
	}

	if err := session.RegisterCommand(
		ctx,
		"status",
		"Show status",
		"/status",
		func(_ context.Context, args string, commandContext CommandContext) (string, error) {
			return commandContext.SessionID + ":" + args, nil
		},
		time.Second,
	); err != nil {
		t.Fatal(err)
	}

	runtime.mu.Lock()
	if len(runtime.callbacks) != 3 {
		t.Fatalf("callbacks = %d, want 3", len(runtime.callbacks))
	}
	budgetID := session.budgetCallback
	commandID := session.commandCallbacks["status"]
	budgetCallback := runtime.callbacks[budgetID]
	commandCallback := runtime.callbacks[commandID]
	runtime.mu.Unlock()

	decision, err := budgetCallback(
		ctx,
		"check_before_tool",
		json.RawMessage(`{"session_id":"s","tool_name":"bash"}`),
	)
	if err != nil || decision.(*BudgetDecision).Decision != "deny" {
		t.Fatalf("budget callback = %#v, %v", decision, err)
	}
	command, err := commandCallback(
		ctx,
		"command",
		json.RawMessage(`{"args":"now","context":{"session_id":"s"}}`),
	)
	if err != nil || command != "s:now" {
		t.Fatalf("command callback = %#v, %v", command, err)
	}

	if removed, err := session.UnregisterHook(ctx, "guard"); err != nil || !removed {
		t.Fatalf("UnregisterHook = %v, %v", removed, err)
	}
	if err := session.SetBudgetGuard(ctx, nil); err != nil {
		t.Fatal(err)
	}
	if err := session.Close(ctx); err != nil {
		t.Fatal(err)
	}
	runtime.mu.Lock()
	defer runtime.mu.Unlock()
	if len(runtime.callbacks) != 0 {
		t.Fatalf("callbacks leaked after close: %#v", runtime.callbacks)
	}
}
