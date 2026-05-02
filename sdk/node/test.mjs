import assert from 'node:assert/strict'
import mod from './index.js'
import os from 'node:os'
import path from 'node:path'
import fs from 'node:fs'

const requiredExports = [
  'Agent',
  'Session',
  'EventStream',
  'builtinSkills',
]

for (const name of requiredExports) {
  assert.equal(name in mod, true, `missing export: ${name}`)
}

assert.equal(typeof mod.Agent, 'function', 'Agent export should be a constructor')
assert.equal(typeof mod.builtinSkills, 'function', 'builtinSkills should be callable')

const tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-node-test-'))
const workspace = path.join(tmpRoot, 'workspace')
fs.mkdirSync(workspace, { recursive: true })
const canonicalWorkspace = fs.realpathSync(workspace)

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
const session = agent.session(workspace, { permissionPolicy: { defaultDecision: 'allow' } })

const commands = session.listCommands()
assert.equal(Array.isArray(commands), true, 'listCommands() should return an array')
assert.equal(commands.some((cmd) => cmd.name === 'help'), true, 'built-in /help should be registered')

session.registerCommand('status', 'Show session info', (args, ctx) => {
  return `args=${args};workspace=${ctx.workspace};tools=${ctx.toolNames.length}`
})

const updatedCommands = session.listCommands()
assert.equal(updatedCommands.some((cmd) => cmd.name === 'status'), true, 'custom /status should be registered')

const help = await session.send('/help')
assert.equal(help.text.includes('/help'), true, '/help should render command help text')

const model = await session.send('/model')
assert.equal(
  model.text.includes('Current model: anthropic/claude-sonnet-4-20250514'),
  true,
  '/model should report the active model'
)

const cost = await session.send('/cost')
assert.equal(cost.text.includes('Model:'), true, '/cost should include model info')
assert.equal(cost.text.includes('Tokens:'), true, '/cost should include token usage')

const history = await session.send('/history')
assert.equal(history.text.includes('Messages:'), true, '/history should include message count')
assert.equal(history.text.includes('Session:'), true, '/history should include session id')

const tools = await session.send('/tools')
assert.equal(tools.text.includes('Tools:'), true, '/tools should summarize registered tools')
assert.equal(tools.text.includes('Builtin'), true, '/tools should list builtin tools')

const result = await session.send('/status hello world')
assert.equal(result.text.includes('args=hello world;'), true, 'custom slash command should receive args')
assert.equal(
  result.text.includes(`workspace=${canonicalWorkspace};`),
  true,
  'custom slash command should receive workspace in context'
)
assert.match(result.text, /tools=\d+$/, 'custom slash command should receive toolNames in context')

session.close()

console.log('node sdk integration ok')
process.exit(0)
