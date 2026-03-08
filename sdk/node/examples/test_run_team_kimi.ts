/**
 * run_team smoke test with Kimi model (Node.js / TypeScript).
 *
 * Tests Orchestrator.runTeam() — Lead decomposes a goal, Workers execute,
 * Reviewer approves. Uses kimi-k2.5 via local proxy.
 */

import { Agent, Orchestrator, AgentSlot } from '@a3s-lab/code';
import * as path from 'path';
import * as fs from 'fs';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

async function main() {
  console.log('=== run_team + Kimi Test (Node.js) ===\n');

  const configPath = path.join(
    __dirname, '..', '..', '..', '..', '..', '.a3s', 'config.hcl'
  );
  if (!fs.existsSync(configPath)) {
    console.error(`✗ Config not found: ${configPath}`);
    process.exit(1);
  }
  console.log(`✓ Config: ${configPath}\n`);

  console.log('Creating agent...');
  const agent = await Agent.create(configPath);
  console.log('✓ Agent created\n');

  console.log('Creating orchestrator (fromAgent mode)...');
  const orch = Orchestrator.create(agent);
  console.log('✓ Orchestrator created\n');

  const slots: AgentSlot[] = [
    {
      agentType: 'general',
      role: 'lead',
      prompt: '',
      description: 'Lead: decompose the goal into tasks',
      permissive: true,
      maxSteps: 5,
    },
    {
      agentType: 'general',
      role: 'worker',
      prompt: '',
      description: 'Worker: execute assigned tasks',
      permissive: true,
      maxSteps: 5,
    },
    {
      agentType: 'general',
      role: 'reviewer',
      prompt: '',
      description: 'Reviewer: approve or reject results',
      permissive: true,
      maxSteps: 3,
    },
  ];

  console.log('Running team: Lead → Worker → Reviewer...');
  const result = await orch.runTeam(
    'List 3 common JavaScript data structures and briefly describe each',
    '.',
    slots,
  );

  console.log(`\n✓ Done tasks:     ${result.doneTasks.length}`);
  console.log(`✓ Rejected tasks: ${result.rejectedTasks.length}`);
  console.log(`✓ Rounds:         ${result.rounds}`);

  for (const task of result.doneTasks) {
    console.log(`\n  Task: ${JSON.stringify(task.description)}`);
    console.log(`  Result: ${JSON.stringify(task.result)}`);
  }

  console.log('\n=== run_team test passed ✓ ===');
}

main().catch((e) => {
  console.error('Test failed:', e);
  process.exit(1);
});
