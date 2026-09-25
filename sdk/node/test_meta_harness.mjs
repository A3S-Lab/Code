// Meta Harness composition through the Node SDK (no provider credentials needed).
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import mod from './index.js'

const { Agent, Harness, LocalWorkspaceBackend } = mod
const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-node-harness-'))

try {
  const recipe = Harness.compose({
    components: [Harness.system(), Harness.tools(), Harness.host('intent_stamp'), Harness.budget(), Harness.infer()],
    toolBudget: 4,
    system: ['careful coding agent'],
  })
  assert.deepEqual(recipe.components, ['system', 'tools', 'host:intent_stamp', 'budget', 'infer'])
  assert.equal(recipe.toolBudget, 4)

  assert.throws(() => Harness.compose({ components: ['system', 'not-a-part'] }), /not-a-part|unknown/i)

  const agent = await Agent.create(`
default_model = "anthropic/claude-sonnet-4-20250514"

providers "anthropic" {
  api_key = "test-key"
  models "claude-sonnet-4-20250514" {
    name = "Claude Sonnet 4"
  }
}
`.trim())
  const session = agent.session(workspace, {
    harness: recipe,
    permissionPolicy: { defaultDecision: 'allow' },
    workspaceBackend: new LocalWorkspaceBackend(workspace),
  })
  assert.equal(session.isClosed(), false)
  session.close()
  console.log('meta harness ok')
} finally {
  fs.rmSync(workspace, { recursive: true, force: true })
}
