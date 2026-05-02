import { Agent } from '../../index.js'

async function main() {
  const agent = await Agent.create('agent.acl')
  const session = agent.session('.', { permissionPolicy: { defaultDecision: 'allow' } })

  const tool = await session.tool('agentic_parse', {
    path: 'docs/scanned.pdf',
    query: 'overview',
  })

  const blocks = tool.agenticParseLlmBlocks ?? []
  for (const block of blocks) {
    console.log(block.index, block.kind, block.label, block.location?.display)
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
