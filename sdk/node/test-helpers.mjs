import assert from 'node:assert/strict'
import mod from './index.js'

assert.equal(typeof mod.enrichToolResult, 'function')
assert.equal(typeof mod.parseAgenticSearchResults, 'function')
assert.equal(typeof mod.parseAgenticParseLlmBlocks, 'function')

const enriched = mod.enrichToolResult({
  name: 'agentic_search',
  output: 'ok',
  exitCode: 0,
  metadataJson: JSON.stringify({
    results: [
      {
        path: 'docs/scanned.pdf',
        file_type: 'file',
        matches: [
          {
            line_number: 12,
            content: 'The parser now emits structured search labels.',
            locator: 'page 2 | page 2: 1. Overview',
            context_before: ['[section] page 2: 1. Overview'],
            context_after: ['Additional supporting text.'],
          },
        ],
      },
    ],
  }),
})

assert.equal(enriched.agenticSearchResults?.[0]?.matches?.[0]?.lineNumber, 12)
assert.equal(enriched.agenticSearchResults?.[0]?.matches?.[0]?.contextBefore?.[0], '[section] page 2: 1. Overview')
console.log('node sdk helper smoke ok')
