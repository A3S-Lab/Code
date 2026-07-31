/**
 * Hand-authored type declarations that complement the auto-generated
 * `generated.d.ts`. These describe shapes that cross the SDK boundary as
 * JSON (e.g. payloads inside `runs()`, `verificationReports()`,
 * `errorKindJson` strings), not napi-bound objects.
 *
 * Keep field names matching the actual JSON wire format (snake_case where
 * the Rust core serializes that way). Do not move these into Rust source —
 * napi-derive would camelCase them and break consumers parsing the JSON.
 */

/**
 * Parsed shape of `ToolResult.errorKindJson` / `AgentEvent.errorKindJson`.
 *
 * Use a discriminated union on the `type` field; new variants may be
 * added in future minor releases — callers should match exhaustively on
 * the kinds they care about and fall through to a default branch for
 * unknown ones.
 */
export type ToolErrorKind =
  | { type: 'version_conflict'; path: string; expected: string; actual: string | null }
  | { type: 'remote_git_conflict'; code: string; message: string }
  | { type: 'not_found'; path: string }
  | { type: 'invalid_argument'; message: string }
  | { type: 'unsupported'; message: string }
  | { type: 'timeout'; op: string; duration_ms: number }
  | { type: 'transport'; op: string }
  | { type: 'cancelled'; op: string }
  | { type: 'partial_failure'; failed: number; total: number }
  | { type: 'rate_limited'; retry_after_ms: number | null }

export type VerificationStatus = 'passed' | 'failed' | 'needs_review' | 'skipped'

export interface VerificationCheck {
  id: string
  kind: string
  description: string
  status: VerificationStatus
  required?: boolean
  suggested_tools?: Array<string>
  evidence_uris?: Array<string>
  residual_risk?: string
}

export interface VerificationReport {
  schema: string
  subject: string
  status: VerificationStatus
  checks: Array<VerificationCheck>
  residual_risks?: Array<string>
}

export interface ToolArtifact {
  artifact_id: string
  artifact_uri: string
  tool_name: string
  content: string
  original_bytes: number
  shown_bytes: number
}

/** Stable codes attached to errors originating in the Rust core. */
export type A3sCodeErrorCode =
  | 'CONFIG_ERROR'
  | 'LLM_ERROR'
  | 'TOOL_ERROR'
  | 'SESSION_ERROR'
  | 'SESSION_CONFIGURATION_ERROR'
  | 'SESSION_INITIALIZATION_ERROR'
  | 'ASYNC_SESSION_BUILD_REQUIRED'
  | 'SESSION_CLOSED'
  | 'SESSION_BUSY'
  | 'BUDGET_EXHAUSTED'
  | 'SECURITY_ERROR'
  | 'CONTEXT_ERROR'
  | 'MCP_ERROR'
  | 'QUEUE_ERROR'
  | 'IO_ERROR'
  | 'SERIALIZATION_ERROR'
  | 'INTERNAL_ERROR'

export interface A3sCodeError extends Error {
  code: A3sCodeErrorCode
}
