/**
 * Test registerCommand functionality
 */

import { Agent } from '../index.js';

async function main() {
  const agent = await Agent.create('agent.hcl');
  const session = agent.session('.');

  // Register a custom command
  session.registerCommand('greet', 'Say hello', (args, ctx) => {
    const message = `Hello ${args}! Session: ${ctx.sessionId}, Model: ${ctx.model}`;
    console.log(message);
    return message;  // Note: return value is not captured in current implementation
  });

  // Register another command
  session.registerCommand('info', 'Show session info', (args, ctx) => {
    console.log(`Session Info:`);
    console.log(`  ID: ${ctx.sessionId}`);
    console.log(`  Model: ${ctx.model}`);
    console.log(`  History: ${ctx.historyLen} messages`);
    console.log(`  Tokens: ${ctx.totalTokens}`);
    console.log(`  Cost: $${ctx.totalCost.toFixed(4)}`);
    console.log(`  Tools: ${ctx.toolNames.length}`);
    return 'Info displayed';
  });

  // List all commands
  const commands = session.listCommands();
  console.log('\nRegistered commands:');
  commands.forEach(cmd => {
    console.log(`  /${cmd.name} - ${cmd.description}`);
  });

  // Test the custom commands
  console.log('\n--- Testing /greet command ---');
  const result1 = await session.send('/greet World');
  console.log(`Result: ${result1.text}`);

  console.log('\n--- Testing /info command ---');
  const result2 = await session.send('/info');
  console.log(`Result: ${result2.text}`);
}

main().catch(console.error);
