import assert from 'node:assert/strict'
import test from 'node:test'

import {
  ANCHOR,
  hasRuntimeBridges,
  patchSource,
} from './patch-loader.mjs'

test('patches a freshly generated napi loader with all runtime bridges', () => {
  const generated = `const nativeBinding = {}${ANCHOR}`
  const first = patchSource(generated)

  assert.equal(first.changed, true)
  assert.deepEqual(first.missing, { moli: true, event: true, error: true })
  assert.equal(hasRuntimeBridges(first.source), true)
  assert.match(first.source, /a3sConfigureBundledMoli\(\)/)
  assert.match(first.source, /wrapA3sCodeFunction\('ensureMoli'/)

  const second = patchSource(first.source)
  assert.equal(second.changed, false)
  assert.deepEqual(second.missing, { moli: false, event: false, error: false })
  assert.equal(second.source, first.source)
})

test('rejects a loader without the stable napi export anchor', () => {
  assert.throws(() => patchSource('const nativeBinding = {}'), /export anchor/)
})
