// Smoke test for the Node SDK BudgetGuard bridge.
//
// Verifies that a JS guard whose checkBeforeLlm returns
// { decision: 'deny', ... } aborts session.send before the LLM is
// touched. Runs with `node sdk/node/test_budget_guard.mjs`.

import assert from 'node:assert/strict'
import os from 'node:os'
import path from 'node:path'
import fs from 'node:fs'
import mod from './index.js'

const tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-budget-'))
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

const session = agent.session(workspace, {
  sessionId: 'budget-deny-node',
  permissionPolicy: { defaultDecision: 'allow' },
  workspaceBackend: new mod.LocalWorkspaceBackend(workspace),
})

let llmChecks = 0
let llmRecords = 0
let toolChecks = 0

session.setBudgetGuard({
  checkBeforeLlm: (ctx) => {
    llmChecks += 1
    assert.equal(ctx.sessionId, 'budget-deny-node', `wrong session_id, got ${ctx.sessionId}`)
    assert.equal(typeof ctx.estimatedTokens, 'number', 'estimatedTokens must be a number')
    return { decision: 'deny', resource: 'llm_tokens', reason: 'cap hit' }
  },
  recordAfterLlm: (_ctx) => {
    llmRecords += 1
  },
  checkBeforeTool: (_ctx) => {
    toolChecks += 1
    return null
  },
})

let threw = false
try {
  await session.send('hello')
} catch (err) {
  threw = true
  const msg = String(err).toLowerCase()
  assert.ok(
    msg.includes('budget exhausted') || msg.includes('llm_tokens'),
    `expected budget-exhausted error, got: ${err}`,
  )
}
assert.equal(threw, true, 'send() must throw when checkBeforeLlm denies')
assert.equal(llmChecks, 1, `checkBeforeLlm must fire exactly once, got ${llmChecks}`)
assert.equal(llmRecords, 0, `recordAfterLlm must not fire on Deny, got ${llmRecords}`)
assert.equal(toolChecks, 0, 'no tool was attempted; toolChecks must stay 0')

// Clearing the guard restores Allow-default behaviour. The mock LLM
// configured by `test_session_close.mjs` is not present here, so a
// real send would still fail at the provider level — we just verify
// that setBudgetGuard(null) is accepted without error.
session.setBudgetGuard(null)

// Throw, timeout, malformed-return, and host-process survival regressions run
// independently in test_callback_safety.mjs.

await session.closeAsync()
await agent.close()
fs.rmSync(tmpRoot, { recursive: true, force: true })

console.log('node sdk budget guard ok')
