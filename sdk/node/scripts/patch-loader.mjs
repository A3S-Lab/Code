#!/usr/bin/env node

import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const scriptPath = fileURLToPath(import.meta.url)
const root = resolve(dirname(scriptPath), '..')
const loader = resolve(root, 'index.js')

export const MOLI_MARKER = '// a3s-code: bundled Moli runtime bridge'
export const EVENT_MARKER = '// a3s-code: EventStream async iterator bridge'
export const ERROR_MARKER = '// a3s-code: typed core error bridge'
export const ANCHOR = '\nmodule.exports.formatVerificationSummary = formatVerificationSummary'

// napi build regenerates index.js. Keep this bridge self-contained so the
// release build can restore bundled-Moli discovery after every native build.
export const MOLI_BRIDGE = `

${MOLI_MARKER}
function a3sConfigureBundledMoli() {
  if (process.env.A3S_CODE_MOLI_EXECUTABLE) return
  const fs = require('node:fs')
  const path = require('node:path')
  const executable = process.platform === 'win32' ? 'moli.exe' : 'moli'
  const candidates = [
    path.join(__dirname, executable),
    path.join(__dirname, 'moli', executable),
    path.join(__dirname, 'resources', executable),
    path.join(__dirname, 'resources', 'moli', executable),
  ]
  const isMusl = () => {
    if (process.platform !== 'linux') return false
    if (process.report && typeof process.report.getReport === 'function') {
      try {
        return !process.report.getReport().header.glibcVersionRuntime
      } catch (_) {
        // Fall through to the ldd probe.
      }
    }
    try {
      const lddPath = require('node:child_process')
        .execFileSync('which', ['ldd'], { encoding: 'utf8' })
        .trim()
      return Boolean(lddPath && fs.readFileSync(lddPath, 'utf8').includes('musl'))
    } catch (_) {
      return true
    }
  }
  let optionalPackage = null
  if (process.platform === 'darwin') {
    optionalPackage = arch === 'arm64'
      ? '@a3s-lab/code-darwin-arm64'
      : arch === 'x64' ? '@a3s-lab/code-darwin-x64' : null
  } else if (process.platform === 'win32') {
    optionalPackage = arch === 'arm64'
      ? '@a3s-lab/code-win32-arm64-msvc'
      : arch === 'x64' ? '@a3s-lab/code-win32-x64-msvc' : null
  } else if (process.platform === 'linux') {
    const libc = isMusl() ? 'musl' : 'gnu'
    optionalPackage = arch === 'arm64'
      ? \`@a3s-lab/code-linux-arm64-\${libc}\`
      : arch === 'x64' ? \`@a3s-lab/code-linux-x64-\${libc}\` : null
  }
  if (optionalPackage) {
    try {
      const nativeFile = require.resolve(optionalPackage)
      const packageDir = path.dirname(nativeFile)
      candidates.push(
        path.join(packageDir, executable),
        path.join(packageDir, 'moli', executable),
        path.join(packageDir, 'resources', 'moli', executable),
      )
    } catch (_) {
      // A local addon or shared-cache executable may still be available.
    }
  }
  for (const candidate of candidates) {
    try {
      if (fs.statSync(candidate).isFile()) {
        process.env.A3S_CODE_MOLI_EXECUTABLE = candidate
        process.env.A3S_CODE_MOLI_DIR = path.dirname(candidate)
        return
      }
    } catch (_) {
      // Continue through the deterministic candidate list.
    }
  }
}
a3sConfigureBundledMoli()
`

export const EVENT_BRIDGE = `

${EVENT_MARKER}
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

export const ERROR_BRIDGE = `

${ERROR_MARKER}
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

function wrapA3sCodeFunction(name, fn) {
  if (typeof fn !== 'function' || fn.__a3sTypedErrorBridge) return fn
  const wrapped = function typedErrorBridge(...args) {
    try {
      const result = fn(...args)
      if (result && typeof result.then === 'function') {
        return result.catch((error) => { throw normalizeA3sCodeError(error) })
      }
      return result
    } catch (error) {
      throw normalizeA3sCodeError(error)
    }
  }
  Object.defineProperty(wrapped, '__a3sTypedErrorBridge', { value: true })
  Object.defineProperty(wrapped, 'name', { configurable: true, value: name })
  return wrapped
}

wrapA3sCodeErrors(Agent, ['create', 'createFromConfig'])
wrapA3sCodeErrors(Agent && Agent.prototype, [
  'session', 'sessionAsync', 'resumeSession', 'resumeSessionAsync',
  'sessionForAgent', 'sessionForAgentAsync', 'sessionForWorker',
  'sessionForWorkerAsync', 'refreshMcpTools', 'replaceSessionAsync',
  'serveAgentDir', 'disconnectIdleMcp', 'closeSession', 'close',
])
wrapA3sCodeErrors(Session && Session.prototype, [
  'send', 'run', 'resumeRun', 'sendRequest', 'stream', 'streamRequest',
  'sendWithAttachments', 'streamWithAttachments', 'save',
  'addMcpServer', 'addMcpServerConfig', 'addMcp', 'removeMcpServer', 'removeMcp',
  'tool', 'governedTool', 'task', 'delegateTask', 'tasks', 'parallelTask', 'program',
  'readFile', 'writeFile', 'ls', 'editFile', 'patchFile', 'bash', 'glob', 'grep',
  'webSearch', 'git', 'gitCommand', 'confirmToolUse', 'verifyCommands',
  'steer', 'interrupt', 'runControlSnapshot',
  'registerAgentDir', 'registerWorkerAgent', 'registerWorkerAgents',
  'registerDynamicWorkflowRuntime', 'unregisterDynamicTool',
  'registerHook', 'unregisterHook', 'registerCommand',
  'ensureRecoveryCapabilityBinding', 'drainCapabilityCleanup',
])
wrapA3sCodeErrors(StateGraphRuntime, ['restore'])
wrapA3sCodeErrors(StateGraphRuntime && StateGraphRuntime.prototype, [
  'proposePatch', 'runGoal', 'emitCustom', 'emitJson', 'checkExternal',
  'projectExternal', 'graphJson', 'eventsJson',
  'forkAt', 'diffJson',
])
if (typeof module.exports.ensureMoli === 'function') {
  module.exports.ensureMoli = wrapA3sCodeFunction('ensureMoli', module.exports.ensureMoli)
}
if (typeof module.exports.strictReplay === 'function') {
  module.exports.strictReplay = wrapA3sCodeFunction('strictReplay', module.exports.strictReplay)
}
`

export function patchSource(source) {
  if (typeof source !== 'string') {
    throw new TypeError('Node loader source must be a string')
  }
  if (!source.includes(ANCHOR)) {
    throw new Error('Could not find napi export anchor')
  }
  const missing = {
    moli: !source.includes(MOLI_MARKER),
    event: !source.includes(EVENT_MARKER),
    error: !source.includes(ERROR_MARKER),
  }
  let patched = source
  if (missing.moli) patched = patched.replace(ANCHOR, `${MOLI_BRIDGE}${ANCHOR}`)
  if (missing.event) patched = patched.replace(ANCHOR, `${EVENT_BRIDGE}${ANCHOR}`)
  if (missing.error) patched = patched.replace(ANCHOR, `${ERROR_BRIDGE}${ANCHOR}`)
  return { source: patched, changed: patched !== source, missing }
}

export function hasRuntimeBridges(source) {
  return [MOLI_MARKER, EVENT_MARKER, ERROR_MARKER].every((marker) =>
    source.includes(marker),
  )
}

if (process.argv[1] && resolve(process.argv[1]) === resolve(scriptPath)) {
  const checkOnly = process.argv.includes('--check')
  const source = readFileSync(loader, 'utf8')
  if (checkOnly) {
    if (!hasRuntimeBridges(source)) {
      process.stderr.write('Node loader is missing an A3S runtime bridge\n')
      process.exit(1)
    }
    process.stdout.write('Node loader runtime bridges present\n')
  } else {
    const result = patchSource(source)
    if (result.changed) {
      writeFileSync(loader, result.source)
      process.stdout.write('Patched Node loader with runtime bridges\n')
    } else {
      process.stdout.write('Node loader runtime bridges present\n')
    }
  }
}
