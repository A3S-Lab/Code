/**
 * Quick Start: Orchestrator Monitoring with Kimi
 *
 * A minimal example showing the core monitoring features.
 *
 * Usage:
 *   export MOONSHOT_API_KEY=your_api_key
 *   npx tsx quickstart_monitoring.ts
 */

import { Orchestrator, SubAgentConfig } from '@a3s-lab/code';

async function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function main() {
  // Check API key
  if (!process.env.MOONSHOT_API_KEY) {
    console.error('Error: Set MOONSHOT_API_KEY environment variable');
    process.exit(1);
  }

  // Create orchestrator
  const orch = Orchestrator.create();
  console.log('✓ Orchestrator created\n');

  // Spawn 2 SubAgents
  const config1: SubAgentConfig = {
    agentType: 'explore',
    description: 'Find TypeScript files',
    prompt: 'Use glob to find all TypeScript files',
    permissive: true,
    maxSteps: 3,
  };

  const config2: SubAgentConfig = {
    agentType: 'analyze',
    description: 'Search TODOs',
    prompt: 'Use grep to find TODO comments',
    permissive: true,
    maxSteps: 3,
  };

  const handle1 = orch.spawnSubagent(config1);
  const handle2 = orch.spawnSubagent(config2);
  console.log(`✓ Spawned: ${handle1.id}`);
  console.log(`✓ Spawned: ${handle2.id}\n`);

  // Monitor for 3 seconds
  console.log('Monitoring for 3 seconds...\n');
  for (let i = 0; i < 3; i++) {
    // Get all SubAgent info
    const subagents = orch.listSubagents();

    console.log(`[${i + 1}s] Active: ${orch.activeCount()}`);
    for (const info of subagents) {
      const activity = info.currentActivity?.activityType || 'None';
      console.log(`  ${info.id}: ${info.state} | Activity: ${activity}`);
    }

    await sleep(1000);
    console.log();
  }

  // Pause first SubAgent
  console.log(`Pausing ${handle1.id}...`);
  orch.pauseSubagent(handle1.id);
  await sleep(500);

  let info = orch.getSubagentInfo(handle1.id);
  if (info) {
    console.log(`  State: ${info.state}\n`);
  }

  // Resume
  console.log(`Resuming ${handle1.id}...`);
  orch.resumeSubagent(handle1.id);
  await sleep(500);

  info = orch.getSubagentInfo(handle1.id);
  if (info) {
    console.log(`  State: ${info.state}\n`);
  }

  // Wait for completion
  console.log('Waiting for all to complete...');
  orch.waitAll();

  // Final states
  console.log('\nFinal states:');
  const states = orch.getAllStates();
  for (const entry of states) {
    console.log(`  ${entry.id}: ${entry.state}`);
  }

  console.log('\n✓ Done!');
}

main().catch((error) => {
  console.error('Error:', error);
  process.exit(1);
});
