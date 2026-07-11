// Regression coverage for JavaScript callbacks that cross a napi
// ThreadsafeFunction boundary. Each callback family runs in its own process so
// a napi_fatal_error/abort is observed as a failed parent assertion.

import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import mod from './index.js'

const childCase = process.argv[2]

if (childCase) {
  await runChildCase(childCase)
  console.log(`callback-safety:${childCase}:ok`)
  // Registered TSFNs intentionally keep Node alive while their Session is
  // reachable. This child has finished every assertion; exit explicitly so
  // the parent timeout tests callback safety rather than GC timing.
  process.exit(0)
} else {
  const script = fileURLToPath(import.meta.url)
  for (const name of ['pipeline', 'budget', 'slash', 'hook']) {
    const child = spawnSync(process.execPath, [script, name], {
      encoding: 'utf8',
      timeout: 20_000,
    })
    assert.equal(child.signal, null, `${name} callback process died with ${child.signal}\n${child.stderr}`)
    assert.equal(child.status, 0, `${name} callback process failed\n${child.stdout}\n${child.stderr}`)
    assert.match(child.stdout, new RegExp(`callback-safety:${name}:ok`))
  }
  console.log('node sdk callback safety ok')
}

async function runChildCase(name) {
  const tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), `a3s-callback-${name}-`))
  const workspace = path.join(tmpRoot, 'workspace')
  fs.mkdirSync(workspace, { recursive: true })

  const config = `
default_model = "anthropic/claude-sonnet-4-20250514"

providers "anthropic" {
  api_key = "test-key"
  models "claude-sonnet-4-20250514" {
    name = "Claude Sonnet 4"
  }
}
`.trim()

  const agent = await mod.Agent.create(config)
  const session = agent.session(workspace, {
    sessionId: `callback-${name}`,
    permissionPolicy: { defaultDecision: 'allow' },
    workspaceBackend: new mod.LocalWorkspaceBackend(workspace),
  })

  try {
    if (name === 'pipeline') {
      await testPipeline(session)
    } else if (name === 'budget') {
      await testBudget(session)
    } else if (name === 'slash') {
      await testSlash(session)
    } else if (name === 'hook') {
      await testHook(session, workspace)
    } else {
      throw new Error(`unknown child case: ${name}`)
    }
  } finally {
    session.close()
    fs.rmSync(tmpRoot, { recursive: true, force: true })
  }
}

async function testPipeline(session) {
  const thrown = await session.pipeline(
    ['item'],
    [() => {
      throw new Error('pipeline exploded')
    }],
    100,
  )
  assert.deepEqual(thrown, [null], 'throwing stage must stop its chain')

  const malformed = await session.pipeline(['item'], [() => 42], 100)
  assert.deepEqual(malformed, [null], 'malformed stage return must stop its chain')

  const asynchronous = await session.pipeline(
    ['item'],
    [async () => {
      throw new Error('async pipeline exploded')
    }],
    100,
  )
  assert.deepEqual(asynchronous, [null], 'async stage return must be controlled')
}

async function testBudget(session) {
  session.setBudgetGuard({
    checkBeforeLlm: () => {
      throw new Error('budget exploded')
    },
    timeoutMs: 100,
  })
  await assert.rejects(session.send('must be denied before network I/O'), /budget/i)

  session.setBudgetGuard({ checkBeforeLlm: () => ({}), timeoutMs: 100 })
  await assert.rejects(session.send('malformed guard must deny'), /budget/i)

  session.setBudgetGuard({
    checkBeforeLlm: () => {
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 40)
      return null
    },
    timeoutMs: 5,
  })
  await assert.rejects(session.send('timed out guard must deny'), /budget/i)

  session.setBudgetGuard({
    checkBeforeTool: () => {
      throw new Error('tool budget exploded')
    },
    timeoutMs: 100,
  })
  const result = await session.tool('read', { file_path: 'missing.txt' })
  assert.notEqual(result.exitCode, 0, 'throwing tool guard must deny execution')
  assert.match(result.output, /budget|denied|callback/i)
}

async function testSlash(session) {
  session.registerCommand(
    'explode',
    'Throw from a command callback',
    () => {
      throw new Error('slash exploded')
    },
    100,
  )
  const thrown = await session.send('/explode')
  assert.match(thrown.text, /command 'explode' failed:.*slash exploded/i)

  session.registerCommand('malformed', 'Return a non-string', () => ({}), 100)
  const malformed = await session.send('/malformed')
  assert.match(malformed.text, /command 'malformed' failed:.*return a string/i)

  session.registerCommand(
    'slow',
    'Exceed the configured command timeout',
    () => {
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 40)
      return 'too late'
    },
    5,
  )
  const timedOut = await session.send('/slow')
  assert.match(timedOut.text, /command 'slow' failed:.*timed out after 5ms/i)
}

async function testHook(session, workspace) {
  const sideEffect = path.join(workspace, 'must-not-exist.txt')
  session.registerHook(
    'throwing-write-gate',
    'pre_tool_use',
    { tool: 'write' },
    { timeoutMs: 100 },
    () => {
      throw new Error('hook exploded')
    },
  )

  const result = await session.tool('write', {
    file_path: 'must-not-exist.txt',
    content: 'this side effect must be blocked',
  })
  assert.notEqual(result.exitCode, 0, 'throwing gating hook must block the tool')
  assert.match(result.output, /hook|block|failed/i)
  assert.equal(fs.existsSync(sideEffect), false, 'tool side effect must not execute')

  assert.equal(session.unregisterHook('throwing-write-gate'), true)
  session.registerHook(
    'malformed-write-gate',
    'pre_tool_use',
    { tool: 'write' },
    { timeoutMs: 100 },
    () => ({}),
  )
  const malformed = await session.tool('write', {
    file_path: 'must-not-exist.txt',
    content: 'malformed hook results must also fail closed',
  })
  assert.notEqual(malformed.exitCode, 0, 'malformed gating hook return must block the tool')
  assert.equal(fs.existsSync(sideEffect), false, 'malformed hook must not allow the side effect')
}
