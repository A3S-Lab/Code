package code

import (
	"context"
	"strings"
)

// SteerOptions carries idempotency and optimistic-concurrency fields for a
// host steering request. Nil pointers are omitted from the wire request.
type SteerOptions struct {
	RequestID            *string `json:"request_id,omitempty"`
	RunID                *string `json:"run_id,omitempty"`
	ExpectedTurnID       *string `json:"expected_turn_id,omitempty"`
	ExpectedTurnRevision *uint64 `json:"expected_turn_revision,omitempty"`
	DeadlineMS           *uint64 `json:"deadline_ms,omitempty"`
}

// InterruptOptions carries the optional reason, force hint, and concurrency
// fields for a cooperative run interruption.
type InterruptOptions struct {
	Reason               *string `json:"reason,omitempty"`
	Force                bool    `json:"force,omitempty"`
	RequestID            *string `json:"request_id,omitempty"`
	RunID                *string `json:"run_id,omitempty"`
	ExpectedTurnID       *string `json:"expected_turn_id,omitempty"`
	ExpectedTurnRevision *uint64 `json:"expected_turn_revision,omitempty"`
	DeadlineMS           *uint64 `json:"deadline_ms,omitempty"`
}

// RunControlReceipt is the durable acknowledgement returned by Steer or
// Interrupt. State is one of accepted, applied, rejected, or settled.
type RunControlReceipt struct {
	Schema       string           `json:"schema"`
	RequestID    string           `json:"request_id"`
	SessionID    string           `json:"session_id"`
	RunID        string           `json:"run_id"`
	Operation    string           `json:"operation"`
	State        string           `json:"state"`
	Sequence     uint64           `json:"sequence"`
	TurnID       *string          `json:"turn_id,omitempty"`
	TurnRevision uint64           `json:"turn_revision"`
	AcceptedAtMS uint64           `json:"accepted_at_ms"`
	AppliedAtMS  *uint64          `json:"applied_at_ms,omitempty"`
	Error        *RunControlError `json:"error,omitempty"`
}

// RunControlError describes a rejected or settled control request.
type RunControlError struct {
	Code    string `json:"code"`
	Message string `json:"message"`
}

// RunControlSnapshot is the optimistic-concurrency view of the active run.
type RunControlSnapshot struct {
	SessionID          string  `json:"session_id"`
	RunID              string  `json:"run_id"`
	Active             bool    `json:"active"`
	TurnID             *string `json:"turn_id,omitempty"`
	TurnRevision       uint64  `json:"turn_revision"`
	QueuedControls     uint64  `json:"queued_controls"`
	InterruptRequested bool    `json:"interrupt_requested"`
	LastSequence       uint64  `json:"last_sequence"`
}

// Steer appends input to the active run at its next safe point. It never
// starts a second turn and is idempotent when RequestID is reused unchanged.
func (session *Session) Steer(
	ctx context.Context,
	input string,
	options *SteerOptions,
) (RunControlReceipt, error) {
	const op = "session_steer"
	if err := validateSession(session, ctx, op); err != nil {
		return RunControlReceipt{}, err
	}
	if strings.TrimSpace(input) == "" {
		return RunControlReceipt{}, invalid(op, "steer input cannot be empty")
	}
	params := session.params()
	params["input"] = input
	if options != nil {
		if options.RequestID != nil {
			params["request_id"] = *options.RequestID
		}
		if options.RunID != nil {
			params["run_id"] = *options.RunID
		}
		if options.ExpectedTurnID != nil {
			params["expected_turn_id"] = *options.ExpectedTurnID
		}
		if options.ExpectedTurnRevision != nil {
			params["expected_turn_revision"] = *options.ExpectedTurnRevision
		}
		if options.DeadlineMS != nil {
			params["deadline_ms"] = *options.DeadlineMS
		}
	}
	var result RunControlReceipt
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return RunControlReceipt{}, err
	}
	return result, nil
}

// Interrupt cooperatively stops the active run after the current safe
// provider/tool boundary. Force remains advisory and never skips cleanup.
func (session *Session) Interrupt(
	ctx context.Context,
	options *InterruptOptions,
) (RunControlReceipt, error) {
	const op = "session_interrupt"
	if err := validateSession(session, ctx, op); err != nil {
		return RunControlReceipt{}, err
	}
	params := session.params()
	if options != nil {
		if options.Reason != nil {
			params["reason"] = *options.Reason
		}
		if options.Force {
			params["force"] = true
		}
		if options.RequestID != nil {
			params["request_id"] = *options.RequestID
		}
		if options.RunID != nil {
			params["run_id"] = *options.RunID
		}
		if options.ExpectedTurnID != nil {
			params["expected_turn_id"] = *options.ExpectedTurnID
		}
		if options.ExpectedTurnRevision != nil {
			params["expected_turn_revision"] = *options.ExpectedTurnRevision
		}
		if options.DeadlineMS != nil {
			params["deadline_ms"] = *options.DeadlineMS
		}
	}
	var result RunControlReceipt
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return RunControlReceipt{}, err
	}
	return result, nil
}

// RunControlSnapshot returns the active run-control state, or nil when the
// session is idle.
func (session *Session) RunControlSnapshot(ctx context.Context) (*RunControlSnapshot, error) {
	const op = "session_run_control_snapshot"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result *RunControlSnapshot
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return result, nil
}
