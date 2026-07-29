package code

import (
	"context"
	"strings"
)

func (session *Session) HasQueue(ctx context.Context) (bool, error) {
	const op = "session_has_queue"
	if err := validateSession(session, ctx, op); err != nil {
		return false, err
	}
	var result struct {
		Available bool `json:"available"`
	}
	err := session.runtime.Request(ctx, op, session.params(), &result)
	return result.Available, err
}

func (session *Session) SetLaneHandler(
	ctx context.Context,
	lane SessionLane,
	config LaneHandlerConfig,
) error {
	const op = "session_set_lane_handler"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	if lane == "" || strings.TrimSpace(config.Mode) == "" {
		return invalid(op, "lane and handler mode cannot be empty")
	}
	params := session.params()
	params["lane"] = lane
	params["config"] = config
	return session.runtime.Request(ctx, op, params, nil)
}

func (session *Session) CompleteExternalTask(
	ctx context.Context,
	taskID string,
	result ExternalTaskResult,
) (bool, error) {
	const op = "session_complete_external_task"
	if err := validateSession(session, ctx, op); err != nil {
		return false, err
	}
	if strings.TrimSpace(taskID) == "" {
		return false, invalid(op, "task id cannot be empty")
	}
	params := session.params()
	params["task_id"] = taskID
	params["result"] = result
	var response struct {
		Completed bool `json:"completed"`
	}
	err := session.runtime.Request(ctx, op, params, &response)
	return response.Completed, err
}

func (session *Session) PendingExternalTasks(ctx context.Context) ([]ExternalTask, error) {
	const op = "session_pending_external_tasks"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result []ExternalTask
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return result, nil
}

func (session *Session) QueueStats(ctx context.Context) (QueueStats, error) {
	const op = "session_queue_stats"
	if err := validateSession(session, ctx, op); err != nil {
		return QueueStats{}, err
	}
	var result QueueStats
	err := session.runtime.Request(ctx, op, session.params(), &result)
	return result, err
}

func (session *Session) DeadLetters(ctx context.Context) ([]DeadLetter, error) {
	const op = "session_dead_letters"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result []DeadLetter
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return result, nil
}

func (session *Session) QueueMetrics(ctx context.Context) (*QueueMetrics, error) {
	const op = "session_queue_metrics"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result *QueueMetrics
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	return result, nil
}
