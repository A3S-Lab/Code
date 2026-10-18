import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import http from 'node:http'
import mod from './index.js'

const root = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-sdk-release-'))
let requests = 0
let signalBlockingResponse
const blockingResponse = new Promise(resolve => { signalBlockingResponse = resolve })
const server = http.createServer(async (request, response) => {
  for await (const _ of request) { /* Drain the bounded local fixture request. */ }
  requests++
  response.writeHead(200, { 'content-type': 'text/event-stream' })
  await new Promise(resolve => setTimeout(resolve, 40))
  response.write(`data: ${JSON.stringify({ id: 'fixture', choices: [{ index: 0, delta: { role: 'assistant', content: 'fixture-answer' }, finish_reason: null }] })}\n\n`)
  if (requests === 3) return // Retain the actual partial response until cancellation.
  if (requests === 5) {
    signalBlockingResponse()
    return
  }
  response.end(`data: ${JSON.stringify({ choices: [{ index: 0, delta: {}, finish_reason: 'stop' }], usage: { prompt_tokens: 10, completion_tokens: 3, total_tokens: 13 } })}\n\ndata: [DONE]\n\n`)
})
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
const config = `default_model = "openai/fixture"
providers "openai" {
  api_key = "local-fixture-unused"
  base_url = "http://127.0.0.1:${server.address().port}/v1"
  models "fixture" { name = "Fixture" }
}
memory { llm_extraction = false }`
const agent = await mod.Agent.create(config)
let session
async function drain(stream) { for await (const _ of stream) {} }
try {
  await assert.rejects(agent.sessionAsync(root, { sandboxNetwork: 'invalid' }), /sandbox network mode/)
  await assert.rejects(agent.sessionAsync(root, { pathInjectionBudget: 0 }), /budget/)
  await assert.rejects(agent.sessionAsync(root, { workspaceReadOverlay: { '../outside.txt': 'refuse' } }), /boundary/)
  fs.writeFileSync(path.join(root, 'buffer.txt'), 'saved\n')
  fs.writeFileSync(path.join(root, 'disk.txt'), 'disk\n')
  const overlay = await agent.sessionAsync(root, {
    workspaceReadOverlay: { './buffer.txt': 'unsaved\n' },
    permissionPolicy: { defaultDecision: 'allow' },
  })
  try {
    assert.match(await overlay.readFile('buffer.txt'), /unsaved/)
    assert.match(await overlay.readFile('disk.txt'), /disk/)
    assert.equal(fs.readFileSync(path.join(root, 'buffer.txt'), 'utf8'), 'saved\n')
  } finally { await overlay.closeAsync() }
  await assert.rejects(agent.sessionAsync(root, { pathInjectionBudget: 1.5 }), /integer/)
  await assert.rejects(agent.sessionAsync(root, { verificationContracts: [{ id: '', command: '', covers: [] }] }), /contract/)
  session = await agent.sessionAsync(root, {
    sessionId: 'node-release-contract', planningMode: 'disabled',
    manualDelegationEnabled: false, continuationEnabled: false,
    permissionPolicy: { defaultDecision: 'deny' },
    watchSkillDirs: [path.join(root, 'future-skills')], projectedSkillFiles: [],
    sandboxNetwork: 'deny', pathInjectionBudget: 4096,
  })
  assert.throws(() => session.setPermissionPosture('invalid'), /posture/)
  session.setPermissionPosture('plan')
  session.setPermissionPolicy({ deny: ['write'], defaultDecision: 'deny' })
  assert.ok(Array.isArray(session.projectedCapabilityNames()))
  const local = await session.streamWithRun('/help')
  assert.equal(local.run, undefined)
  await drain(local.stream)
  const first = await session.streamWithRun('Return fixture-answer without tools.')
  assert.ok(first.run.id)
  assert.equal(first.run.sessionId, session.sessionId)
  await drain(first.stream)
  const second = await session.streamWithRun('Repeat fixture-answer without tools.')
  assert.notEqual(second.run.id, first.run.id)
  assert.equal(await first.run.cancel(), false, 'an old handle must not cancel the new Run')
  await drain(second.stream)
  assert.equal((await second.run.snapshot()).status, 'completed')
  assert.equal(requests, 2)
  const context = session.contextReport()
  assert.equal(context.usage.prompt_tokens, 20)
  assert.equal(context.usage.completion_tokens, 6)
  assert.equal(context.tool_results, 0)
  assert.deepEqual(context.heaviest, [])
  const evidence = await session.readEvaluationEvidence({ target: {
    schema: 'a3s.code.execution-target.v1', session_id: session.sessionId, run_id: second.run.id,
  } })
  assert.ok(evidence)
  await assert.rejects(session.readEvaluationEvidence({ target: {
    schema: 'a3s.code.execution-target.v1', session_id: 'foreign', run_id: second.run.id,
  } }), /not found/i)
  const cancelled = await session.streamWithRun('Cancel this partial fixture response.')
  const cancelledEvents = []
  let accepted = false
  for await (const event of cancelled.stream) {
    cancelledEvents.push(event.type)
    if (event.type === 'text_delta' && !accepted) {
      accepted = await cancelled.run.cancel()
      assert.equal(accepted, true)
    }
  }
  assert.equal(accepted, true)
  assert.equal(cancelledEvents.filter(type => type === 'agent_end').length, 0)
  assert.equal(cancelledEvents.filter(type => type === 'error').length, 1)
  assert.equal((await cancelled.run.snapshot()).status, 'cancelled')
  const continued = await session.streamWithRun('Continue after the cancelled fixture response.')
  assert.notEqual(continued.run.id, cancelled.run.id)
  assert.equal(await cancelled.run.cancel(), false)
  await drain(continued.stream)
  assert.equal((await continued.run.snapshot()).status, 'completed')
  assert.equal(requests, 4)
  const blocking = session.send('Cancel this blocking partial response.')
    .then(() => { throw new Error('cancelled blocking call returned success') }, error => error)
  let deadline
  try {
    await Promise.race([blockingResponse, new Promise((_, reject) => {
      deadline = setTimeout(() => reject(new Error('blocking provider was not entered')), 5000)
    })])
  } finally { clearTimeout(deadline) }
  const active = await session.currentRun()
  assert.ok(active.id)
  assert.equal(await session.cancelRun(active.id), true)
  const error = await blocking
  assert.equal(error.code, 'RUN_CANCELLED')
  assert.ok(error.message.includes(active.id))
  assert.equal((await session.runSnapshot(active.id)).status, 'cancelled')
  const afterBlocking = await session.streamWithRun('Continue after blocking cancellation.')
  assert.notEqual(afterBlocking.run.id, active.id)
  assert.equal(await session.cancelRun(active.id), false)
  await drain(afterBlocking.stream)
  assert.equal((await afterBlocking.run.snapshot()).status, 'completed')
  assert.equal(requests, 6)
  const factsBefore = fs.readdirSync(root)
  assert.equal(await session.forkSessionAt(Number.MAX_SAFE_INTEGER), 0)
  await assert.rejects(session.forkSessionAt(-1), /safe integer/)
  assert.deepEqual(fs.readdirSync(root), factsBefore)
  console.log('node SDK release contract passed: admission, original Run, policy, evidence, cancellation continuity and option refusal')
} finally {
  if (session) await session.closeAsync()
  await agent.close()
  server.closeAllConnections()
  await new Promise(resolve => server.close(resolve))
  fs.rmSync(root, { recursive: true, force: true })
}
