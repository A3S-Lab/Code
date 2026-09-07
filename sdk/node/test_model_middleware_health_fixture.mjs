import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import mod from './index.js'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const fixture = JSON.parse(
  await readFile(path.join(root, 'evaluation', 'model-middleware-health-v1.json'), 'utf8'),
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

function assertSnapshot(health) {
  for (const field of fixture.required_snapshot_fields) {
    assert.ok(field in health, `missing snapshot field ${field}`)
    assert.equal(health[field], 0, `${field} must start at zero`)
  }
  assertNoForbiddenFields(health)
}

const workspace = await mkdtemp(path.join(tmpdir(), 'a3s-node-middleware-health-'))
try {
  const agent = await mod.Agent.create(INLINE_CONFIG)
  const session = await agent.session(workspace, {
    sessionId: 'node-middleware-health-fixture',
  })
  try {
    const health = session.modelMiddlewareHealth()
    assertSnapshot(health)
  } finally {
    await session.close()
    await agent.close()
  }
} finally {
  await rm(workspace, { recursive: true, force: true })
}

console.log('model middleware health fixture ok')
