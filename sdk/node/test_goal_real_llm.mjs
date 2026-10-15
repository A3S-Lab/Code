// Explicit live gate: A3S_CONFIG_FILE must name a credentialed ACL file.
// One real Run per session proves the four work-policy x goal modes.
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import mod from './index.js'

const config = process.env.A3S_CONFIG_FILE
assert.ok(config && fs.statSync(config).isFile(), 'A3S_CONFIG_FILE is required')
const agent = await mod.Agent.create(config)
const base = {
  planningMode: 'disabled', manualDelegationEnabled: false, autoDelegation: { enabled: false },
  continuationEnabled: false, maxToolRounds: 2, llmApiTimeoutMs: 120_000, sandboxNetwork: 'deny',
  permissionPolicy: { defaultDecision: 'deny' },
}
const prompt = 'Reply with exactly the word READY. Do not use tools.'
const workspace = () => fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-node-goal-live-'))
const create = (session, digest) => ({
  kind: 'create', operation_id: 'create', goal_id: 'release', session_id: session.sessionId,
  workspace: { id: session.workspace, display_root: session.workspace },
  goal: {
    description: 'Report the release code', success_criteria: ['Reported exactly'],
    progress: 0, achieved: false, created_at: Math.floor(Date.now() / 1000), achieved_at: null,
  },
  non_goals: [], scope_fact_sequences: [], work_policy_digest: digest,
  expected_goal_revision: null, expected_plan_revision: null, expected_goal_sequence: null,
})
const started = Date.now()

const plain = await agent.sessionAsync(workspace(), { ...base, sessionId: 'node-goal-live-plain' })
assert.match((await plain.send(prompt)).text, /READY/)
assert.equal(plain.workPolicyDigest(), null, 'no policy: a Run binds none')
assert.equal(plain.applyGoalDefinition(create(plain, null)).binding.work_policy_digest, null)
plain.close()

const workPolicy = { id: 'eng', revision: 1, instructions: 'Answer exactly what is asked and change nothing else.' }
const governed = await agent.sessionAsync(workspace(), { ...base, sessionId: 'node-goal-live-policy', workPolicy })
assert.equal(governed.workPolicyDigest(), null, 'configured but not yet admitted')
const result = await governed.send(prompt)
assert.match(result.text, /READY/)
const digest = governed.workPolicyDigest()
assert.match(digest ?? '', /\S/, 'the first Run admits the policy')
assert.equal(governed.goalDefinition(), null, 'policy without goal')
assert.throws(() => governed.applyGoalDefinition(create(governed, null)), /goal\./)
const bound = governed.applyGoalDefinition(create(governed, digest))
assert.equal(bound.binding.work_policy_digest, digest)
governed.close()

// Trusted host bounds admit one goal slice; its events arrive as a stream.
const bounded = await mod.Agent.create(`${fs.readFileSync(config, 'utf8')}
goal_control {
  automatic_enabled = true
  max_slices = "1"
  max_no_progress_slices = "1"
  deadline_unix_ms = "${Date.now() + 600_000}"
}`)
const slice = await bounded.sessionAsync(workspace(), {
  ...base, sessionId: 'node-goal-live-slice', planningMode: 'enabled', goalTracking: true,
})
const ref = (kind, operation_id, view) => ({
  kind, operation_id, goal_id: view.binding.goal_id, session_id: view.binding.session_id,
  workspace: view.binding.workspace, work_policy_digest: null,
  expected_goal_revision: view.binding.goal_revision, expected_plan_revision: view.plan_revision,
  expected_goal_sequence: view.goal_sequence,
})
const goal = slice.applyGoalDefinition(create(slice, null))
const proposed = slice.applyGoalPlan({
  ...ref('propose_plan', 'propose', goal), proposal_sequence: null,
  plan: {
    goal: 'Release check', complexity: 'Simple', required_tools: [], estimated_steps: 1,
    steps: [{ id: 'report', content: prompt, status: 'pending', priority: 'medium', success_criteria: 'Replied READY' }],
  },
})
const accepted = slice.applyGoalPlan({
  ...ref('accept_plan', 'accept', proposed), plan: null, proposal_sequence: proposed.plan_fact_sequence,
})
const tick = ref('tick', 'tick-1', accepted)
const streamed = await slice.streamGoalTick(tick)
assert.equal(streamed.decision ?? null, null)
assert.ok(streamed.started?.run, 'the tick starts one original Run')
const types = []
for await (const event of streamed.started.stream) types.push(event.type)
const snapshot = await streamed.started.run.snapshot()
assert.equal(snapshot.id, 'tick-1', 'the Run is named by the operation id')
assert.notEqual(snapshot.status, 'running')
assert.ok(types.length > 0, 'slice events reached the host')
const again = await slice.streamGoalTick(tick)
assert.equal(again.started ?? null, null, 'a repeated operation starts nothing')
slice.close()
console.log(JSON.stringify({
  sdk: 'node', digest, tokens: result.totalTokens, slice: { status: snapshot.status, events: types.length,
    again: again.decision ?? (again.replayed ? 'replayed' : null) }, ms: Date.now() - started,
}))
