#!/usr/bin/env node

import { readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const checkOnly = process.argv.includes('--check');

function read(relativePath) {
  return readFileSync(path.join(root, relativePath), 'utf8');
}

function protocolDefinition() {
  const source = read('core/src/event_protocol.rs');
  const versionMatch = source.match(/pub const EVENT_ENVELOPE_V1_VERSION: u16 = (\d+);/);
  if (!versionMatch) {
    throw new Error('could not find EVENT_ENVELOPE_V1_VERSION');
  }

  const catalogMatch = source.match(/define_agent_event_types_v1!\s*\{([\s\S]*?)\n\}/);
  if (!catalogMatch) {
    throw new Error('could not find define_agent_event_types_v1! catalog');
  }

  const events = [];
  const eventPattern = /^\s*([A-Za-z0-9_]+)\s*=>\s*([A-Z0-9_]+)\s*=\s*"([a-z0-9_]+)",?\s*$/gm;
  for (const match of catalogMatch[1].matchAll(eventPattern)) {
    events.push({ variant: match[1], constant: match[2], wireName: match[3] });
  }
  if (events.length === 0) {
    throw new Error('event protocol catalog is empty');
  }

  for (const key of ['variant', 'constant', 'wireName']) {
    const values = events.map(event => event[key]);
    if (new Set(values).size !== values.length) {
      throw new Error(`event protocol catalog has duplicate ${key} values`);
    }
  }

  return { version: Number(versionMatch[1]), events };
}

function nodeDeclaration({ version, events }) {
  const literals = events.map(event => `  | '${event.wireName}'`).join('\n');
  return `/**
 * Generated from core/src/event_protocol.rs.
 * Run \`node scripts/generate_event_protocol_artifacts.mjs\` to update.
 */

import type { AgentEvent } from './generated'

/** Event types defined by envelope version ${version}. */
export type KnownAgentEventTypeV1 =
${literals}

/**
 * Open event discriminant. Known values retain autocomplete while unknown
 * future values remain representable.
 */
export type AgentEventTypeV1 = KnownAgentEventTypeV1 | (string & {})

/** Stable, lossless event envelope shared by the core and SDKs. */
export interface EventEnvelopeV1<TPayload = unknown, TMetadata = unknown> {
  readonly version: ${version}
  readonly type: AgentEventTypeV1
  readonly payload: TPayload
  readonly metadata?: TMetadata
}

/** AgentEvent convenience fields combined with the strict v1 envelope. */
export type AgentEventV1<TPayload = unknown, TMetadata = unknown> =
  Omit<AgentEvent, 'version' | 'type' | 'payload' | 'metadata'>
  & EventEnvelopeV1<TPayload, TMetadata>
`;
}

function pythonDeclaration({ version, events }) {
  const literals = events.map(event => `    "${event.wireName}",`).join('\n');
  const tuple = events.map(event => `    "${event.wireName}",`).join('\n');
  const constants = events
    .map(event => `    ${event.constant}: Final[str] = "${event.wireName}"`)
    .join('\n');
  return `"""Generated version-1 event protocol declarations.

Generated from core/src/event_protocol.rs. Run
node scripts/generate_event_protocol_artifacts.mjs to update.
"""

from typing import Final, Literal, Tuple

EVENT_ENVELOPE_V1_VERSION: Final[int] = ${version}

KnownAgentEventTypeV1 = Literal[
${literals}
]

# Event types are open for forward compatibility. Use the known alias when an
# exhaustive catalog is specifically required.
AgentEventTypeV1 = str

AGENT_EVENT_TYPES_V1: Final[Tuple[KnownAgentEventTypeV1, ...]] = (
${tuple}
)


class EventType:
    """Canonical string constants for AgentEvent.type."""

${constants}

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
`;
}

const definition = protocolDefinition();
const outputs = new Map([
  ['sdk/node/event-protocol-v1.d.ts', nodeDeclaration(definition)],
  ['sdk/python/python/a3s_code/event_protocol_v1.py', pythonDeclaration(definition)],
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

  if (actual === expected) continue;
  stale = true;
  if (checkOnly) {
    console.error(`${relativePath} is stale or missing`);
  } else {
    writeFileSync(absolutePath, expected);
    console.log(`generated ${relativePath}`);
  }
}

if (checkOnly && stale) {
  console.error('run: node scripts/generate_event_protocol_artifacts.mjs');
  process.exitCode = 1;
} else if (checkOnly) {
  console.log(`event protocol artifacts aligned (${definition.events.length} event types)`);
}
