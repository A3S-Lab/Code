/**
 * AgentSlot smoke test with Kimi model (Node.js / TypeScript).
 *
 * Verifies that the new AgentSlot interface and spawn() method work correctly
 * with a real LLM (kimi-k2.5 via local proxy).
 */

import { Agent, Orchestrator, AgentSlot } from '@a3s-lab/code';
import * as path from 'path';
import * as fs from 'fs';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

async function main() {
  console.log('=== AgentSlot + Kimi Test (Node.js) ===\n');

  const configPath = path.join(
    __dirname, '..', '..', '..', '..', '..', '.a3s', 'config.hcl'
  );
  if (!fs.existsSync(configPath)) {
    console.error(`✗ Config not found: ${configPath}`);
    process.exit(1);
  }
  console.log(`✓ Config: ${configPath}\n`);

  // Create agent
  console.log('Creating agent...');
  const agent = await Agent.create(configPath);
  console.log('✓ Agent created\n');

  // Orchestrator backed by a real agent
  console.log('Creating orchestrator (fromAgent mode)...');
  const orch = Orchestrator.create(agent);
  console.log('✓ Orchestrator created\n');

  // Test 1: spawn() via AgentSlot — standalone, no role
  console.log('Test 1: spawn() via AgentSlot (standalone, no role)...');
  const slot: AgentSlot = {
    agentType: 'general',
    prompt: 'Reply with exactly: SLOT_OK',
    description: 'AgentSlot smoke test',
    permissive: true,
    maxSteps: 3,
  };
  const handle = orch.spawn(slot);
  console.log(`  ✓ Subagent spawned: ${handle.id}`);
  const result = handle.wait();
  console.log(`  ✓ Result: ${JSON.stringify(result)}\n`);

  // Test 2: spawn() with role field present (standalone, role carried but ignored)
  console.log("Test 2: spawn() with role='worker' (standalone mode)...");
  const slotWorker: AgentSlot = {
    agentType: 'general',
    prompt: 'Reply with exactly: WORKER_OK',
    description: 'Worker slot smoke test',
    role: 'worker',
    permissive: true,
    maxSteps: 3,
  };
  const handle2 = orch.spawn(slotWorker);
  console.log(`  ✓ Subagent spawned: ${handle2.id}`);
  const result2 = handle2.wait();
  console.log(`  ✓ Result: ${JSON.stringify(result2)}\n`);

  console.log('=== All AgentSlot tests passed ✓ ===');
}

main().catch((e) => {
  console.error('Test failed:', e);
  process.exit(1);
});
