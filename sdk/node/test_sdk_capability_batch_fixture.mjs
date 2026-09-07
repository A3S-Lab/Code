import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import mod from './index.js'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const fixture = JSON.parse(
  await readFile(path.join(root, 'evaluation', 'sdk-capability-batch-v1.json'), 'utf8'),
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
assert.equal(fixture.fixture_id, 'sdk-capability-batch-v1')
for (const field of fixture.required_batch_fields) {
  assert.ok(field in fixture.sample_batch, `missing batch field ${field}`)
}

const workspace = await mkdtemp(path.join(tmpdir(), 'a3s-node-sdk-cap-batch-'))
try {
  const agent = await mod.Agent.create(INLINE_CONFIG)
  const session = await agent.session(workspace, {
    sessionId: 'node-sdk-capability-batch-fixture',
  })
  try {
    const receipt = await session.applyCapabilityBatch(fixture.sample_batch)
    assert.equal(
      receipt.previousGeneration,
      fixture.expected_receipt.previousGeneration,
    )
    assert.equal(
      receipt.committedGeneration,
      fixture.expected_receipt.committedGeneration,
    )
    for (const field of fixture.required_receipt_fields) {
      assert.ok(field in receipt, `missing receipt field ${field}`)
    }
    assertNoForbiddenFields(receipt)
    const stamp = session.capabilityCatalogStamp()
    assert.equal(stamp.generation, 1)
  } finally {
    await session.close()
    await agent.close()
  }
} finally {
  await rm(workspace, { recursive: true, force: true })
}

console.log('sdk capability batch fixture ok')
