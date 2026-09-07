import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import crypto from 'node:crypto'
import mod from './index.js'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const fixture = JSON.parse(
  await readFile(path.join(root, 'evaluation', 'sdk-immutable-content-v1.json'), 'utf8'),
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
assert.equal(fixture.fixture_id, 'sdk-immutable-content-v1')
for (const field of fixture.required_write_request_fields) {
  assert.ok(field in fixture.sample_write_request, `missing write field ${field}`)
}
for (const field of fixture.required_reference_fields) {
  assert.ok(field in fixture.sample_reference, `missing reference field ${field}`)
}
assertNoForbiddenFields(fixture.sample_write_request)
assertNoForbiddenFields(fixture.sample_reference)

const workspace = await mkdtemp(path.join(tmpdir(), 'a3s-node-sdk-imm-'))
try {
  const agent = await mod.Agent.create(INLINE_CONFIG)
  assert.equal(typeof mod.ImmutableContentAdapterOptions, 'function')
  const authorityDigest = `sha256:${'a'.repeat(64)}`
  const adapter = new mod.ImmutableContentAdapterOptions(
    authorityDigest,
    4096,
    'node-sdk-imm1',
    async (request) => {
      assert.ok(request.binding)
      assert.ok(request.descriptor)
      assert.equal(typeof request.contentBase64, 'string')
      const content = Buffer.from(request.contentBase64, 'base64')
      const digest = crypto.createHash('sha256').update(content).digest('hex')
      return {
        schema: 'a3s.code.immutable-content-reference.v1',
        binding_digest: request.binding.binding_digest,
        uri: `a3s+test://node-sdk-imm1/${digest}`,
        content_digest: `sha256:${digest}`,
        media_type: request.descriptor.media_type,
        size_bytes: content.length,
        reference_digest: `sha256:${'b'.repeat(64)}`,
      }
    },
    5_000,
  )
  const session = await agent.session(workspace, {
    sessionId: 'node-sdk-immutable-content-fixture',
    immutableContentAdapter: adapter,
  })
  try {
    assert.ok(session)
  } finally {
    await session.close()
    await agent.close()
  }
} finally {
  await rm(workspace, { recursive: true, force: true })
}

console.log('sdk immutable content fixture ok')
