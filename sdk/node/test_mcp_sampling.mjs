import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import mod from './index.js'

const root = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-node-sampling-'))
const peer = fileURLToPath(new URL('../fixtures/mcp_sampling_server.py', import.meta.url))
const config = `default_model = "fixture/model"
providers "fixture" {
  api_key = "fixture-unused"
  models "model" { name = "Fixture" }
}`
const result = { role: 'assistant', model: 'fixture', content: { type: 'text', text: 'host-answer' } }
const agent = await mod.Agent.create(config)
try {
  assert.equal(await agent.hasSamplingHandlerFactory(), false)
  assert.deepEqual(await agent.globalMcpConfigs(), [])
  for (const timeout of [0, 300001]) {
    assert.throws(() => agent.setSamplingHandlerFactory(async () => result, timeout), /timeoutMs/)
    assert.equal(await agent.hasSamplingHandlerFactory(), false)
  }
  let calls = 0
  const cases = ['unset', 'async', 'throw', 'reject', 'primitive', 'circular', 'bigint', 'timeout', 'ceiling']
  for (const name of cases) {
    const marker = path.join(root, `${name}.json`)
    calls = 0
    if (name !== 'unset') {
      agent.setSamplingHandlerFactory((server, params) => {
        calls++
        assert.equal(server, name)
        assert.equal(params.messages[0].content.text, 'fixture-only')
        if (name === 'throw') throw new Error('fixture refusal')
        if (name === 'reject') return Promise.reject(new Error('fixture refusal'))
        if (name === 'primitive') return 42
        if (name === 'circular') { const value = {}; value.self = value; return value }
        if (name === 'bigint') return { value: 1n }
        if (name === 'timeout') return new Promise(() => {})
        return new Promise(resolve => setTimeout(() => resolve(result), 2))
      }, name === 'timeout' ? 30 : 1000)
      assert.equal(await agent.hasSamplingHandlerFactory(), true)
    }
    const server = { name, enabled: true, transport: {
      type: 'stdio', command: process.env.A3S_TEST_PYTHON ?? 'python3',
      args: [peer, marker, name === 'ceiling' ? '65' : '1'],
    }, env: {} }
    await agent.syncGlobalMcpServers([server])
    const configs = await agent.globalMcpConfigs()
    assert.equal(configs.length, 1)
    assert.equal(configs[0].name, name)
    assert.equal(configs[0].toolTimeoutSecs, 60)
    const replies = JSON.parse(fs.readFileSync(marker, 'utf8'))
    if (name === 'async' || name === 'ceiling') {
      assert.deepEqual(replies[0].result, result)
      assert.equal(calls, name === 'ceiling' ? 64 : 1)
      if (name === 'ceiling') assert.ok(replies[64].error)
    } else {
      assert.ok(replies[0].error, `${name} must refuse`)
      assert.equal(calls, name === 'unset' ? 0 : 1)
      if (name === 'unset') assert.equal(replies[0].error.code, -32601)
    }
    await agent.syncGlobalMcpServers([])
  }
  console.log('node SDK MCP sampling: 9 scenarios passed')
} finally {
  await agent.close()
  fs.rmSync(root, { recursive: true, force: true })
}
