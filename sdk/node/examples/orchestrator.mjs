/**
 * Agent Orchestrator Example for Node.js
 *
 * Demonstrates main-sub agent coordination with real-time monitoring
 * and dynamic control.
 */

const { Orchestrator } = require('@a3s-lab/code');

async function main() {
  console.log('=== Agent Orchestrator Node.js Example ===\n');

  // Create orchestrator
  console.log('1. Creating orchestrator...');
  const orch = Orchestrator.create();
  console.log(`   Active SubAgents: ${orch.activeCount()}\n`);

  // Spawn SubAgent 1
  console.log('2. Spawning SubAgent 1...');
  const config1 = {
    agentType: 'general',
    description: 'Code analysis task',
    prompt: 'Use glob to find all JavaScript files',
    permissive: true,
    maxSteps: 10
  };
  const handle1 = orch.spawnSubagent(config1);
  console.log(`   SubAgent 1 ID: ${handle1.id}\n`);

  // Spawn SubAgent 2
  console.log('3. Spawning SubAgent 2...');
  const config2 = {
    agentType: 'explore',
    description: 'Documentation check',
    prompt: 'Check if README.md exists',
    permissive: true,
    maxSteps: 5
  };
  const handle2 = orch.spawnSubagent(config2);
  console.log(`   SubAgent 2 ID: ${handle2.id}\n`);

  // Monitor execution
  console.log('4. Monitoring execution...');
  for (let i = 0; i < 15; i++) {
    await sleep(200);
    const active = orch.activeCount();
    const state1 = handle1.state();
    const state2 = handle2.state();

    console.log(`   [${(i * 0.2).toFixed(1)}s] Active: ${active}, State1: ${state1.substring(0, 20)}, State2: ${state2.substring(0, 20)}`);

    // Test control operations
    if (i === 3) {
      console.log('\n   -> Pausing SubAgent 1...');
      handle1.pause();
    }

    if (i === 6) {
      console.log('   -> Resuming SubAgent 1...\n');
      handle1.resume();
    }

    // Check if all done
    if (active === 0) {
      console.log(`\n   All SubAgents completed at ${(i * 0.2).toFixed(1)}s`);
      break;
    }
  }

  // Collect results
  console.log('\n5. Collecting results...');
  try {
    const result1 = handle1.wait();
    console.log(`   SubAgent 1: ${result1.substring(0, 80)}...`);
  } catch (e) {
    console.log(`   SubAgent 1 error: ${e.message}`);
  }

  try {
    const result2 = handle2.wait();
    console.log(`   SubAgent 2: ${result2.substring(0, 80)}...`);
  } catch (e) {
    console.log(`   SubAgent 2 error: ${e.message}`);
  }

  // Final status
  console.log('\n6. Final status:');
  console.log(`   Active SubAgents: ${orch.activeCount()}`);
  console.log(`   SubAgent 1 state: ${handle1.state()}`);
  console.log(`   SubAgent 2 state: ${handle2.state()}`);

  console.log('\n=== Example completed ===');
  console.log('\nNote: This example uses placeholder execution.');
  console.log('For real LLM execution, integrate AgentLoop in SubAgentWrapper.');
}

function sleep(ms) {
  return new Promise(resolve => setTimeout(resolve, ms));
}

main().catch(console.error);
