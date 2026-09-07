import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import mod from './index.js'

const fixture = JSON.parse(fs.readFileSync(
  new URL('../evaluation/model-generation-pool-health-v1.json', import.meta.url),
  'utf8',
))
assert.equal(fixture.schema_version, 1)
assert.equal(fixture.report_schema_version, 1)

const root = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-node-pool-health-'))
const config = `
default_model = "openai/fixture-model"
providers "openai" {
  apiKey = "fixture-key-never-sent"
  baseUrl = "https://fixture.invalid/v1"
  models "fixture-model" {
    name = "Fixture Model"
  }
}
`.trim()

const agent = await mod.Agent.create(config)
const session = agent.session(root, {
  sessionId: 'node-pool-health-fixture',
  planningMode: 'disabled',
})

try {
  const snapshots = []
  for (let index = 0; index < Math.min(fixture.sample_limit, 3); index += 1) {
    const health = await session.modelGenerationPoolHealth()
    assert.ok(health, 'configured provider must publish pool health')
    snapshots.push(health)
    for (const field of fixture.required_snapshot_fields) {
      assert.ok(Object.hasOwn(health, field), `missing snapshot field ${field}`)
    }
    assert.ok(health.pool.maxConcurrency > 0)
    assert.ok(health.pool.maxConcurrency <= fixture.max_concurrency)
    assert.equal(
      health.localReserved + health.localAvailable,
      health.localMaxConcurrency,
    )
    assert.ok(health.localMaxConcurrency <= health.pool.maxConcurrency)
    for (const field of fixture.required_identity_fields) {
      assert.ok(Object.hasOwn(health.pool.identity, field), `missing identity field ${field}`)
    }
    assert.equal(
      health.pool.identity.domain,
      'a3s.code.model-generation-pool.identity.v1',
    )
    assert.match(health.pool.identity.digest, /^sha256:[0-9a-f]{64}$/)
    if (health.scheduler) {
      assert.deepEqual(health.scheduler.identity, health.pool.identity)
      assert.equal(health.scheduler.maxActive, health.pool.maxConcurrency)
      assert.ok(health.scheduler.active <= health.scheduler.maxActive)
      assert.ok(health.scheduler.pending <= health.scheduler.maxActive)
    }
  }
  assert.equal(snapshots.length, Math.min(fixture.sample_limit, 3))

  const aggregate = {
    sampleCount: snapshots.length,
    maxLocalReserved: Math.max(...snapshots.map((value) => value.localReserved)),
    maxSchedulerActive: Math.max(...snapshots.map((value) => value.scheduler?.active ?? 0)),
    maxSchedulerPending: Math.max(...snapshots.map((value) => value.scheduler?.pending ?? 0)),
    admitted: Math.max(...snapshots.map((value) => value.scheduler?.admitted ?? 0)),
    released: Math.max(...snapshots.map((value) => value.scheduler?.released ?? 0)),
    cancelled: Math.max(...snapshots.map((value) => value.scheduler?.cancelled ?? 0)),
    rejected: Math.max(...snapshots.map((value) => value.scheduler?.rejected ?? 0)),
  }
  assert.deepEqual(
    Object.keys(aggregate).sort(),
    [...fixture.aggregate_fields].sort(),
  )
  const forbidden = new Set(fixture.forbidden_fields)
  const walk = (value) => {
    if (Array.isArray(value)) {
      value.forEach(walk)
      return
    }
    if (value && typeof value === 'object') {
      for (const [key, child] of Object.entries(value)) {
        assert.equal(forbidden.has(key), false, `forbidden diagnostic field ${key}`)
        walk(child)
      }
    }
  }
  walk({ snapshots, aggregate })
} finally {
  await session.closeAsync()
  await agent.close()
}

console.log('node model-generation pool health fixture passed')
