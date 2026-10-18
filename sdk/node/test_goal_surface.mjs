// Durable goal and work-policy surface: Core parses every command and
// refuses identically to the Rust, Python and Go hosts.
//
// Run with: node sdk/node/test_goal_surface.mjs (no credentials needed).

import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import mod from './index.js'

const agent = await mod.Agent.create(`
default_model = "anthropic/test-model"
providers "anthropic" {
  api_key = "test-key"
  models "test-model" { name = "Test Model" }
}
`.trim())
const workspace = () => fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-node-goal-'))

assert.throws(
  () => agent.session(workspace(), { workPolicy: { id: '', revision: 1, instructions: 'x' } }),
  /workPolicy/,
)
for (const workPolicy of [undefined, { id: 'eng', revision: 1, instructions: 'Make the smallest complete change.' }]) {
  const session = agent.session(workspace(), { workPolicy })
  const digest = session.workPolicyDigest()
  if (workPolicy === undefined) assert.equal(digest, null)
  else assert.ok(typeof digest === 'string' && digest.length > 0, 'policy is admitted at session build')
  assert.equal(session.workPolicyDigest(), digest)
  assert.equal(await session.currentRun(), null, 'policy admission starts no Run')
  session.close()
}

const session = agent.session(workspace(), { sessionId: 'node-goal' })
const ws = { id: session.workspace, display_root: session.workspace }
assert.equal(session.goalDefinition(), null)
const create = {
  kind: 'create', operation_id: 'create', goal_id: 'release', session_id: session.sessionId, workspace: ws,
  goal: {
    description: 'Report the release code', success_criteria: ['Reported exactly'],
    progress: 0, achieved: false, created_at: Math.floor(Date.now() / 1000), achieved_at: null,
  },
  non_goals: ['Do not change files'], scope_fact_sequences: [], work_policy_digest: null,
  expected_goal_revision: null, expected_plan_revision: null, expected_goal_sequence: null,
}
assert.throws(() => session.applyGoalDefinition({ ...create, unexpected: true }), /unknown field/)
assert.equal(session.goalDefinition(), null, 'a refused command records nothing')

const created = session.applyGoalDefinition(create)
assert.equal(created.state, 'active')
assert.deepEqual(session.goalDefinition(), created)
const ref = (kind, operation_id, view) => ({
  kind, operation_id, goal_id: view.binding.goal_id, session_id: view.binding.session_id,
  workspace: view.binding.workspace, work_policy_digest: null,
  expected_goal_revision: view.binding.goal_revision, expected_plan_revision: view.plan_revision,
  expected_goal_sequence: view.goal_sequence,
})
const proposed = session.applyGoalPlan({
  ...ref('propose_plan', 'propose', created), proposal_sequence: null,
  plan: {
    goal: 'Release check', complexity: 'Simple', required_tools: [], estimated_steps: 1,
    steps: [{ id: 'report', content: 'Report the release code', status: 'pending', priority: 'medium', success_criteria: 'Reported exactly' }],
  },
})
const accepted = session.applyGoalPlan({
  ...ref('accept_plan', 'accept', proposed), plan: null, proposal_sequence: proposed.plan_fact_sequence,
})
assert.equal(accepted.plan_accepted, true)

// Without trusted host goal_control bounds a ready step never advances.
const tick = ref('tick', 'tick-1', accepted)
assert.equal(await session.previewGoalTick(tick), 'budget_exhausted')
assert.deepEqual(await session.tickGoal(tick), { decision: 'budget_exhausted' })
const streamed = await session.streamGoalTick(tick)
assert.equal(streamed.decision, 'budget_exhausted')
assert.equal(streamed.started ?? null, null)
assert.equal(streamed.replayed ?? null, null)

assert.equal(session.applyGoalControl(ref('pause', 'pause', accepted)).state, 'paused')
assert.throws(() => session.applyGoalControl(ref('resume', 'resume', accepted)), /goal\./)
assert.equal(session.reconcileGoalSlices(), 0)
session.close()

// Fork a saved SessionSnapshotV1; Core rejects a malformed one.
const store = workspace()
const source = agent.session(workspace(), { sessionId: 'node-fork-src', sessionStore: new mod.FileSessionStore(store) })
await source.save()
const sessionsDir = path.join(store, 'v1', 'sessions')
const [snapshotFile] = fs.readdirSync(sessionsDir).filter((name) => name.endsWith('.json'))
const snapshot = JSON.parse(fs.readFileSync(path.join(sessionsDir, snapshotFile), 'utf8'))
const forked = await agent.forkSessionAsync(snapshot, workspace(), { sessionId: 'node-fork' })
assert.equal(forked.sessionId, 'node-fork')
assert.equal(forked.goalDefinition(), null, 'source Goal facts stay at the source')
await assert.rejects(agent.forkSessionAsync({}, workspace(), {}), /schema_version/)
forked.close()
source.close()
console.log('node goal surface ok')
