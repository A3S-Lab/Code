"""Generated version-1 event protocol declarations.

Generated from core/src/event_protocol.rs. Run
node scripts/generate_event_protocol_artifacts.mjs to update.
"""

from typing import Final, Literal, Tuple

EVENT_ENVELOPE_V1_VERSION: Final[int] = 1

KnownAgentEventTypeV1 = Literal[
    "agent_start",
    "agent_mode_changed",
    "turn_start",
    "text_delta",
    "reasoning_delta",
    "tool_start",
    "tool_input_delta",
    "tool_execution_start",
    "tool_end",
    "tool_output_delta",
    "turn_end",
    "agent_end",
    "error",
    "confirmation_required",
    "confirmation_received",
    "confirmation_timeout",
    "external_task_pending",
    "external_task_completed",
    "permission_denied",
    "context_resolving",
    "context_resolved",
    "command_dead_lettered",
    "command_retry",
    "queue_alert",
    "task_updated",
    "memory_stored",
    "memory_recalled",
    "memories_searched",
    "memory_cleared",
    "subagent_start",
    "subagent_progress",
    "subagent_end",
    "planning_start",
    "planning_end",
    "step_start",
    "step_end",
    "goal_extracted",
    "goal_progress",
    "goal_achieved",
    "context_compacted",
    "persistence_failed",
    "budget_threshold_hit",
    "passivation_requested",
    "peer_invocation",
]

# Event types are open for forward compatibility. Use the known alias when an
# exhaustive catalog is specifically required.
AgentEventTypeV1 = str

AGENT_EVENT_TYPES_V1: Final[Tuple[KnownAgentEventTypeV1, ...]] = (
    "agent_start",
    "agent_mode_changed",
    "turn_start",
    "text_delta",
    "reasoning_delta",
    "tool_start",
    "tool_input_delta",
    "tool_execution_start",
    "tool_end",
    "tool_output_delta",
    "turn_end",
    "agent_end",
    "error",
    "confirmation_required",
    "confirmation_received",
    "confirmation_timeout",
    "external_task_pending",
    "external_task_completed",
    "permission_denied",
    "context_resolving",
    "context_resolved",
    "command_dead_lettered",
    "command_retry",
    "queue_alert",
    "task_updated",
    "memory_stored",
    "memory_recalled",
    "memories_searched",
    "memory_cleared",
    "subagent_start",
    "subagent_progress",
    "subagent_end",
    "planning_start",
    "planning_end",
    "step_start",
    "step_end",
    "goal_extracted",
    "goal_progress",
    "goal_achieved",
    "context_compacted",
    "persistence_failed",
    "budget_threshold_hit",
    "passivation_requested",
    "peer_invocation",
)


class EventType:
    """Canonical string constants for AgentEvent.type."""

    AGENT_START: Final[str] = "agent_start"
    AGENT_MODE_CHANGED: Final[str] = "agent_mode_changed"
    TURN_START: Final[str] = "turn_start"
    TEXT_DELTA: Final[str] = "text_delta"
    REASONING_DELTA: Final[str] = "reasoning_delta"
    TOOL_START: Final[str] = "tool_start"
    TOOL_INPUT_DELTA: Final[str] = "tool_input_delta"
    TOOL_EXECUTION_START: Final[str] = "tool_execution_start"
    TOOL_END: Final[str] = "tool_end"
    TOOL_OUTPUT_DELTA: Final[str] = "tool_output_delta"
    TURN_END: Final[str] = "turn_end"
    AGENT_END: Final[str] = "agent_end"
    ERROR: Final[str] = "error"
    CONFIRMATION_REQUIRED: Final[str] = "confirmation_required"
    CONFIRMATION_RECEIVED: Final[str] = "confirmation_received"
    CONFIRMATION_TIMEOUT: Final[str] = "confirmation_timeout"
    EXTERNAL_TASK_PENDING: Final[str] = "external_task_pending"
    EXTERNAL_TASK_COMPLETED: Final[str] = "external_task_completed"
    PERMISSION_DENIED: Final[str] = "permission_denied"
    CONTEXT_RESOLVING: Final[str] = "context_resolving"
    CONTEXT_RESOLVED: Final[str] = "context_resolved"
    COMMAND_DEAD_LETTERED: Final[str] = "command_dead_lettered"
    COMMAND_RETRY: Final[str] = "command_retry"
    QUEUE_ALERT: Final[str] = "queue_alert"
    TASK_UPDATED: Final[str] = "task_updated"
    MEMORY_STORED: Final[str] = "memory_stored"
    MEMORY_RECALLED: Final[str] = "memory_recalled"
    MEMORIES_SEARCHED: Final[str] = "memories_searched"
    MEMORY_CLEARED: Final[str] = "memory_cleared"
    SUBAGENT_START: Final[str] = "subagent_start"
    SUBAGENT_PROGRESS: Final[str] = "subagent_progress"
    SUBAGENT_END: Final[str] = "subagent_end"
    PLANNING_START: Final[str] = "planning_start"
    PLANNING_END: Final[str] = "planning_end"
    STEP_START: Final[str] = "step_start"
    STEP_END: Final[str] = "step_end"
    GOAL_EXTRACTED: Final[str] = "goal_extracted"
    GOAL_PROGRESS: Final[str] = "goal_progress"
    GOAL_ACHIEVED: Final[str] = "goal_achieved"
    CONTEXT_COMPACTED: Final[str] = "context_compacted"
    PERSISTENCE_FAILED: Final[str] = "persistence_failed"
    BUDGET_THRESHOLD_HIT: Final[str] = "budget_threshold_hit"
    PASSIVATION_REQUESTED: Final[str] = "passivation_requested"
    PEER_INVOCATION: Final[str] = "peer_invocation"

    # Compatibility aliases. Values use the canonical v1 wire names.
    START: Final[str] = AGENT_START
    END: Final[str] = AGENT_END

    @classmethod
    def values(cls) -> Tuple[KnownAgentEventTypeV1, ...]:
        """Return the ordered version-1 event type catalog."""

        return AGENT_EVENT_TYPES_V1


__all__ = [
    "AGENT_EVENT_TYPES_V1",
    "AgentEventTypeV1",
    "EVENT_ENVELOPE_V1_VERSION",
    "EventType",
    "KnownAgentEventTypeV1",
]
