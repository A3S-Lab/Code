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

### Agentic Parse LLM Blocks

When `agentic_parse` runs with a query, the SDK exposes the exact structured
document blocks selected for the LLM input.

```js
const tool = await session.tool('agentic_parse', {
  path: 'docs/scanned.pdf',
  query: 'overview',
})

for (const block of tool.agenticParseLlmBlocks ?? []) {
  console.log(block.index, block.kind, block.label, block.location?.display)
}
```

### Agentic Search Match Locators

`agentic_search` results expose typed match metadata, including page / section
locators derived from `CompositeDocumentParser` blocks.

```js
const search = await session.tool('agentic_search', {
  query: 'overview',
  mode: 'fast',
})

for (const result of search.agenticSearchResults ?? []) {
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

for (const result of deep.agenticSearchResults ?? []) {
  for (const sampled of result.sampledLines ?? []) {
    console.log(sampled.lineNumber, sampled.locator, sampled.distance, sampled.weight)
  }
}
```

## Examples

- `examples/test-agentic-parse-llm-blocks.js`
- `examples/test-agentic-search-locators.js`
- `examples/test-agentic-search-sampled-lines.js`
- `examples/test-agentic-search-sdk.js`
