import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'

import {
  PLATFORM_PACKAGE_NAMES,
  withReleaseOptionalDependencies,
} from './release-manifest.mjs'

test('release manifest binds every platform package to the exact SDK version', () => {
  const source = {
    name: '@a3s-lab/code',
    version: '8.0.1',
    scripts: { test: 'node test.mjs' },
  }

  const prepared = withReleaseOptionalDependencies(source)

  assert.deepEqual(prepared.optionalDependencies, {
    '@a3s-lab/code-darwin-arm64': '8.0.1',
    '@a3s-lab/code-linux-arm64-gnu': '8.0.1',
    '@a3s-lab/code-linux-arm64-musl': '8.0.1',
    '@a3s-lab/code-linux-x64-gnu': '8.0.1',
    '@a3s-lab/code-linux-x64-musl': '8.0.1',
    '@a3s-lab/code-win32-x64-msvc': '8.0.1',
  })
  assert.deepEqual(Object.keys(prepared.optionalDependencies), PLATFORM_PACKAGE_NAMES)
  assert.deepEqual(source, {
    name: '@a3s-lab/code',
    version: '8.0.1',
    scripts: { test: 'node test.mjs' },
  })
})

test('release manifest rejects a missing or non-release version', () => {
  assert.throws(
    () => withReleaseOptionalDependencies({ name: '@a3s-lab/code' }),
    /valid release version/,
  )
  assert.throws(
    () => withReleaseOptionalDependencies({ version: 'latest' }),
    /valid release version/,
  )
})

test('development manifest does not depend on unpublished platform packages', () => {
  const manifest = JSON.parse(
    readFileSync(new URL('../package.json', import.meta.url), 'utf8'),
  )

  assert.equal(manifest.optionalDependencies, undefined)
})
