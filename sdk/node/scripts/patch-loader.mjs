#!/usr/bin/env node

import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const loader = resolve(root, 'index.js')
const checkOnly = process.argv.includes('--check')
const marker = '// a3s-code: EventStream async iterator bridge'
const errorMarker = '// a3s-code: typed core error bridge'
const anchor = '\nmodule.exports.formatVerificationSummary = formatVerificationSummary'
const bridge = `

${marker}
// napi-rs exposes the async \`next()\` method but does not install the symbol
// required by JavaScript's \`for await ... of\` protocol.
if (EventStream && typeof EventStream.prototype[Symbol.asyncIterator] !== 'function') {
  Object.defineProperty(EventStream.prototype, Symbol.asyncIterator, {
    configurable: true,
    value: function asyncIterator() {
      return this
    },
  })
}
`

const errorBridge = `

${errorMarker}
// Core errors carry a private marker across napi's generic Error conversion.
// Strip it at the JS boundary and expose the stable machine-readable code.
function normalizeA3sCodeError(error) {
  if (!(error instanceof Error)) return error
  const match = /^\\[A3S_CODE_ERROR:([A-Z_]+)\\]\\s*/.exec(error.message)
  if (!match) return error
  Object.defineProperty(error, 'code', {
    configurable: true,
    enumerable: true,
    value: match[1],
  })
  error.message = error.message.slice(match[0].length)
  return error
}

function wrapA3sCodeErrors(target, methods) {
  if (!target) return
  for (const method of methods) {
    const original = target[method]
    if (typeof original !== 'function' || original.__a3sTypedErrorBridge) continue
    const wrapped = function typedErrorBridge(...args) {
      try {
        const result = original.apply(this, args)
        if (result && typeof result.then === 'function') {
          return result.catch((error) => { throw normalizeA3sCodeError(error) })
        }
        return result
      } catch (error) {
        throw normalizeA3sCodeError(error)
      }
    }
    Object.defineProperty(wrapped, '__a3sTypedErrorBridge', { value: true })
    target[method] = wrapped
  }
}

wrapA3sCodeErrors(Agent, ['create'])
wrapA3sCodeErrors(Agent && Agent.prototype, [
  'session', 'sessionAsync', 'resumeSession', 'resumeSessionAsync',
  'sessionForAgent', 'sessionForAgentAsync', 'sessionForWorker',
  'sessionForWorkerAsync', 'refreshMcpTools',
])
wrapA3sCodeErrors(Session && Session.prototype, [
  'send', 'run', 'resumeRun', 'sendRequest', 'stream', 'streamRequest',
  'sendWithAttachments', 'streamWithAttachments', 'save',
  'addMcpServer', 'addMcpServerConfig', 'addMcp', 'removeMcpServer', 'removeMcp',
  'tool', 'task', 'delegateTask', 'tasks', 'parallelTask', 'program',
  'readFile', 'writeFile', 'ls', 'editFile', 'patchFile', 'bash', 'glob', 'grep',
  'webSearch', 'git', 'gitCommand', 'confirmToolUse', 'verifyCommands',
  'registerAgentDir', 'registerWorkerAgent', 'registerWorkerAgents',
  'registerDynamicWorkflowRuntime', 'unregisterDynamicTool',
  'registerHook', 'unregisterHook', 'registerCommand',
])
`

let source = readFileSync(loader, 'utf8')
const missingBridge = !source.includes(marker)
const missingErrorBridge = !source.includes(errorMarker)

if (checkOnly) {
  if (missingBridge || missingErrorBridge) {
    process.stderr.write('Node loader is missing an A3S runtime bridge\n')
    process.exit(1)
  }
  process.stdout.write('Node loader runtime bridges present\n')
  process.exit(0)
}
if (!source.includes(anchor)) {
  throw new Error(`Could not find napi export anchor in ${loader}`)
}

if (missingBridge) source = source.replace(anchor, `${bridge}${anchor}`)
if (missingErrorBridge) source = source.replace(anchor, `${errorBridge}${anchor}`)
if (missingBridge || missingErrorBridge) {
  writeFileSync(loader, source)
  process.stdout.write('Patched Node loader with runtime bridges\n')
} else {
  process.stdout.write('Node loader runtime bridges present\n')
}
