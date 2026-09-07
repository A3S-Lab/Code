import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import mod from './index.js'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const fixture = JSON.parse(
  await readFile(path.join(root, 'evaluation', 'sdk-checkpoint-export-v1.json'), 'utf8'),
)

const INLINE_CONFIG = `
default_model = "openai/fixture-model"
providers "openai" {
  apiKey = "fixture-key-never-sent"
  baseUrl = "https://fixture.invalid/v1"
  models "fixture-model" {
    name = "Fixture Model"
  }
}
`.trim()

function assertNoForbiddenFields(value) {
  const forbidden = new Set(fixture.forbidden_fields)
  if (Array.isArray(value)) {
    for (const child of value) {
      assertNoForbiddenFields(child)
    }
    return
  }
  if (value && typeof value === 'object') {
    for (const [key, child] of Object.entries(value)) {
      assert.equal(forbidden.has(key), false, `forbidden diagnostic field ${key}`)
      assertNoForbiddenFields(child)
    }
  }
}

assert.equal(fixture.schema_version, 1)
assert.equal(fixture.fixture_id, 'sdk-checkpoint-export-v1')
for (const field of fixture.required_export_fields) {
  assert.ok(field in fixture.sample_export, `missing export field ${field}`)
}
for (const field of fixture.required_descriptor_fields) {
  assert.ok(field in fixture.sample_export.descriptor, `missing descriptor field ${field}`)
}
assertNoForbiddenFields(fixture.sample_export)

const workspace = await mkdtemp(path.join(tmpdir(), 'a3s-node-sdk-cp-export-'))
try {
  const agent = await mod.Agent.create(INLINE_CONFIG)
  const session = await agent.session(workspace, {
    sessionId: 'node-sdk-checkpoint-export-fixture',
  })
  try {
    assert.equal(typeof session.setSessionCheckpointExportSink, 'function')
    let installed = false
    session.setSessionCheckpointExportSink((exportPayload) => {
      installed = true
      assert.ok(exportPayload.descriptor)
      assert.ok(typeof exportPayload.contentBase64 === 'string')
      return { ok: true }
    }, 5_000)
    assert.equal(installed, false, 'install must not invoke the sink eagerly')
    session.setSessionCheckpointExportSink(null)
  } finally {
    await session.close()
    await agent.close()
  }
} finally {
  await rm(workspace, { recursive: true, force: true })
}

console.log('sdk checkpoint export fixture ok')
