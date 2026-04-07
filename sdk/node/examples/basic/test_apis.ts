/**
 * Test script to verify Orchestrator monitoring APIs are working correctly.
 * This test uses placeholder execution (no real LLM calls).
 */

import { Orchestrator, SubAgentConfig } from '@a3s-lab/code';

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function testOrchestratorAPIs(): Promise<boolean> {
  console.log('Testing Orchestrator Monitoring APIs...\n');

  // Test 1: Create Orchestrator
  console.log('1. Testing Orchestrator.create()...');
  let orch: Orchestrator;
  try {
    orch = Orchestrator.create();
    console.log('   ✓ Orchestrator created');
  } catch (e) {
    console.log(`   ✗ Failed: ${e}`);
    return false;
  }

  // Test 2: Create SubAgentConfig
  console.log('\n2. Testing SubAgentConfig...');
  let config: SubAgentConfig;
  try {
    config = {
      agentType: 'test',
      description: 'Test SubAgent',
      prompt: 'Test prompt',
      permissive: true,
      maxSteps: 3,
    };
    console.log('   ✓ SubAgentConfig created');
  } catch (e) {
    console.log(`   ✗ Failed: ${e}`);
    return false;
  }

  // Test 3: Spawn SubAgent
  console.log('\n3. Testing spawnSubagent()...');
  let handle;
  try {
    handle = orch.spawnSubagent(config);
    console.log(`   ✓ SubAgent spawned: ${handle.id}`);
  } catch (e) {
    console.log(`   ✗ Failed: ${e}`);
    return false;
  }

  // Test 4: Active count
  console.log('\n4. Testing activeCount()...');
  try {
    const count = orch.activeCount();
    console.log(`   ✓ Active count: ${count}`);
  } catch (e) {
    console.log(`   ✗ Failed: ${e}`);
    return false;
  }

  // Test 5: List SubAgents
  console.log('\n5. Testing listSubagents()...');
  try {
    const subagents = orch.listSubagents();
    console.log(`   ✓ Found ${subagents.length} SubAgent(s)`);
    if (subagents.length > 0) {
      const info = subagents[0];
      console.log(`      - ID: ${info.id}`);
      console.log(`      - Type: ${info.agentType}`);
      console.log(`      - State: ${info.state}`);
      console.log(`      - Description: ${info.description}`);
      if (info.currentActivity) {
        console.log(`      - Activity: ${info.currentActivity.activityType}`);
      }
    }
  } catch (e) {
    console.log(`   ✗ Failed: ${e}`);
    return false;
  }

  // Test 6: Get SubAgent info
  console.log('\n6. Testing getSubagentInfo()...');
  try {
    const info = orch.getSubagentInfo(handle.id);
    if (info) {
      console.log(`   ✓ Got info for ${info.id}`);
      console.log(`      - State: ${info.state}`);
      console.log(`      - Created: ${info.createdAt}`);
      console.log(`      - Updated: ${info.updatedAt}`);
    } else {
      console.log('   ✗ No info returned');
      return false;
    }
  } catch (e) {
    console.log(`   ✗ Failed: ${e}`);
    return false;
  }

  // Test 7: Get active activities
  console.log('\n7. Testing getActiveActivities()...');
  try {
    const activities = orch.getActiveActivities();
    console.log(`   ✓ Found ${activities.length} active activit(ies)`);
    for (const entry of activities) {
      console.log(`      - ${entry.id}: ${entry.activity.activityType}`);
    }
  } catch (e) {
    console.log(`   ✗ Failed: ${e}`);
    return false;
  }

  // Test 8: Get all states
  console.log('\n8. Testing getAllStates()...');
  try {
    const states = orch.getAllStates();
    console.log(`   ✓ Found ${states.length} state(s)`);
    for (const entry of states) {
      console.log(`      - ${entry.id}: ${entry.state}`);
    }
  } catch (e) {
    console.log(`   ✗ Failed: ${e}`);
    return false;
  }

  // Test 9: Pause SubAgent
  console.log('\n9. Testing pauseSubagent()...');
  try {
    orch.pauseSubagent(handle.id);
    await sleep(200);
    const info = orch.getSubagentInfo(handle.id);
    console.log(`   ✓ Paused, state: ${info?.state}`);
  } catch (e) {
    console.log(`   ✗ Failed: ${e}`);
    return false;
  }

  // Test 10: Resume SubAgent
  console.log('\n10. Testing resumeSubagent()...');
  try {
    orch.resumeSubagent(handle.id);
    await sleep(200);
    const info = orch.getSubagentInfo(handle.id);
    console.log(`   ✓ Resumed, state: ${info?.state}`);
  } catch (e) {
    console.log(`   ✗ Failed: ${e}`);
    return false;
  }

  // Test 11: Cancel SubAgent
  console.log('\n11. Testing cancelSubagent()...');
  try {
    orch.cancelSubagent(handle.id);
    await sleep(200);
    const info = orch.getSubagentInfo(handle.id);
    console.log(`   ✓ Cancelled, state: ${info?.state}`);
  } catch (e) {
    console.log(`   ✗ Failed: ${e}`);
    return false;
  }

  console.log('\n' + '='.repeat(50));
  console.log('✓ All API tests passed!');
  console.log('='.repeat(50));
  return true;
}

testOrchestratorAPIs()
  .then((success) => {
    process.exit(success ? 0 : 1);
  })
  .catch((error) => {
    console.error('Error:', error);
    process.exit(1);
  });
