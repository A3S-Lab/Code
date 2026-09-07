#!/usr/bin/env node

/**
 * Generate the language projections for the research wire envelope.
 *
 * Rust remains the source of truth: the generator reads the version, schema,
 * size bound, and one-line kind catalog from core/src/research/protocol.rs.
 * The generated SDK declarations intentionally keep payloads as opaque JSON
 * objects/bytes. Core performs the closed, typed payload validation; hosts and
 * SDKs must not invent a second scientific policy or business schema.
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
  const source = read('core/src/research/protocol.rs');
  const versionMatch = source.match(
    /pub const RESEARCH_PROTOCOL_VERSION_V1: u16 = (\d+);/,
  );
  const schemaMatch = source.match(
    /pub const RESEARCH_PROTOCOL_SCHEMA_V1: &str = "([^"]+)";/,
  );
  const maxMessageMatch = source.match(
    /pub const RESEARCH_PROTOCOL_MAX_MESSAGE_BYTES: usize = ([^;]+);/,
  );
  const catalogMatch = source.match(
    /define_research_wire_kinds_v1!\s*\{([\s\S]*?)\n\}/,
  );
  if (!versionMatch || !schemaMatch || !maxMessageMatch || !catalogMatch) {
    throw new Error('could not find research protocol definition');
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
    throw new Error('research protocol catalog is empty');
  }

  for (const key of ['variant', 'constant', 'wireName', 'payloadType']) {
    const values = kinds.map((kind) => kind[key]);
    if (new Set(values).size !== values.length) {
      throw new Error(`research protocol catalog has duplicate ${key} values`);
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
        `  | (ResearchWireEnvelopeV1<${kind.payloadType.replace(/V1$/, '')}PayloadV1> & { readonly kind: '${kind.wireName}' })`,
    )
    .join('\n');
  const constants = kinds
    .map(
      (kind) =>
        `  ${kind.constant}: '${kind.wireName}',`,
    )
    .join('\n');
  return `/**
 * Generated from core/src/research/protocol.rs.
 * Run \`node scripts/generate_research_protocol_artifacts.mjs\` to update.
 *
 * Payloads remain opaque JSON objects at the SDK boundary. Rust Core owns the
 * closed payload schemas and validation; hosts own business/reviewer meaning.
 */

export const RESEARCH_PROTOCOL_VERSION_V1 = ${version} as const
export const RESEARCH_PROTOCOL_SCHEMA_V1 = '${schema}' as const
export const RESEARCH_PROTOCOL_MAX_MESSAGE_BYTES = ${maxMessageBytes} as const

/** Closed top-level kinds accepted by research wire version ${version}. */
export type KnownResearchWireKindV1 =
${literals}

export type ResearchWireKindV1 = KnownResearchWireKindV1

export const ResearchWireTypeV1 = {
${constants}
} as const

export const RESEARCH_WIRE_KINDS_V1 = [
${tuple}
] as const satisfies readonly KnownResearchWireKindV1[]

/** Strict envelope shared by Core and all SDKs. */
export interface ResearchWireEnvelopeV1<TPayload = ResearchWirePayloadV1> {
  readonly schema: typeof RESEARCH_PROTOCOL_SCHEMA_V1
  readonly version: typeof RESEARCH_PROTOCOL_VERSION_V1
  readonly kind: ResearchWireKindV1
  readonly payload: TPayload
}

${payloadAliases}

/** Opaque JSON payload preserved for host-owned transport adapters. */
export type ResearchWirePayloadV1 = Readonly<Record<string, unknown>>

/** Union of all payload shapes known to Core at wire version ${version}. */
export type KnownResearchWirePayloadV1 =
${payloadUnion}

/** Discriminated message union for exhaustive SDK dispatch. */
export type ResearchWireMessageV1 =
${messageUnion}
`;
}

function pythonDeclaration(definition) {
  const { version, schema, maxMessageBytes, kinds } = definition;
  const literals = kinds.map((kind) => `    "${kind.wireName}",`).join('\n');
  const constants = kinds
    .map((kind) => `    ${kind.constant}: Final[str] = "${kind.wireName}"`)
    .join('\n');
  return `"""Generated research wire protocol declarations.

Generated from core/src/research/protocol.rs. Run
node scripts/generate_research_protocol_artifacts.mjs to update.

Payload values intentionally remain mappings: Core is the single authority for
closed payload validation, while hosts own transport and business semantics.
"""

from typing import Final, Literal, Mapping, Tuple, TypedDict

RESEARCH_PROTOCOL_VERSION_V1: Final[int] = ${version}
RESEARCH_PROTOCOL_SCHEMA_V1: Final[str] = "${schema}"
RESEARCH_PROTOCOL_MAX_MESSAGE_BYTES: Final[int] = ${maxMessageBytes}

KnownResearchWireKindV1 = Literal[
${literals}
]
ResearchWireKindV1 = KnownResearchWireKindV1

RESEARCH_WIRE_KINDS_V1: Final[Tuple[KnownResearchWireKindV1, ...]] = (
${literals}
)


class ResearchWireTypeV1:
    """Canonical string constants for research wire version ${version}."""

${constants}


ResearchWirePayloadV1 = Mapping[str, object]
${kinds
  .map(
    (kind) =>
      `${kind.payloadType.replace(/V1$/, '')}PayloadV1 = ResearchWirePayloadV1`,
  )
  .join('\n')}


class ResearchWireEnvelopeV1(TypedDict):
    """Strict top-level envelope shape emitted by Code Core."""

    schema: str
    version: int
    kind: KnownResearchWireKindV1
    payload: ResearchWirePayloadV1


__all__ = [
    "RESEARCH_PROTOCOL_MAX_MESSAGE_BYTES",
    "RESEARCH_PROTOCOL_SCHEMA_V1",
    "RESEARCH_PROTOCOL_VERSION_V1",
    "RESEARCH_WIRE_KINDS_V1",
    "ResearchWireEnvelopeV1",
    "ResearchWireKindV1",
    "ResearchWirePayloadV1",
    "ResearchWireTypeV1",
    "KnownResearchWireKindV1",
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
  const goConstants = kinds.map((kind) => `ResearchWire${goName(kind.constant)}`);
  const constantWidth = Math.max(...goConstants.map((name) => name.length)) + 1;
  const constants = kinds
    .map(
      (kind) =>
        `\t${(`ResearchWire${goName(kind.constant)}`).padEnd(constantWidth)}ResearchWireKindV1 = "${kind.wireName}"`,
    )
    .join('\n');
  const catalog = kinds
    .map((kind) => `\tResearchWire${goName(kind.constant)},`)
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
    ['Kind', 'ResearchWireKindV1', '`json:"kind"`'],
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
  return `// Code generated from core/src/research/protocol.rs; DO NOT EDIT.
//
// Run: node scripts/generate_research_protocol_artifacts.mjs

package code

import (
\t"bytes"
\t"encoding/json"
\t"fmt"
\t"io"
)

const ResearchProtocolVersionV1 = ${version}
const ResearchProtocolSchemaV1 = "${schema}"
const ResearchProtocolMaxMessageBytes = ${maxMessageBytes}

// ResearchWireKindV1 is the closed top-level payload catalog accepted by
// Core. Payload bytes remain opaque until a host chooses a typed adapter.
type ResearchWireKindV1 string

const (
${constants}
)

var researchWireKindsV1 = [...]ResearchWireKindV1{
${catalog}
}

// ResearchWireKindsV1 returns the ordered version-${version} catalog.
func ResearchWireKindsV1() []ResearchWireKindV1 {
\treturn append([]ResearchWireKindV1(nil), researchWireKindsV1[:]...)
}

// ResearchWireEnvelopeV1 is the strict JSON transport shape shared by Core
// and the SDKs. Core validates payload fields before admission.
type ResearchWireEnvelopeV1 struct {
${structFields}
}

// Validate checks the envelope identity and the closed kind catalog. Core
// remains responsible for validating the concrete payload fields.
func (envelope ResearchWireEnvelopeV1) Validate() error {
\tif envelope.Schema != ResearchProtocolSchemaV1 {
\t\treturn fmt.Errorf("unsupported research wire schema %q", envelope.Schema)
\t}
\tif envelope.Version != ResearchProtocolVersionV1 {
\t\treturn fmt.Errorf("unsupported research wire version %d", envelope.Version)
\t}
\ttrimmed := bytes.TrimSpace(envelope.Payload)
\tif len(trimmed) == 0 || bytes.Equal(trimmed, []byte("null")) {
\t\treturn fmt.Errorf("research wire payload is required")
\t}
\tfor _, known := range researchWireKindsV1 {
\t\tif envelope.Kind == known {
\t\t\treturn nil
\t\t}
\t}
\treturn fmt.Errorf("unknown research wire kind %q", envelope.Kind)
}

// DecodeResearchWireEnvelopeV1 rejects unknown top-level fields, unsupported
// versions, unknown kinds, trailing JSON, and oversized messages.
func DecodeResearchWireEnvelopeV1(data []byte) (ResearchWireEnvelopeV1, error) {
\tvar envelope ResearchWireEnvelopeV1
\tif len(data) > ResearchProtocolMaxMessageBytes {
\t\treturn envelope, fmt.Errorf("research wire message exceeds %d bytes", ResearchProtocolMaxMessageBytes)
\t}
\tdecoder := json.NewDecoder(bytes.NewReader(data))
\tdecoder.DisallowUnknownFields()
\tif err := decoder.Decode(&envelope); err != nil {
\t\treturn envelope, err
\t}
\tvar trailing any
\tif err := decoder.Decode(&trailing); err != io.EOF {
\t\tif err == nil {
\t\t\treturn envelope, fmt.Errorf("research wire message has trailing JSON")
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
    kind: 'research_event',
    payload: {
      schema: 'a3s.code.science-event.v1',
      projectId: 'fixture-project',
      projectRevision: 1,
      runId: 'fixture-run',
      sequence: 1,
      eventType: 'research.run.admitted',
      payloadDigest: `sha256:${'1'.repeat(64)}`,
      observedAtMs: 1,
      eventDigest:
        'sha256:3107e7cc70926565f2bc43e0cf0c23ce6c0ba4ddd4ed76bc863677473bd47329',
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
  ['sdk/node/research-protocol-v1.d.ts', nodeDeclaration(definition)],
  ['sdk/python/python/a3s_code/research_protocol_v1.py', pythonDeclaration(definition)],
  ['sdk/go/research_protocol_v1.go', goDeclaration(definition)],
  ['sdk/research/research-wire-v1.json', manifest(definition)],
  ['sdk/research/research-wire-v1-fixtures.json', fixtures(definition)],
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
  console.error('run: node scripts/generate_research_protocol_artifacts.mjs');
  process.exitCode = 1;
} else if (checkOnly) {
  console.log(`research protocol artifacts aligned (${definition.kinds.length} kinds)`);
}
