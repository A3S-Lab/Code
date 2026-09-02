#!/usr/bin/env node

import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const RELEASE_VERSION = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/
const scriptPath = fileURLToPath(import.meta.url)
const defaultManifestPath = resolve(dirname(scriptPath), '..', 'package.json')

export const PLATFORM_PACKAGE_NAMES = Object.freeze([
  '@a3s-lab/code-darwin-arm64',
  '@a3s-lab/code-darwin-x64',
  '@a3s-lab/code-linux-arm64-gnu',
  '@a3s-lab/code-linux-arm64-musl',
  '@a3s-lab/code-linux-x64-gnu',
  '@a3s-lab/code-linux-x64-musl',
  '@a3s-lab/code-win32-arm64-msvc',
  '@a3s-lab/code-win32-x64-msvc',
])

export function withReleaseOptionalDependencies(manifest) {
  const version = manifest?.version
  if (typeof version !== 'string' || !RELEASE_VERSION.test(version)) {
    throw new Error('Node release manifest requires a valid release version')
  }

  return {
    ...manifest,
    optionalDependencies: Object.fromEntries(
      PLATFORM_PACKAGE_NAMES.map((name) => [name, version]),
    ),
  }
}

export function prepareReleaseManifest(manifestPath = defaultManifestPath) {
  const source = JSON.parse(readFileSync(manifestPath, 'utf8'))
  const prepared = withReleaseOptionalDependencies(source)
  writeFileSync(manifestPath, `${JSON.stringify(prepared, null, 2)}\n`)
  return prepared
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  const manifestPath = resolve(process.argv[2] ?? defaultManifestPath)
  const prepared = prepareReleaseManifest(manifestPath)
  process.stdout.write(
    `Prepared ${prepared.name}@${prepared.version} with ${PLATFORM_PACKAGE_NAMES.length} exact platform dependencies\n`,
  )
}
