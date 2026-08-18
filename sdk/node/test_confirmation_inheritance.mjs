/**
 * Integration test for ConfirmationInheritance in Node SDK.
 *
 * Tests that confirmation inheritance is exposed through the native runtime.
 * The public surface is hermetic by default. Set A3S_CONFIG_FILE and
 * A3S_CODE_SDK_REAL_AGENT_SMOKE=1 to add a real delegated LLM turn.
 */

import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const { Agent } = await import(join(__dirname, 'index.js'));

const inlineConfig = `
default_model = "openai/confirmation-test"

providers "openai" {
  api_key = "hermetic-test-key"
  models "confirmation-test" {
    name = "Confirmation Test"
    tool_call = true
  }
}
`.trim();
const configSource = process.env.A3S_CONFIG_FILE || inlineConfig;

console.log('[node-sdk-confirmation] Starting integration test...');
console.log(
  `[node-sdk-confirmation] Config source: ${process.env.A3S_CONFIG_FILE ? 'external ACL' : 'hermetic inline ACL'}`,
);

const agent = await Agent.create(configSource);
const workspace = mkdtempSync(join(tmpdir(), 'a3s-node-confirmation-'));
console.log(`[node-sdk-confirmation] Workspace: ${workspace}`);

// Test 1: WorkerAgentSpec with confirmation_inheritance field
console.log('[node-sdk-confirmation] Test 1: Create WorkerAgentSpec with confirmation_inheritance');
const workerSpec = {
  name: 'test-writer',
  description: 'Test worker with auto-approve confirmation',
  kind: 'implementer',
  confirmationInheritance: 'auto_approve',
  permissions: {
    rules: ['allow(write(*))'],
    defaultDecision: 'ask',
    enabled: true,
  },
  maxSteps: 3,
};

const session = agent.session(workspace, {
  planningMode: 'disabled',
  permissionPolicy: { defaultDecision: 'allow' },
  workerAgents: [workerSpec],
});

// Test 2: Verify worker was registered
console.log('[node-sdk-confirmation] Test 2: Verify worker registration');
const toolNames = await session.toolNames();
assert.ok(toolNames.includes('task'), 'task tool should be registered');

// Test 3: Register another worker and check AgentDefinition
console.log('[node-sdk-confirmation] Test 3: Register worker and check AgentDefinition');
const workerSpec2 = {
  name: 'test-reader',
  description: 'Test worker with deny-on-ask confirmation',
  kind: 'read_only',
  confirmationInheritance: 'deny_on_ask',
  maxSteps: 2,
};

const agentDef = await session.registerWorkerAgent(workerSpec2);
assert.equal(agentDef.name, 'test-reader');
assert.equal(agentDef.confirmationInheritance, 'deny_on_ask');
console.log(`[node-sdk-confirmation] AgentDefinition.confirmationInheritance: ${agentDef.confirmationInheritance}`);

// Test 4: Real LLM task delegation with confirmation_inheritance
if (process.env.A3S_CODE_SDK_REAL_AGENT_SMOKE === '1') {
  console.log('[node-sdk-confirmation] Test 4: Real LLM task delegation');

  // Create a test file for the worker to read
  const testFile = join(workspace, 'test.txt');
  writeFileSync(testFile, 'CONFIRMATION_TEST_CONTENT');

  const taskResult = await session.task({
    agent: 'test-reader',
    description: 'Read test file',
    prompt: 'Read the file test.txt and reply with its content',
    maxSteps: 2,
  });

  assert.equal(taskResult.exitCode, 0, `Task should succeed: ${taskResult.output}`);
  assert.ok(
    taskResult.output.includes('CONFIRMATION_TEST_CONTENT') ||
    taskResult.output.includes('test.txt'),
    'Task output should reference the test file'
  );
  console.log('[node-sdk-confirmation] Task delegation successful');
} else {
  console.log('[node-sdk-confirmation] Test 4: Skipped (set A3S_CODE_SDK_REAL_AGENT_SMOKE=1 to enable)');
}

await agent.close();
console.log('[node-sdk-confirmation] All tests passed ✓');
