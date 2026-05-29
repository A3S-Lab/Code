// Smoke test for the Agent / Session close surface exposed by the
// core in steps 1–3 and propagated through the NAPI bindings in step 4.
//
// Run with:
//   node sdk/node/test_session_close.mjs
// (no provider credentials needed — uses inline ACL).

import assert from 'node:assert/strict'
import os from 'node:os'
import path from 'node:path'
import fs from 'node:fs'
import mod from './index.js'

const tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-node-close-'))
const workspace = path.join(tmpRoot, 'workspace')
fs.mkdirSync(workspace, { recursive: true })

const inlineConfig = `
default_model = "anthropic/claude-sonnet-4-20250514"

providers "anthropic" {
  api_key = "test-key"
  models "claude-sonnet-4-20250514" {
    name = "Claude Sonnet 4"
  }
}
`.trim()

const agent = await mod.Agent.create(inlineConfig)

function makeSession(sessionId) {
  return agent.session(workspace, {
    sessionId,
    permissionPolicy: { defaultDecision: 'allow' },
    workspaceBackend: new mod.LocalWorkspaceBackend(workspace),
  })
}

// 1. Fresh session: isClosed is false; agent.listSessions sees it.
const sessionA = makeSession('node-close-1')
assert.equal(sessionA.isClosed(), false, 'fresh session should not be closed')

const listedBefore = await agent.listSessions()
assert.ok(
  listedBefore.includes('node-close-1'),
  `agent.listSessions() should include node-close-1, got ${JSON.stringify(listedBefore)}`,
)

// 2. session.close() flips isClosed and is idempotent.
sessionA.close()
assert.equal(sessionA.isClosed(), true, 'session.close() must set isClosed = true')
sessionA.close() // second close must not throw
assert.equal(sessionA.isClosed(), true)

// 3. agent.closeSession(id) on a new live session closes it.
const sessionB = makeSession('node-close-2')
assert.equal(sessionB.isClosed(), false)
const wasOpen = await agent.closeSession('node-close-2')
assert.equal(
  wasOpen,
  true,
  `closeSession() on a live session must return true, got ${wasOpen}`,
)
assert.equal(
  sessionB.isClosed(),
  true,
  'closeSession() must propagate to the JS wrapper\'s isClosed view',
)

// 4. closeSession() on an unknown id returns false, doesn't throw.
const unknown = await agent.closeSession('does-not-exist')
assert.equal(
  unknown,
  false,
  `closeSession() on unknown id must return false, got ${unknown}`,
)

// 5. agent.close() closes every live session and rejects new session().
const sessionC = makeSession('node-close-3')
const sessionD = makeSession('node-close-4')
assert.equal(sessionC.isClosed(), false)
assert.equal(sessionD.isClosed(), false)

await agent.close()
assert.equal(agent.isClosed(), true, 'agent.isClosed() must be true after agent.close()')
assert.equal(sessionC.isClosed(), true, 'agent.close() must close sessionC')
assert.equal(sessionD.isClosed(), true, 'agent.close() must close sessionD')

let threw = false
try {
  makeSession('node-close-post')
} catch (err) {
  threw = true
  const msg = String(err).toLowerCase()
  assert.ok(
    msg.includes('closed'),
    `post-close session() error must mention 'closed', got: ${err}`,
  )
}
assert.equal(threw, true, 'session() after agent.close() must throw')

// disconnectIdleMcp is exposed and returns an array (empty here — the
// inline config registers no MCP servers). Call on a fresh agent since
// the one above is closed.
{
  const agent2 = await mod.Agent.create(inlineConfig)
  const dropped = await agent2.disconnectIdleMcp(5 * 60 * 1000)
  assert.ok(Array.isArray(dropped), 'disconnectIdleMcp must return an array')
  assert.equal(dropped.length, 0, 'no MCP servers configured -> nothing dropped')
}

console.log('node sdk session close api ok')
