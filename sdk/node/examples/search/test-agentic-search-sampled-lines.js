const { Agent } = require('..')

async function main() {
  const agent = await Agent.create('agent.hcl')
  const session = agent.session('.', { permissive: true })

  const tool = await session.tool('agentic_search', {
    query: 'overview',
    mode: 'deep',
  })

  const results = tool.agenticSearchResults ?? []
  for (const result of results) {
    console.log('result:', result.path, result.fileType, result.relevance)
    for (const sampled of result.sampledLines ?? []) {
      console.log(
        '  sampled:',
        sampled.lineNumber,
        sampled.locator,
        sampled.distance,
        sampled.weight
      )
    }
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
