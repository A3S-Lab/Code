#!/usr/bin/env node

/** Cross-language parity check for generated evaluation wire declarations. */

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

function read(relativePath) {
  return readFileSync(path.join(root, relativePath), 'utf8');
}

const manifest = JSON.parse(read('sdk/evaluation/evaluation-wire-v1.json'));
const fixtures = JSON.parse(read('sdk/evaluation/evaluation-wire-v1-fixtures.json'));
assert.equal(manifest.schema, 'a3s.code.evaluation-wire.v1');
assert.equal(manifest.version, 1);
assert.equal(manifest.max_message_bytes, 32 * 1024 * 1024);
assert.equal(manifest.kinds.length, 7);
assert.equal(fixtures.schema, manifest.schema);
assert.equal(fixtures.version, manifest.version);
assert.equal(fixtures.valid.schema, manifest.schema);
assert.equal(fixtures.valid.version, manifest.version);
assert.equal(fixtures.valid.kind, 'evidence_read_request');
assert.equal(fixtures.unknown_top_level_field.future_field, true);
assert.equal(fixtures.unknown_payload_field.payload.future_field, true);
assert.equal(fixtures.unsupported_version.version, manifest.version + 1);

const node = read('sdk/node/evaluation-protocol-v1.d.ts');
const python = read('sdk/python/python/a3s_code/evaluation_protocol_v1.py');
const go = read('sdk/go/evaluation_protocol_v1.go');

for (const source of [node, python, go]) {
  assert.match(source, /a3s\.code\.evaluation-wire\.v1/);
  assert.match(source, /33554432/);
  for (const kind of manifest.kinds) {
    assert.ok(
      source.includes(kind.wire_name),
      `missing ${kind.wire_name} in generated SDK projection`,
    );
  }
}

const names = manifest.kinds.map((kind) => kind.wire_name);
assert.equal(new Set(names).size, names.length);
console.log(`evaluation protocol SDK parity aligned (${names.length} kinds)`);
