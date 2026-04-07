import assert from 'node:assert/strict'
import mod from './index.js'
import os from 'node:os'
import path from 'node:path'
import fs from 'node:fs'

const requiredExports = [
  'Agent',
  'Session',
  'EventStream',
  'Team',
  'TeamRunner',
  'builtinSkills',
  'enrichToolResult',
  'parseAgenticSearchResults',
  'parseAgenticParseLlmBlocks',
]

for (const name of requiredExports) {
  assert.equal(name in mod, true, `missing export: ${name}`)
}

assert.equal(typeof mod.Agent, 'function', 'Agent export should be a constructor')
assert.equal(typeof mod.builtinSkills, 'function', 'builtinSkills should be callable')
assert.equal(typeof mod.enrichToolResult, 'function', 'enrichToolResult should be callable')
assert.equal(typeof mod.parseAgenticSearchResults, 'function', 'parseAgenticSearchResults should be callable')
assert.equal(typeof mod.parseAgenticParseLlmBlocks, 'function', 'parseAgenticParseLlmBlocks should be callable')

const enriched = mod.enrichToolResult({
  name: 'agentic_parse',
  output: 'ok',
  exitCode: 0,
  metadataJson: JSON.stringify({
    llm_blocks: [{ index: 1, kind: 'section', label: 'page 2: 1. Overview' }],
    other: { score: 1 },
  }),
})
assert.equal(enriched.metadata.other.score, 1, 'enrichToolResult() should parse metadataJson')
assert.equal(enriched.agenticParseLlmBlocks?.[0]?.label, 'page 2: 1. Overview')
assert.equal(
  mod.parseAgenticParseLlmBlocks(enriched)?.[0]?.kind,
  'section',
  'parseAgenticParseLlmBlocks() should accept ToolResult objects'
)

const searchPayload = {
  results: [
    {
      path: 'docs/scanned.pdf',
      file_type: 'file',
      relevance: 1.25,
      matches: [
        {
          line_number: 12,
          content: 'The parser now emits structured search labels.',
          locator: 'page 2 | page 2: 1. Overview',
          context_before: ['[section] page 2: 1. Overview'],
          context_after: [],
        },
      ],
      sampled_lines: [
        {
          line_number: 12,
          content: 'The parser now emits structured search labels.',
          locator: 'page 2 | page 2: 1. Overview',
          distance: 0,
          weight: 1,
        },
      ],
    },
  ],
}
const enrichedSearch = mod.enrichToolResult({
  name: 'agentic_search',
  output: 'ok',
  exitCode: 0,
  metadataJson: JSON.stringify(searchPayload),
})
assert.equal(
  enrichedSearch.agenticSearchResults?.[0]?.matches?.[0]?.lineNumber,
  12,
  'agentic_search result entries should expose camelCase match fields'
)
assert.equal(
  enrichedSearch.agenticSearchResults?.[0]?.sampledLines?.[0]?.lineNumber,
  12,
  'agentic_search sampled_lines should expose sampledLines camelCase helper field'
)
assert.equal(typeof mod.builtinSkills, 'function', 'builtinSkills() should remain exported')

const tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-node-test-'))
const workspace = path.join(tmpRoot, 'workspace')
fs.mkdirSync(workspace, { recursive: true })
const canonicalWorkspace = fs.realpathSync(workspace)

const inlineConfig = `
default_model = "anthropic/claude-sonnet-4-20250514"

providers {
  name = "anthropic"
  api_key = "test-key"
  models {
    id = "claude-sonnet-4-20250514"
    name = "Claude Sonnet 4"
  }
}
`.trim()

const agent = await mod.Agent.create(inlineConfig)
const session = agent.session(workspace, { permissive: true })

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

const scheduledId = session.scheduleTask('print working directory', 15)
assert.match(scheduledId, /^[0-9a-f]{8}$/, 'scheduleTask() should return an 8-char task id')

const scheduledTasks = session.listScheduledTasks()
assert.equal(scheduledTasks.some((task) => task.id === scheduledId), true, 'listScheduledTasks() should include scheduled task')

const cronList = await session.send('/cron-list')
assert.equal(cronList.text.includes(scheduledId), true, '/cron-list should include scheduled task id')

const loopResult = await session.send('/loop 30s monitor build status')
assert.match(loopResult.text, /^Scheduled \[[0-9a-f]{8}\]: "monitor build status" /, '/loop should schedule a recurring prompt')

const tasksAfterLoop = session.listScheduledTasks()
assert.equal(tasksAfterLoop.length >= 2, true, '/loop should add a second scheduled task')

const loopTask = tasksAfterLoop.find((task) => task.id !== scheduledId)
assert.ok(loopTask, 'loop-created task should be discoverable')
assert.equal(loopTask.prompt, 'monitor build status', 'loop-created task should preserve the prompt')
assert.equal(loopTask.intervalSecs, 30, 'loop-created task should preserve the requested interval')

const cancelProgrammatic = session.cancelScheduledTask(scheduledId)
assert.equal(cancelProgrammatic, true, 'cancelScheduledTask() should cancel existing task')
assert.equal(
  session.listScheduledTasks().some((task) => task.id === scheduledId),
  false,
  'cancelScheduledTask() should remove the task from the scheduler'
)

const cancelCommand = await session.send(`/cron-cancel ${loopTask.id}`)
assert.equal(cancelCommand.text.includes(`Cancelled task [${loopTask.id}]`), true, '/cron-cancel should confirm cancellation')

const cronListAfter = await session.send('/cron-list')
assert.equal(
  cronListAfter.text.includes('No scheduled tasks'),
  true,
  '/cron-list should report empty state after all tasks are cancelled'
)

session.close()

console.log('node sdk integration ok')
