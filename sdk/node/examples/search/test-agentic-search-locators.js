const { Agent } = require('..')

async function main() {
  const agent = await Agent.create('agent.hcl')
  const session = agent.session('.', { permissive: true })

  const tool = await session.tool('agentic_search', {
    query: 'overview',
    mode: 'fast',
  })

  const results = tool.agenticSearchResults ?? []
  for (const result of results) {
    for (const match of result.matches ?? []) {
      console.log(match.lineNumber, match.locator, match.content)
    }
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
