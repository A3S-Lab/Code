/**
 * Generated from core/src/event_protocol.rs.
 * Run `node scripts/generate_event_protocol_artifacts.mjs` to update.
 */

import type { AgentEvent } from './generated'

/** Event types defined by envelope version 1. */
export type KnownAgentEventTypeV1 =
  | 'agent_start'
  | 'agent_mode_changed'
  | 'turn_start'
  | 'text_delta'
  | 'reasoning_delta'
  | 'tool_start'
  | 'tool_input_delta'
  | 'tool_request_bound'
  | 'tool_execution_start'
  | 'tool_end'
  | 'tool_output_delta'
  | 'turn_end'
  | 'agent_end'
  | 'error'
  | 'confirmation_required'
  | 'confirmation_received'
  | 'confirmation_timeout'
  | 'external_task_pending'
  | 'external_task_completed'
  | 'permission_denied'
  | 'context_resolving'
  | 'context_resolved'
  | 'run_capability_bound'
  | 'model_presentation_bound'
  | 'model_input_bound'
  | 'model_usage_bound'
  | 'cognitive_context_bound'
  | 'command_dead_lettered'
  | 'command_retry'
  | 'queue_alert'
  | 'task_updated'
  | 'memory_stored'
  | 'memory_recalled'
  | 'memories_searched'
  | 'memory_cleared'
  | 'subagent_start'
  | 'subagent_progress'
  | 'subagent_end'
  | 'planning_start'
  | 'planning_end'
  | 'step_start'
  | 'step_end'
  | 'goal_extracted'
  | 'goal_progress'
  | 'goal_achieved'
  | 'context_compacted'
  | 'persistence_failed'
  | 'budget_threshold_hit'
  | 'passivation_requested'
  | 'peer_invocation'

/**
 * Open event discriminant. Known values retain autocomplete while unknown
 * future values remain representable.
 */
export type AgentEventTypeV1 = KnownAgentEventTypeV1 | (string & {})

/** Stable, lossless event envelope shared by the core and SDKs. */
export interface EventEnvelopeV1<TPayload = unknown, TMetadata = unknown> {
  readonly version: 1
  readonly type: AgentEventTypeV1
  readonly payload: TPayload
  readonly metadata?: TMetadata
}

/** AgentEvent convenience fields combined with the strict v1 envelope. */
export type AgentEventV1<TPayload = unknown, TMetadata = unknown> =
  Omit<AgentEvent, 'version' | 'type' | 'payload' | 'metadata'>
  & EventEnvelopeV1<TPayload, TMetadata>
