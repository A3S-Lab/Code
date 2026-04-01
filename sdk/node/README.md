# A3S Code — Node SDK

Native Node.js bindings for the A3S Code AI coding agent.

## Installation

```bash
npm install @a3s-lab/code
```

## Quick Start

```js
const { Agent } = require('@a3s-lab/code')

async function main() {
  const agent = await Agent.create('agent.hcl')
  const session = agent.session('/my-project')

  const result = await session.send('What files handle authentication?')
  console.log(result.text)
}

main().catch(console.error)
```

## Tool Metadata Helpers

`session.tool(...)` returns a `ToolResult` enriched with parsed metadata helpers.

### Document Runtime

```js
const { parseDocumentRuntime } = require('@a3s-lab/code')

const tool = await session.tool('agentic_parse', { path: 'docs/scanned.pdf' })
console.log(tool.metadata)
console.log(tool.documentRuntime)

const runtime = parseDocumentRuntime(tool)
if (runtime?.ocr) {
  console.log(runtime.ocr.provider, runtime.ocr.model, runtime.ocr.dpi)
}
```

### Agentic Parse LLM Blocks

When `agentic_parse` runs with a query, the SDK exposes the exact structured
document blocks selected for the LLM input.

```js
const { parseAgenticParseLlmBlocks } = require('@a3s-lab/code')

const tool = await session.tool('agentic_parse', {
  path: 'docs/scanned.pdf',
  query: 'overview',
})

for (const block of tool.agenticParseLlmBlocks ?? parseAgenticParseLlmBlocks(tool) ?? []) {
  console.log(block.index, block.kind, block.label, block.location?.display)
}
```

### Agentic Search Match Locators

`agentic_search` results expose typed match metadata, including page / section
locators derived from `CompositeDocumentParser` blocks.

```js
const { parseAgenticSearchResults } = require('@a3s-lab/code')

const search = await session.tool('agentic_search', {
  query: 'overview',
  mode: 'fast',
})

for (const result of search.agenticSearchResults ?? parseAgenticSearchResults(search) ?? []) {
  for (const match of result.matches ?? []) {
    console.log(match.lineNumber, match.locator, match.content)
  }
}
```

Deep search also exposes `sampledLines` with `locator`, `distance`, and `weight`.

```js
const deep = await session.tool('agentic_search', {
  query: 'overview',
  mode: 'deep',
})

for (const result of deep.agenticSearchResults ?? parseAgenticSearchResults(deep) ?? []) {
  for (const sampled of result.sampledLines ?? []) {
    console.log(sampled.lineNumber, sampled.locator, sampled.distance, sampled.weight)
  }
}
```

## OCR Backend For Better Context Extraction

Use `documentOcrProvider` when you want `agentic_search` and `agentic_parse`
to recover context from scanned PDFs or images via a JavaScript OCR callback.

```js
const {
  Agent,
  DocumentParserRegistry,
} = require('@a3s-lab/code')

async function main() {
  const agent = await Agent.create('agent.hcl')

  const session = agent.session('.', {
    documentParserRegistry: new DocumentParserRegistry({
      ocr: {
        enabled: true,
        model: 'openai/gpt-4.1-mini',
        maxImages: 2,
        dpi: 144,
      },
    }),
    documentOcrProvider: {
      name: 'node-mock-ocr',
      formats: ['pdf', 'image'],
      handler(request) {
        console.log(request.path, request.format, request.config)
        return 'Recovered OCR text from Node backend.'
      },
    },
  })

  const tool = await session.tool('agentic_parse', { path: 'docs/scanned.pdf' })
  console.log(tool.documentRuntime)
}

main().catch(console.error)
```

If you do not inject a backend, the Rust core can also auto-detect local
`tesseract` and `pdftoppm` installations for the same context-extraction fallback.

## Examples

- `examples/test-document-ocr-provider.js`
- `examples/test-agentic-parse-llm-blocks.js`
- `examples/test-agentic-search-locators.js`
- `examples/test-agentic-search-sampled-lines.js`
- `examples/test-agentic-search-sdk.js`
