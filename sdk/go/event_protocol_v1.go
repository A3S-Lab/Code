// Code generated from core/src/event_protocol.rs; DO NOT EDIT.
//
// Run: node scripts/generate_event_protocol_artifacts.mjs

package code

const EventEnvelopeV1Version = 1

const (
	EventAgentStart            = "agent_start"
	EventAgentModeChanged      = "agent_mode_changed"
	EventTurnStart             = "turn_start"
	EventTextDelta             = "text_delta"
	EventReasoningDelta        = "reasoning_delta"
	EventToolStart             = "tool_start"
	EventToolInputDelta        = "tool_input_delta"
	EventToolExecutionStart    = "tool_execution_start"
	EventToolEnd               = "tool_end"
	EventToolOutputDelta       = "tool_output_delta"
	EventTurnEnd               = "turn_end"
	EventAgentEnd              = "agent_end"
	EventError                 = "error"
	EventConfirmationRequired  = "confirmation_required"
	EventConfirmationReceived  = "confirmation_received"
	EventConfirmationTimeout   = "confirmation_timeout"
	EventExternalTaskPending   = "external_task_pending"
	EventExternalTaskCompleted = "external_task_completed"
	EventPermissionDenied      = "permission_denied"
	EventContextResolving      = "context_resolving"
	EventContextResolved       = "context_resolved"
	EventCognitiveContextBound = "cognitive_context_bound"
	EventCommandDeadLettered   = "command_dead_lettered"
	EventCommandRetry          = "command_retry"
	EventQueueAlert            = "queue_alert"
	EventTaskUpdated           = "task_updated"
	EventMemoryStored          = "memory_stored"
	EventMemoryRecalled        = "memory_recalled"
	EventMemoriesSearched      = "memories_searched"
	EventMemoryCleared         = "memory_cleared"
	EventSubagentStart         = "subagent_start"
	EventSubagentProgress      = "subagent_progress"
	EventSubagentEnd           = "subagent_end"
	EventPlanningStart         = "planning_start"
	EventPlanningEnd           = "planning_end"
	EventStepStart             = "step_start"
	EventStepEnd               = "step_end"
	EventGoalExtracted         = "goal_extracted"
	EventGoalProgress          = "goal_progress"
	EventGoalAchieved          = "goal_achieved"
	EventContextCompacted      = "context_compacted"
	EventPersistenceFailed     = "persistence_failed"
	EventBudgetThresholdHit    = "budget_threshold_hit"
	EventPassivationRequested  = "passivation_requested"
	EventPeerInvocation        = "peer_invocation"
)

var agentEventTypesV1 = [...]string{
	EventAgentStart,
	EventAgentModeChanged,
	EventTurnStart,
	EventTextDelta,
	EventReasoningDelta,
	EventToolStart,
	EventToolInputDelta,
	EventToolExecutionStart,
	EventToolEnd,
	EventToolOutputDelta,
	EventTurnEnd,
	EventAgentEnd,
	EventError,
	EventConfirmationRequired,
	EventConfirmationReceived,
	EventConfirmationTimeout,
	EventExternalTaskPending,
	EventExternalTaskCompleted,
	EventPermissionDenied,
	EventContextResolving,
	EventContextResolved,
	EventCognitiveContextBound,
	EventCommandDeadLettered,
	EventCommandRetry,
	EventQueueAlert,
	EventTaskUpdated,
	EventMemoryStored,
	EventMemoryRecalled,
	EventMemoriesSearched,
	EventMemoryCleared,
	EventSubagentStart,
	EventSubagentProgress,
	EventSubagentEnd,
	EventPlanningStart,
	EventPlanningEnd,
	EventStepStart,
	EventStepEnd,
	EventGoalExtracted,
	EventGoalProgress,
	EventGoalAchieved,
	EventContextCompacted,
	EventPersistenceFailed,
	EventBudgetThresholdHit,
	EventPassivationRequested,
	EventPeerInvocation,
}

// AgentEventTypesV1 returns the ordered catalog known to event envelope version 1.
// Event.Type remains an open string so future event types are preserved.
func AgentEventTypesV1() []string {
	return append([]string(nil), agentEventTypesV1[:]...)
}
