#!/usr/bin/env node

/**
 * Generate the language projections for the evaluation wire envelope.
 *
 * Rust remains the source of truth: the generator reads the version, schema,
 * size bound, and one-line kind catalog from core/src/evaluation/protocol.rs.
 * The generated SDK declarations intentionally keep payloads as opaque JSON
 * objects/bytes. Core performs the closed, typed payload validation; hosts and
 * SDKs must not invent a second reviewer or business schema.
 */

import { readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const checkOnly = process.argv.includes('--check');

function read(relativePath) {
  return readFileSync(path.join(root, relativePath), 'utf8');
}

function normalizeLineEndings(value) {
  return value.replace(/\r\n?/g, '\n');
}

function parseSizeExpression(expression) {
  const parts = expression
    .split('*')
    .map((part) => part.trim())
    .filter(Boolean);
  if (parts.length === 0 || parts.some((part) => !/^\d+$/.test(part))) {
    throw new Error(`unsupported size expression: ${expression}`);
  }
  return parts.reduce((total, part) => total * Number(part), 1);
}

function protocolDefinition() {
  const source = read('core/src/evaluation/protocol.rs');
  const versionMatch = source.match(
    /pub const EVALUATION_PROTOCOL_VERSION_V1: u16 = (\d+);/,
  );
  const schemaMatch = source.match(
    /pub const EVALUATION_PROTOCOL_SCHEMA_V1: &str = "([^"]+)";/,
  );
  const maxMessageMatch = source.match(
    /pub const EVALUATION_PROTOCOL_MAX_MESSAGE_BYTES: usize = ([^;]+);/,
  );
  const catalogMatch = source.match(
    /define_evaluation_wire_kinds_v1!\s*\{([\s\S]*?)\n\}/,
  );
  if (!versionMatch || !schemaMatch || !maxMessageMatch || !catalogMatch) {
    throw new Error('could not find evaluation protocol definition');
  }

  const kinds = [];
  const kindPattern =
    /^\s*([A-Za-z0-9_]+)\s*=>\s*([A-Z0-9_]+)\s*=\s*"([a-z0-9_]+)"\s*=>\s*([A-Za-z0-9_]+),?\s*$/gm;
  for (const match of catalogMatch[1].matchAll(kindPattern)) {
    kinds.push({
      variant: match[1],
      constant: match[2],
      wireName: match[3],
      payloadType: match[4],
    });
  }
  if (kinds.length === 0) {
    throw new Error('evaluation protocol catalog is empty');
  }

  for (const key of ['variant', 'constant', 'wireName', 'payloadType']) {
    const values = kinds.map((kind) => kind[key]);
    if (new Set(values).size !== values.length) {
      throw new Error(`evaluation protocol catalog has duplicate ${key} values`);
    }
  }

  return {
    version: Number(versionMatch[1]),
    schema: schemaMatch[1],
    maxMessageBytes: parseSizeExpression(maxMessageMatch[1]),
    kinds,
  };
}

function nodeDeclaration(definition) {
  const { version, schema, maxMessageBytes, kinds } = definition;
  const literals = kinds
    .map((kind) => `  | '${kind.wireName}'`)
    .join('\n');
  const tuple = kinds.map((kind) => `  '${kind.wireName}',`).join('\n');
  const payloadAliases = kinds
    .map(
      (kind) =>
        `export type ${kind.payloadType.replace(/V1$/, '')}PayloadV1 = Readonly<Record<string, unknown>>`,
    )
    .join('\n');
  const payloadUnion = kinds
    .map(
      (kind) =>
        `  | ${kind.payloadType.replace(/V1$/, '')}PayloadV1`,
    )
    .join('\n');
  const messageUnion = kinds
    .map(
      (kind) =>
        `  | (EvaluationWireEnvelopeV1<${kind.payloadType.replace(/V1$/, '')}PayloadV1> & { readonly kind: '${kind.wireName}' })`,
    )
    .join('\n');
  const constants = kinds
    .map(
      (kind) =>
        `  ${kind.constant}: '${kind.wireName}',`,
    )
    .join('\n');
  return `/**
 * Generated from core/src/evaluation/protocol.rs.
 * Run \`node scripts/generate_evaluation_protocol_artifacts.mjs\` to update.
 *
 * Payloads remain opaque JSON objects at the SDK boundary. Rust Core owns the
 * closed payload schemas and validation; hosts own business/reviewer meaning.
 */

export const EVALUATION_PROTOCOL_VERSION_V1 = ${version} as const
export const EVALUATION_PROTOCOL_SCHEMA_V1 = '${schema}' as const
export const EVALUATION_PROTOCOL_MAX_MESSAGE_BYTES = ${maxMessageBytes} as const

/** Closed top-level kinds accepted by evaluation wire version ${version}. */
export type KnownEvaluationWireKindV1 =
${literals}

export type EvaluationWireKindV1 = KnownEvaluationWireKindV1

export const EvaluationWireTypeV1 = {
${constants}
} as const

export const EVALUATION_WIRE_KINDS_V1 = [
${tuple}
] as const satisfies readonly KnownEvaluationWireKindV1[]

/** Strict envelope shared by Core and all SDKs. */
export interface EvaluationWireEnvelopeV1<TPayload = EvaluationWirePayloadV1> {
  readonly schema: typeof EVALUATION_PROTOCOL_SCHEMA_V1
  readonly version: typeof EVALUATION_PROTOCOL_VERSION_V1
  readonly kind: EvaluationWireKindV1
  readonly payload: TPayload
}

${payloadAliases}

/** Opaque JSON payload preserved for host-owned transport adapters. */
export type EvaluationWirePayloadV1 = Readonly<Record<string, unknown>>

/** Union of all payload shapes known to Core at wire version ${version}. */
export type KnownEvaluationWirePayloadV1 =
${payloadUnion}

/** Discriminated message union for exhaustive SDK dispatch. */
export type EvaluationWireMessageV1 =
${messageUnion}
`;
}

function pythonDeclaration(definition) {
  const { version, schema, maxMessageBytes, kinds } = definition;
  const literals = kinds.map((kind) => `    "${kind.wireName}",`).join('\n');
  const constants = kinds
    .map((kind) => `    ${kind.constant}: Final[str] = "${kind.wireName}"`)
    .join('\n');
  return `"""Generated evaluation wire protocol declarations.

Generated from core/src/evaluation/protocol.rs. Run
node scripts/generate_evaluation_protocol_artifacts.mjs to update.

Payload values intentionally remain mappings: Core is the single authority for
closed payload validation, while hosts own transport and business semantics.
"""

from typing import Final, Literal, Mapping, Tuple, TypedDict

EVALUATION_PROTOCOL_VERSION_V1: Final[int] = ${version}
EVALUATION_PROTOCOL_SCHEMA_V1: Final[str] = "${schema}"
EVALUATION_PROTOCOL_MAX_MESSAGE_BYTES: Final[int] = ${maxMessageBytes}

KnownEvaluationWireKindV1 = Literal[
${literals}
]
EvaluationWireKindV1 = KnownEvaluationWireKindV1

EVALUATION_WIRE_KINDS_V1: Final[Tuple[KnownEvaluationWireKindV1, ...]] = (
${literals}
)


class EvaluationWireTypeV1:
    """Canonical string constants for evaluation wire version ${version}."""

${constants}


EvaluationWirePayloadV1 = Mapping[str, object]
${kinds
  .map(
    (kind) =>
      `${kind.payloadType.replace(/V1$/, '')}PayloadV1 = EvaluationWirePayloadV1`,
  )
  .join('\n')}


class EvaluationWireEnvelopeV1(TypedDict):
    """Strict top-level envelope shape emitted by Code Core."""

    schema: str
    version: int
    kind: KnownEvaluationWireKindV1
    payload: EvaluationWirePayloadV1


__all__ = [
    "EVALUATION_PROTOCOL_MAX_MESSAGE_BYTES",
    "EVALUATION_PROTOCOL_SCHEMA_V1",
    "EVALUATION_PROTOCOL_VERSION_V1",
    "EVALUATION_WIRE_KINDS_V1",
    "EvaluationWireEnvelopeV1",
    "EvaluationWireKindV1",
    "EvaluationWirePayloadV1",
    "EvaluationWireTypeV1",
    "KnownEvaluationWireKindV1",
${kinds
  .map(
    (kind) =>
      `    "${kind.payloadType.replace(/V1$/, '')}PayloadV1",`,
  )
  .join('\n')}
]
`;
}

function goName(constant) {
  return constant
    .toLowerCase()
    .split('_')
    .map((part) => part[0].toUpperCase() + part.slice(1))
    .join('');
}

function goDeclaration(definition) {
  const { version, schema, maxMessageBytes, kinds } = definition;
  const goConstants = kinds.map((kind) => `EvaluationWire${goName(kind.constant)}`);
  const constantWidth = Math.max(...goConstants.map((name) => name.length)) + 1;
  const constants = kinds
    .map(
      (kind) =>
        `\t${(`EvaluationWire${goName(kind.constant)}`).padEnd(constantWidth)}EvaluationWireKindV1 = "${kind.wireName}"`,
    )
    .join('\n');
  const catalog = kinds
    .map((kind) => `\tEvaluationWire${goName(kind.constant)},`)
    .join('\n');
  const payloadAliases = kinds
    .map(
      (kind) =>
        `// ${kind.payloadType} is preserved as JSON so hosts can apply their own typed adapter.\ntype ${kind.payloadType.replace(/V1$/, '')}PayloadV1 = json.RawMessage`,
    )
    .join('\n\n');
  const goFields = [
    ['Schema', 'string', '`json:"schema"`'],
    ['Version', 'uint16', '`json:"version"`'],
    ['Kind', 'EvaluationWireKindV1', '`json:"kind"`'],
    ['Payload', 'json.RawMessage', '`json:"payload"`'],
  ];
  const fieldNameWidth = Math.max(...goFields.map(([name]) => name.length)) + 1;
  const fieldTypeWidth = Math.max(...goFields.map(([, type]) => type.length)) + 1;
  const structFields = goFields
    .map(
      ([name, type, tag]) =>
        `\t${name.padEnd(fieldNameWidth)}${type.padEnd(fieldTypeWidth)}${tag}`,
    )
    .join('\n');
  return `// Code generated from core/src/evaluation/protocol.rs; DO NOT EDIT.
//
// Run: node scripts/generate_evaluation_protocol_artifacts.mjs

package code

import (
\t"bytes"
\t"encoding/json"
\t"fmt"
\t"io"
)

const EvaluationProtocolVersionV1 = ${version}
const EvaluationProtocolSchemaV1 = "${schema}"
const EvaluationProtocolMaxMessageBytes = ${maxMessageBytes}

// EvaluationWireKindV1 is the closed top-level payload catalog accepted by
// Core. Payload bytes remain opaque until a host chooses a typed adapter.
type EvaluationWireKindV1 string

const (
${constants}
)

var evaluationWireKindsV1 = [...]EvaluationWireKindV1{
${catalog}
}

// EvaluationWireKindsV1 returns the ordered version-${version} catalog.
func EvaluationWireKindsV1() []EvaluationWireKindV1 {
\treturn append([]EvaluationWireKindV1(nil), evaluationWireKindsV1[:]...)
}

// EvaluationWireEnvelopeV1 is the strict JSON transport shape shared by Core
// and the SDKs. Core validates payload fields before admission.
type EvaluationWireEnvelopeV1 struct {
${structFields}
}

// Validate checks the envelope identity and the closed kind catalog. Core
// remains responsible for validating the concrete payload fields.
func (envelope EvaluationWireEnvelopeV1) Validate() error {
\tif envelope.Schema != EvaluationProtocolSchemaV1 {
\t\treturn fmt.Errorf("unsupported evaluation wire schema %q", envelope.Schema)
\t}
\tif envelope.Version != EvaluationProtocolVersionV1 {
\t\treturn fmt.Errorf("unsupported evaluation wire version %d", envelope.Version)
\t}
\ttrimmed := bytes.TrimSpace(envelope.Payload)
\tif len(trimmed) == 0 || bytes.Equal(trimmed, []byte("null")) {
\t\treturn fmt.Errorf("evaluation wire payload is required")
\t}
\tfor _, known := range evaluationWireKindsV1 {
\t\tif envelope.Kind == known {
\t\t\treturn nil
\t\t}
\t}
\treturn fmt.Errorf("unknown evaluation wire kind %q", envelope.Kind)
}

// DecodeEvaluationWireEnvelopeV1 rejects unknown top-level fields, unsupported
// versions, unknown kinds, trailing JSON, and oversized messages.
func DecodeEvaluationWireEnvelopeV1(data []byte) (EvaluationWireEnvelopeV1, error) {
\tvar envelope EvaluationWireEnvelopeV1
\tif len(data) > EvaluationProtocolMaxMessageBytes {
\t\treturn envelope, fmt.Errorf("evaluation wire message exceeds %d bytes", EvaluationProtocolMaxMessageBytes)
\t}
\tdecoder := json.NewDecoder(bytes.NewReader(data))
\tdecoder.DisallowUnknownFields()
\tif err := decoder.Decode(&envelope); err != nil {
\t\treturn envelope, err
\t}
\tvar trailing any
\tif err := decoder.Decode(&trailing); err != io.EOF {
\t\tif err == nil {
\t\t\treturn envelope, fmt.Errorf("evaluation wire message has trailing JSON")
\t\t}
\t\treturn envelope, err
\t}
\tif err := envelope.Validate(); err != nil {
\t\treturn envelope, err
\t}
\treturn envelope, nil
}

${payloadAliases}
`;
}

function manifest(definition) {
  return `${JSON.stringify(
    {
      schema: definition.schema,
      version: definition.version,
      max_message_bytes: definition.maxMessageBytes,
      kinds: definition.kinds.map(({ variant, constant, wireName, payloadType }) => ({
        variant,
        constant,
        wire_name: wireName,
        payload_type: payloadType,
      })),
    },
    null,
    2,
  )}\n`;
}

function fixtures(definition) {
  const valid = {
    schema: definition.schema,
    version: definition.version,
    kind: 'evidence_read_request',
    payload: {
      target: {
        schema: 'a3s.code.execution-target.v1',
        session_id: 'fixture-session',
        run_id: 'fixture-run',
      },
    },
  };
  return `${JSON.stringify(
    {
      schema: definition.schema,
      version: definition.version,
      valid,
      unknown_top_level_field: { ...valid, future_field: true },
      unknown_payload_field: {
        ...valid,
        payload: { ...valid.payload, future_field: true },
      },
      unsupported_version: { ...valid, version: definition.version + 1 },
    },
    null,
    2,
  )}\n`;
}

const definition = protocolDefinition();
const outputs = new Map([
  ['sdk/node/evaluation-protocol-v1.d.ts', nodeDeclaration(definition)],
  ['sdk/python/python/a3s_code/evaluation_protocol_v1.py', pythonDeclaration(definition)],
  ['sdk/go/evaluation_protocol_v1.go', goDeclaration(definition)],
  ['sdk/evaluation/evaluation-wire-v1.json', manifest(definition)],
  ['sdk/evaluation/evaluation-wire-v1-fixtures.json', fixtures(definition)],
]);

let stale = false;
for (const [relativePath, expected] of outputs) {
  const absolutePath = path.join(root, relativePath);
  let actual = null;
  try {
    actual = read(relativePath);
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }

  if (actual !== null && normalizeLineEndings(actual) === normalizeLineEndings(expected)) {
    continue;
  }
  stale = true;
  if (checkOnly) {
    console.error(`${relativePath} is stale or missing`);
  } else {
    writeFileSync(absolutePath, expected);
    console.log(`generated ${relativePath}`);
  }
}

if (checkOnly && stale) {
  console.error('run: node scripts/generate_evaluation_protocol_artifacts.mjs');
  process.exitCode = 1;
} else if (checkOnly) {
  console.log(`evaluation protocol artifacts aligned (${definition.kinds.length} kinds)`);
}
