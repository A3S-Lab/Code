package code

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"
)

// RegisterHook registers a lifecycle hook. Hooks without a handler remain
// observable through the event stream and always continue.
func (session *Session) RegisterHook(ctx context.Context, hook Hook) error {
	return session.registerHook(ctx, hook, nil)
}

// RegisterHookWithHandler registers a lifecycle hook whose decision is made by
// a Go callback. Gating callback failures fail closed in the native core.
func (session *Session) RegisterHookWithHandler(
	ctx context.Context,
	hook Hook,
	handler HookHandler,
) error {
	if handler == nil {
		return invalid("session_register_hook", "hook handler cannot be nil")
	}
	return session.registerHook(ctx, hook, handler)
}

func (session *Session) registerHook(
	ctx context.Context,
	hook Hook,
	handler HookHandler,
) error {
	const op = "session_register_hook"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	if strings.TrimSpace(hook.ID) == "" || strings.TrimSpace(hook.EventType) == "" {
		return invalid(op, "hook id and event type cannot be empty")
	}
	params := session.params()
	params["hook"] = hook
	var callbackID string
	callbacks, supportsCallbacks := session.runtime.(callbackRuntime)
	if handler != nil {
		if !supportsCallbacks {
			return sdkError(op, CodeUnavailable, "runtime does not support Go callbacks", nil)
		}
		var err error
		callbackID, err = callbacks.registerCallback(
			func(callbackCtx context.Context, method string, payload json.RawMessage) (any, error) {
				if method != "hook" {
					return nil, fmt.Errorf("unexpected hook callback method %q", method)
				}
				return handler(callbackCtx, payload)
			},
		)
		if err != nil {
			return err
		}
		params["handler_id"] = callbackID
	}
	if err := session.runtime.Request(ctx, op, params, nil); err != nil {
		if callbackID != "" {
			callbacks.unregisterCallback(callbackID)
		}
		return err
	}

	session.callbackMu.Lock()
	if session.hookCallbacks == nil {
		session.hookCallbacks = make(map[string]string)
	}
	previous := session.hookCallbacks[hook.ID]
	if callbackID == "" {
		delete(session.hookCallbacks, hook.ID)
	} else {
		session.hookCallbacks[hook.ID] = callbackID
	}
	session.callbackMu.Unlock()
	if previous != "" && supportsCallbacks {
		callbacks.unregisterCallback(previous)
	}
	return nil
}

func (session *Session) UnregisterHook(
	ctx context.Context,
	hookID string,
) (bool, error) {
	const op = "session_unregister_hook"
	if err := validateSession(session, ctx, op); err != nil {
		return false, err
	}
	if strings.TrimSpace(hookID) == "" {
		return false, invalid(op, "hook id cannot be empty")
	}
	params := session.params()
	params["hook_id"] = hookID
	var result struct {
		Removed bool `json:"removed"`
	}
	err := session.runtime.Request(ctx, op, params, &result)
	if err == nil {
		session.callbackMu.Lock()
		callbackID := session.hookCallbacks[hookID]
		delete(session.hookCallbacks, hookID)
		session.callbackMu.Unlock()
		if callbacks, ok := session.runtime.(callbackRuntime); ok {
			callbacks.unregisterCallback(callbackID)
		}
	}
	return result.Removed, err
}

func (session *Session) HookCount(ctx context.Context) (uint, error) {
	const op = "session_hook_count"
	if err := validateSession(session, ctx, op); err != nil {
		return 0, err
	}
	var result struct {
		Count uint `json:"count"`
	}
	err := session.runtime.Request(ctx, op, session.params(), &result)
	return result.Count, err
}

func (session *Session) ListCommands(ctx context.Context) ([]CommandInfo, error) {
	const op = "session_list_commands"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result struct {
		Commands []CommandInfo `json:"commands"`
	}
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return result.Commands, nil
}

func (session *Session) SetBudgetGuard(
	ctx context.Context,
	handlers *BudgetGuardHandlers,
) error {
	const op = "session_set_budget_guard"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	params := session.params()
	callbacks, supportsCallbacks := session.runtime.(callbackRuntime)
	var callbackID string
	if handlers != nil {
		if !supportsCallbacks {
			return sdkError(op, CodeUnavailable, "runtime does not support Go callbacks", nil)
		}
		timeout := handlers.Timeout
		if timeout == 0 {
			timeout = 5 * time.Second
		}
		if timeout < 0 {
			return invalid(op, "callback timeout cannot be negative")
		}
		var err error
		callbackID, err = callbacks.registerCallback(
			func(callbackCtx context.Context, method string, payload json.RawMessage) (any, error) {
				switch method {
				case "check_before_llm":
					if handlers.CheckBeforeLLM == nil {
						return &BudgetDecision{Decision: "allow"}, nil
					}
					var value BudgetLLMContext
					if err := json.Unmarshal(payload, &value); err != nil {
						return nil, err
					}
					return handlers.CheckBeforeLLM(callbackCtx, value)
				case "record_after_llm":
					if handlers.RecordAfterLLM == nil {
						return nil, nil
					}
					var value BudgetUsageContext
					if err := json.Unmarshal(payload, &value); err != nil {
						return nil, err
					}
					return nil, handlers.RecordAfterLLM(callbackCtx, value)
				case "check_before_tool":
					if handlers.CheckBeforeTool == nil {
						return &BudgetDecision{Decision: "allow"}, nil
					}
					var value BudgetToolContext
					if err := json.Unmarshal(payload, &value); err != nil {
						return nil, err
					}
					return handlers.CheckBeforeTool(callbackCtx, value)
				default:
					return nil, fmt.Errorf("unexpected budget callback method %q", method)
				}
			},
		)
		if err != nil {
			return err
		}
		params["handler_id"] = callbackID
		params["timeout_ms"] = timeout.Milliseconds()
	}
	if err := session.runtime.Request(ctx, op, params, nil); err != nil {
		if callbackID != "" {
			callbacks.unregisterCallback(callbackID)
		}
		return err
	}

	session.callbackMu.Lock()
	previous := session.budgetCallback
	session.budgetCallback = callbackID
	session.callbackMu.Unlock()
	if previous != "" && supportsCallbacks {
		callbacks.unregisterCallback(previous)
	}
	return nil
}

func (session *Session) RegisterCommand(
	ctx context.Context,
	name string,
	description string,
	usage string,
	handler CommandHandler,
	timeout time.Duration,
) error {
	const op = "session_register_command"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	if strings.TrimSpace(name) == "" || strings.TrimSpace(description) == "" {
		return invalid(op, "command name and description cannot be empty")
	}
	if handler == nil {
		return invalid(op, "command handler cannot be nil")
	}
	if timeout == 0 {
		timeout = 5 * time.Second
	}
	if timeout < 0 {
		return invalid(op, "callback timeout cannot be negative")
	}
	callbacks, ok := session.runtime.(callbackRuntime)
	if !ok {
		return sdkError(op, CodeUnavailable, "runtime does not support Go callbacks", nil)
	}
	callbackID, err := callbacks.registerCallback(
		func(callbackCtx context.Context, method string, payload json.RawMessage) (any, error) {
			if method != "command" {
				return nil, fmt.Errorf("unexpected command callback method %q", method)
			}
			var request struct {
				Args    string         `json:"args"`
				Context CommandContext `json:"context"`
			}
			if err := json.Unmarshal(payload, &request); err != nil {
				return nil, err
			}
			return handler(callbackCtx, request.Args, request.Context)
		},
	)
	if err != nil {
		return err
	}
	params := session.params()
	params["name"] = name
	params["description"] = description
	params["handler_id"] = callbackID
	params["timeout_ms"] = timeout.Milliseconds()
	if usage != "" {
		params["usage"] = usage
	}
	if err := session.runtime.Request(ctx, op, params, nil); err != nil {
		callbacks.unregisterCallback(callbackID)
		return err
	}

	session.callbackMu.Lock()
	if session.commandCallbacks == nil {
		session.commandCallbacks = make(map[string]string)
	}
	previous := session.commandCallbacks[name]
	session.commandCallbacks[name] = callbackID
	session.callbackMu.Unlock()
	callbacks.unregisterCallback(previous)
	return nil
}
