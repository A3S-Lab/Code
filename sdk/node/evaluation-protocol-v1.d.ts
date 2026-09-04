/**
 * Generated from core/src/evaluation/protocol.rs.
 * Run `node scripts/generate_evaluation_protocol_artifacts.mjs` to update.
 *
 * Payloads remain opaque JSON objects at the SDK boundary. Rust Core owns the
 * closed payload schemas and validation; hosts own business/reviewer meaning.
 */

export const EVALUATION_PROTOCOL_VERSION_V1 = 1 as const
export const EVALUATION_PROTOCOL_SCHEMA_V1 = 'a3s.code.evaluation-wire.v1' as const
export const EVALUATION_PROTOCOL_MAX_MESSAGE_BYTES = 33554432 as const

/** Closed top-level kinds accepted by evaluation wire version 1. */
export type KnownEvaluationWireKindV1 =
  | 'evidence_read_request'
  | 'evidence_snapshot'
  | 'auxiliary_run_spec'
  | 'auxiliary_run_snapshot'
  | 'auxiliary_run_output'
  | 'evaluation_result'
  | 'evaluation_record'

export type EvaluationWireKindV1 = KnownEvaluationWireKindV1

export const EvaluationWireTypeV1 = {
  EVIDENCE_READ_REQUEST: 'evidence_read_request',
  EVIDENCE_SNAPSHOT: 'evidence_snapshot',
  AUXILIARY_RUN_SPEC: 'auxiliary_run_spec',
  AUXILIARY_RUN_SNAPSHOT: 'auxiliary_run_snapshot',
  AUXILIARY_RUN_OUTPUT: 'auxiliary_run_output',
  EVALUATION_RESULT: 'evaluation_result',
  EVALUATION_RECORD: 'evaluation_record',
} as const

export const EVALUATION_WIRE_KINDS_V1 = [
  'evidence_read_request',
  'evidence_snapshot',
  'auxiliary_run_spec',
  'auxiliary_run_snapshot',
  'auxiliary_run_output',
  'evaluation_result',
  'evaluation_record',
] as const satisfies readonly KnownEvaluationWireKindV1[]

/** Strict envelope shared by Core and all SDKs. */
export interface EvaluationWireEnvelopeV1<TPayload = EvaluationWirePayloadV1> {
  readonly schema: typeof EVALUATION_PROTOCOL_SCHEMA_V1
  readonly version: typeof EVALUATION_PROTOCOL_VERSION_V1
  readonly kind: EvaluationWireKindV1
  readonly payload: TPayload
}

export type EvidenceReadRequestPayloadV1 = Readonly<Record<string, unknown>>
export type EvidenceSnapshotPayloadV1 = Readonly<Record<string, unknown>>
export type AuxiliaryRunSpecPayloadV1 = Readonly<Record<string, unknown>>
export type AuxiliaryRunSnapshotPayloadV1 = Readonly<Record<string, unknown>>
export type AuxiliaryRunOutputPayloadV1 = Readonly<Record<string, unknown>>
export type EvaluationResultPayloadV1 = Readonly<Record<string, unknown>>
export type EvaluationRecordPayloadV1 = Readonly<Record<string, unknown>>

/** Opaque JSON payload preserved for host-owned transport adapters. */
export type EvaluationWirePayloadV1 = Readonly<Record<string, unknown>>

/** Union of all payload shapes known to Core at wire version 1. */
export type KnownEvaluationWirePayloadV1 =
  | EvidenceReadRequestPayloadV1
  | EvidenceSnapshotPayloadV1
  | AuxiliaryRunSpecPayloadV1
  | AuxiliaryRunSnapshotPayloadV1
  | AuxiliaryRunOutputPayloadV1
  | EvaluationResultPayloadV1
  | EvaluationRecordPayloadV1

/** Discriminated message union for exhaustive SDK dispatch. */
export type EvaluationWireMessageV1 =
  | (EvaluationWireEnvelopeV1<EvidenceReadRequestPayloadV1> & { readonly kind: 'evidence_read_request' })
  | (EvaluationWireEnvelopeV1<EvidenceSnapshotPayloadV1> & { readonly kind: 'evidence_snapshot' })
  | (EvaluationWireEnvelopeV1<AuxiliaryRunSpecPayloadV1> & { readonly kind: 'auxiliary_run_spec' })
  | (EvaluationWireEnvelopeV1<AuxiliaryRunSnapshotPayloadV1> & { readonly kind: 'auxiliary_run_snapshot' })
  | (EvaluationWireEnvelopeV1<AuxiliaryRunOutputPayloadV1> & { readonly kind: 'auxiliary_run_output' })
  | (EvaluationWireEnvelopeV1<EvaluationResultPayloadV1> & { readonly kind: 'evaluation_result' })
  | (EvaluationWireEnvelopeV1<EvaluationRecordPayloadV1> & { readonly kind: 'evaluation_record' })
