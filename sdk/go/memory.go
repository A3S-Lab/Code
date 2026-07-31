package code

import (
	"context"
	"strings"
)

func (session *Session) HasMemory(ctx context.Context) (bool, error) {
	const op = "session_has_memory"
	if err := validateSession(session, ctx, op); err != nil {
		return false, err
	}
	var result struct {
		Available bool `json:"available"`
	}
	err := session.runtime.Request(ctx, op, session.params(), &result)
	return result.Available, err
}

func (session *Session) RememberSuccess(
	ctx context.Context,
	task string,
	tools []string,
	result string,
) error {
	const op = "session_remember_success"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	if strings.TrimSpace(task) == "" {
		return invalid(op, "task cannot be empty")
	}
	params := session.params()
	params["task"] = task
	params["tools"] = tools
	params["result"] = result
	return session.runtime.Request(ctx, op, params, nil)
}

func (session *Session) RememberFailure(
	ctx context.Context,
	task string,
	failure string,
	tools []string,
) error {
	const op = "session_remember_failure"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	if strings.TrimSpace(task) == "" {
		return invalid(op, "task cannot be empty")
	}
	params := session.params()
	params["task"] = task
	params["error"] = failure
	params["tools"] = tools
	return session.runtime.Request(ctx, op, params, nil)
}

func (session *Session) RecallSimilar(
	ctx context.Context,
	query string,
	limit uint,
) ([]MemoryItem, error) {
	const op = "session_recall_similar"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(query) == "" {
		return nil, invalid(op, "query cannot be empty")
	}
	params := session.params()
	params["query"] = query
	if limit > 0 {
		params["limit"] = limit
	}
	var result []MemoryItem
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return result, nil
}

func (session *Session) RecallByTags(
	ctx context.Context,
	tags []string,
	limit uint,
) ([]MemoryItem, error) {
	const op = "session_recall_by_tags"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if len(tags) == 0 {
		return nil, invalid(op, "at least one tag is required")
	}
	params := session.params()
	params["tags"] = tags
	if limit > 0 {
		params["limit"] = limit
	}
	var result []MemoryItem
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return result, nil
}

func (session *Session) MemoryRecent(
	ctx context.Context,
	limit uint,
) ([]MemoryItem, error) {
	const op = "session_memory_recent"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	params := session.params()
	if limit > 0 {
		params["limit"] = limit
	}
	var result []MemoryItem
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return nil, err
	}
	return result, nil
}

func (session *Session) MemoryStats(ctx context.Context) (MemoryStats, error) {
	const op = "session_memory_stats"
	if err := validateSession(session, ctx, op); err != nil {
		return MemoryStats{}, err
	}
	var result MemoryStats
	err := session.runtime.Request(ctx, op, session.params(), &result)
	return result, err
}

func (session *Session) WorkingMemory(ctx context.Context) ([]MemoryItem, error) {
	return session.memoryItems(ctx, "session_get_working_memory")
}

func (session *Session) GetWorking(ctx context.Context) ([]MemoryItem, error) {
	return session.WorkingMemory(ctx)
}

func (session *Session) ClearWorkingMemory(ctx context.Context) error {
	return session.memoryMutation(ctx, "session_clear_working_memory")
}

func (session *Session) ClearWorking(ctx context.Context) error {
	return session.ClearWorkingMemory(ctx)
}

func (session *Session) ShortTermMemory(ctx context.Context) ([]MemoryItem, error) {
	return session.memoryItems(ctx, "session_get_short_term_memory")
}

func (session *Session) GetShortTerm(ctx context.Context) ([]MemoryItem, error) {
	return session.ShortTermMemory(ctx)
}

func (session *Session) ClearShortTermMemory(ctx context.Context) error {
	return session.memoryMutation(ctx, "session_clear_short_term_memory")
}

func (session *Session) ClearShortTerm(ctx context.Context) error {
	return session.ClearShortTermMemory(ctx)
}

func (session *Session) memoryItems(
	ctx context.Context,
	operation string,
) ([]MemoryItem, error) {
	if err := validateSession(session, ctx, operation); err != nil {
		return nil, err
	}
	var result []MemoryItem
	if err := session.runtime.Request(ctx, operation, session.params(), &result); err != nil {
		return nil, err
	}
	return result, nil
}

func (session *Session) memoryMutation(ctx context.Context, operation string) error {
	if err := validateSession(session, ctx, operation); err != nil {
		return err
	}
	return session.runtime.Request(ctx, operation, session.params(), nil)
}
