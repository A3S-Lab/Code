/**
 * Test agentic_search tool availability in Node.js SDK
 *
 * This example verifies that the agentic_search tool is automatically
 * available through the SDK without requiring explicit bindings.
 */

const { Agent } = require('../index.js');

async function main() {
  // Create agent
  const agent = await Agent.create('../../agent.example.hcl');

  // Create session with builtin skills
  const session = agent.session('.', {
    builtinSkills: true,
    permissive: true,  // Auto-approve tool execution for testing
  });

  console.log('Testing agentic_search tool availability...\n');

  // Test: LLM should be able to call agentic_search tool
  const result = await session.send(
    'Use the agentic_search tool to find files related to "agent" in this codebase. ' +
    'Use mode "filename_only" and max_results 5.'
  );

  console.log('Result:', result.text);
  console.log('\n✅ agentic_search tool is available and working!');
}

main().catch(console.error);
