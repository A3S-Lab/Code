package code

import (
	"context"
	"errors"
	"fmt"
)

type ErrorCode string

const (
	CodeInvalidRequest        ErrorCode = "INVALID_REQUEST"
	CodeNotFound              ErrorCode = "NOT_FOUND"
	CodeUnavailable           ErrorCode = "UNAVAILABLE"
	CodeProtocol              ErrorCode = "PROTOCOL_ERROR"
	CodeRuntime               ErrorCode = "RUNTIME_ERROR"
	CodeBridgeClosed          ErrorCode = "BRIDGE_CLOSED"
	CodeBridgeTimeout         ErrorCode = "BRIDGE_TIMEOUT"
	CodeNotInstalled          ErrorCode = "NOT_INSTALLED"
	CodeConfig                ErrorCode = "CONFIG_ERROR"
	CodeLLM                   ErrorCode = "LLM_ERROR"
	CodeTool                  ErrorCode = "TOOL_ERROR"
	CodeSession               ErrorCode = "SESSION_ERROR"
	CodeSessionConfiguration  ErrorCode = "SESSION_CONFIGURATION_ERROR"
	CodeSessionInitialization ErrorCode = "SESSION_INITIALIZATION_ERROR"
	CodeSessionClosed         ErrorCode = "SESSION_CLOSED"
	CodeSessionBusy           ErrorCode = "SESSION_BUSY"
	CodeBudgetExhausted       ErrorCode = "BUDGET_EXHAUSTED"
	CodeSecurity              ErrorCode = "SECURITY_ERROR"
	CodeContext               ErrorCode = "CONTEXT_ERROR"
	CodeMCP                   ErrorCode = "MCP_ERROR"
	CodeQueue                 ErrorCode = "QUEUE_ERROR"
	CodeIO                    ErrorCode = "IO_ERROR"
	CodeSerialization         ErrorCode = "SERIALIZATION_ERROR"
	CodeInternal              ErrorCode = "INTERNAL_ERROR"
	CodeUnsupportedOperation  ErrorCode = "UNSUPPORTED_OPERATION"
	CodeWorkspaceRetrieval    ErrorCode = "WORKSPACE_RETRIEVAL_ERROR"
	CodeStateGraphClosed      ErrorCode = "STATE_GRAPH_CLOSED"
)

const (
	CodeTaskAdmissionCancelled ErrorCode = "TASK_ADMISSION_CANCELLED"
	CodeTaskSchedulerClosed    ErrorCode = "TASK_SCHEDULER_CLOSED"
	CodeRunIdentityConflict    ErrorCode = "RUN_IDENTITY_CONFLICT"
	CodeRunControlInvalidRequest ErrorCode = "INVALID_REQUEST"
	CodeRunControlSessionMismatch ErrorCode = "SESSION_MISMATCH"
	CodeRunControlRunMismatch ErrorCode = "RUN_MISMATCH"
	CodeRunControlNoActiveRun ErrorCode = "NO_ACTIVE_RUN"
	CodeRunControlStaleTurn ErrorCode = "STALE_TURN"
	CodeRunControlDeadlineExceeded ErrorCode = "DEADLINE_EXCEEDED"
	CodeRunControlQueueFull ErrorCode = "QUEUE_FULL"
	CodeRunControlClosed ErrorCode = "CLOSED"
	CodeRunControlDuplicateRequest ErrorCode = "DUPLICATE_REQUEST"
	CodeRunControlHookDenied ErrorCode = "HOOK_DENIED"
	CodeRunControlHookRetry ErrorCode = "HOOK_RETRY"
)

const (
	CodeServeStartupFailed            ErrorCode = "SERVE_STARTUP_FAILED"
	CodeServeRuntimeFailed            ErrorCode = "SERVE_RUNTIME_FAILED"
	CodeServeDaemonPanicked           ErrorCode = "SERVE_DAEMON_PANICKED"
	CodeServeShutdownDeadlineExceeded ErrorCode = "SERVE_SHUTDOWN_DEADLINE_EXCEEDED"
)

var (
	ErrInvalidRequest        = &Error{Code: CodeInvalidRequest}
	ErrNotFound              = &Error{Code: CodeNotFound}
	ErrUnavailable           = &Error{Code: CodeUnavailable}
	ErrProtocol              = &Error{Code: CodeProtocol}
	ErrRuntime               = &Error{Code: CodeRuntime}
	ErrBridgeClosed          = &Error{Code: CodeBridgeClosed}
	ErrBridgeTimeout         = &Error{Code: CodeBridgeTimeout}
	ErrNotInstalled          = &Error{Code: CodeNotInstalled}
	ErrConfig                = &Error{Code: CodeConfig}
	ErrLLM                   = &Error{Code: CodeLLM}
	ErrTool                  = &Error{Code: CodeTool}
	ErrSession               = &Error{Code: CodeSession}
	ErrSessionConfiguration  = &Error{Code: CodeSessionConfiguration}
	ErrSessionInitialization = &Error{Code: CodeSessionInitialization}
	ErrSessionClosed         = &Error{Code: CodeSessionClosed}
	ErrSessionBusy           = &Error{Code: CodeSessionBusy}
	ErrBudgetExhausted       = &Error{Code: CodeBudgetExhausted}
	ErrSecurity              = &Error{Code: CodeSecurity}
	ErrContext               = &Error{Code: CodeContext}
	ErrMCP                   = &Error{Code: CodeMCP}
	ErrQueue                 = &Error{Code: CodeQueue}
	ErrIO                    = &Error{Code: CodeIO}
	ErrSerialization         = &Error{Code: CodeSerialization}
	ErrInternal              = &Error{Code: CodeInternal}
	ErrUnsupportedOperation  = &Error{Code: CodeUnsupportedOperation}
	ErrWorkspaceRetrieval    = &Error{Code: CodeWorkspaceRetrieval}
	ErrStateGraphClosed      = &Error{Code: CodeStateGraphClosed}
)

var (
	ErrTaskAdmissionCancelled = &Error{Code: CodeTaskAdmissionCancelled}
	ErrTaskSchedulerClosed    = &Error{Code: CodeTaskSchedulerClosed}
	ErrRunIdentityConflict    = &Error{Code: CodeRunIdentityConflict}
)

var (
	ErrServeStartupFailed            = &Error{Code: CodeServeStartupFailed}
	ErrServeRuntimeFailed            = &Error{Code: CodeServeRuntimeFailed}
	ErrServeDaemonPanicked           = &Error{Code: CodeServeDaemonPanicked}
	ErrServeShutdownDeadlineExceeded = &Error{Code: CodeServeShutdownDeadlineExceeded}
)

type Error struct {
	Op      string
	Code    ErrorCode
	Message string
	Err     error
}

func (err *Error) Error() string {
	if err == nil {
		return "<nil>"
	}
	message := err.Message
	if message == "" {
		message = string(err.Code)
	}
	if err.Op == "" {
		return message
	}
	return fmt.Sprintf("%s: %s", err.Op, message)
}

func (err *Error) Unwrap() error {
	if err == nil {
		return nil
	}
	return err.Err
}

func (err *Error) Is(target error) bool {
	other, ok := target.(*Error)
	return ok && err != nil && other != nil && other.Code != "" && err.Code == other.Code
}

func sdkError(op string, code ErrorCode, message string, cause error) error {
	return &Error{Op: op, Code: code, Message: message, Err: cause}
}

func invalid(op, message string) error {
	return sdkError(op, CodeInvalidRequest, message, nil)
}

func contextError(op string, err error) error {
	switch {
	case errors.Is(err, context.Canceled):
		return sdkError(op, CodeContext, "operation canceled", context.Canceled)
	case errors.Is(err, context.DeadlineExceeded):
		return sdkError(op, CodeContext, "operation deadline exceeded", context.DeadlineExceeded)
	default:
		return sdkError(op, CodeContext, "context failed", err)
	}
}
