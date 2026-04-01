import assert from 'node:assert/strict'
import mod from './index.js'

const runtimePayload = {
  ocr: {
    used: true,
    mode: 'ocr',
    format: 'pdf',
    provider: 'node-mock-ocr',
    model: 'openai/gpt-4.1-mini',
    maxImages: 2,
    dpi: 144,
  },
}

assert.equal(typeof mod.enrichToolResult, 'function')
assert.equal(typeof mod.parseDocumentRuntime, 'function')
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
        document_runtime: runtimePayload,
      },
    ],
  }),
})

assert.deepEqual(enriched.agenticSearchResults?.[0]?.documentRuntime, runtimePayload)
assert.equal(enriched.agenticSearchResults?.[0]?.matches?.[0]?.lineNumber, 12)
assert.equal(enriched.agenticSearchResults?.[0]?.matches?.[0]?.contextBefore?.[0], '[section] page 2: 1. Overview')
assert.deepEqual(mod.parseDocumentRuntime(JSON.stringify(runtimePayload)), runtimePayload)
assert.deepEqual(
  mod.parseAgenticSearchResults(
    JSON.stringify({
      results: [{ document_runtime: runtimePayload, sampled_lines: [{ line_number: 7, locator: 'page 1' }] }],
    }),
  )?.[0]?.documentRuntime,
  runtimePayload,
)
assert.equal(
  mod.parseAgenticSearchResults(
    JSON.stringify({
      results: [{ sampled_lines: [{ line_number: 7, locator: 'page 1' }] }],
    }),
  )?.[0]?.sampledLines?.[0]?.lineNumber,
  7,
)
assert.equal(
  mod.parseAgenticParseLlmBlocks(
    JSON.stringify({
      llm_blocks: [{ index: 1, kind: 'section', label: 'page 2: 1. Overview' }],
    }),
  )?.[0]?.label,
  'page 2: 1. Overview',
)

console.log('node sdk helper smoke ok')
