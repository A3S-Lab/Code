// Explicit live gate: A3S_CONFIG_FILE must name a credentialed ACL file.
// Run separately from npm test. No local model peer or command execution.
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import mod from './index.js'

const config = process.env.A3S_CONFIG_FILE
assert.ok(config && fs.statSync(config).isFile(), 'A3S_CONFIG_FILE is required')
assert.equal(JSON.parse(fs.readFileSync(new URL('./package.json', import.meta.url))).version, '9.1.3')
const root = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-node-live-release-'))
const agent = await mod.Agent.create(config)
let session
const marker = 'SDK-LIVE-913:NODE:42'
const receipts = []
try {
  session = await agent.sessionAsync(root, {
    sessionId: 'node-release-real-913', planningMode: 'disabled',
    manualDelegationEnabled: false, autoDelegation: { enabled: false },
    continuationEnabled: false, maxToolRounds: 2,
    llmApiTimeoutMs: 120_000, sandboxNetwork: 'deny',
    permissionPolicy: { defaultDecision: 'deny' },
  })
  let previous
  for (const prompt of [
    `Remember the verification code ${marker}. Reply with exactly that code. Do not use tools.`,
    'Return only the verification code from my previous message. Do not use tools.',
  ]) {
    session.setPermissionPolicy({ defaultDecision: 'deny' })
    const admitted = await session.streamWithRun(prompt)
    assert.ok(admitted.run, 'a model turn must admit an original Run')
    assert.equal(admitted.run.sessionId, session.sessionId)
    if (previous) {
      assert.notEqual(admitted.run.id, previous.id)
      assert.equal(await previous.cancel(), false, 'a completed handle cannot cancel a later Run')
    }
    const types = []
    for await (const event of admitted.stream) types.push(event.type)
    const snapshot = await admitted.run.snapshot()
    assert.equal(snapshot.id, admitted.run.id)
    assert.equal(snapshot.status, 'completed')
    assert.equal(snapshot.result_text.trim(), marker)
    assert.ok(types.includes('text_delta'))
    assert.equal(types.filter(type => type === 'agent_end').length, 1)
    assert.ok(!types.includes('tool_execution_start'), 'the conversation must not execute tools')
    const target = { schema: 'a3s.code.execution-target.v1', session_id: session.sessionId, run_id: admitted.run.id }
    const evidence = await session.readEvaluationEvidence({ target })
    assert.deepEqual(evidence.target, target)
    await assert.rejects(session.readEvaluationEvidence({ target: { ...target, session_id: 'foreign' } }), /not found/i)
    receipts.push({ run_id: admitted.run.id, status: snapshot.status, result: snapshot.result_text, event_types: [...new Set(types)], evidence_target: evidence.target })
    previous = admitted.run
  }
  const report = { sdk: 'node', version: '9.1.3', model: session.modelName, session_id: session.sessionId, receipts }
  console.log('SDK_RELEASE_REAL ' + JSON.stringify(report))
} finally {
  if (session) await session.closeAsync()
  await agent.close()
  fs.rmSync(root, { recursive: true, force: true })
}
