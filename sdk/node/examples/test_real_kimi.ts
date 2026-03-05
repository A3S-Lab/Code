/**
 * Real-world test of Orchestrator monitoring with actual Kimi API.
 * Uses the config from a3s/.a3s/config.hcl
 *
 * This test will:
 * 1. Create an Orchestrator
 * 2. Spawn 3 SubAgents with real tasks
 * 3. Monitor their execution in real-time
 * 4. Demonstrate control operations
 * 5. Show all monitoring APIs in action
 */

import { Agent, Orchestrator, SubAgentConfig } from '@a3s-lab/code';
import * as path from 'path';
import * as fs from 'fs';

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function main() {
  console.log('='.repeat(70));
  console.log('Orchestrator Real-World Test with Kimi API');
  console.log('='.repeat(70));
  console.log();

  // Get config path
  const configPath = path.join(__dirname, '..', '..', '..', '..', '..', '.a3s', 'config.hcl');
  if (!fs.existsSync(configPath)) {
    console.error(`✗ Config file not found: ${configPath}`);
    process.exit(1);
  }

  console.log(`✓ Using config: ${configPath}`);
  console.log();

  // Create Agent with config
  console.log('Creating Agent with Kimi configuration...');
  let agent: Agent;
  try {
    agent = await Agent.create(configPath);
    console.log('✓ Agent created successfully');
  } catch (e) {
    console.error(`✗ Failed to create agent: ${e}`);
    process.exit(1);
  }

  console.log();

  // Create Orchestrator
  console.log('Creating Orchestrator...');
  let orch: Orchestrator;
  try {
    orch = Orchestrator.create();
    console.log('✓ Orchestrator created');
  } catch (e) {
    console.error(`✗ Failed to create orchestrator: ${e}`);
    process.exit(1);
  }

  console.log();

  // Configure SubAgents with real tasks
  console.log('Configuring SubAgents with real tasks...');
  const configs: SubAgentConfig[] = [
    {
      agentType: 'explore',
      description: '探索代码库结构',
      prompt: '使用 glob 工具查找当前目录下的所有 .ts 文件，并统计数量',
      permissive: true,
      maxSteps: 3,
    },
    {
      agentType: 'analyze',
      description: '分析 TODO 注释',
      prompt: '使用 grep 工具搜索所有 TODO 注释，并列出前 5 个',
      permissive: true,
      maxSteps: 3,
    },
    {
      agentType: 'document',
      description: '读取 README',
      prompt: '使用 read 工具读取 README.md 文件的前 10 行',
      permissive: true,
      maxSteps: 2,
    },
  ];

  // Spawn SubAgents
  console.log('\nSpawning SubAgents...');
  const handles = [];
  for (let i = 0; i < configs.length; i++) {
    const config = configs[i];
    try {
      const handle = orch.spawnSubagent(config);
      handles.push(handle);
      console.log(`  ${i + 1}. ✓ Spawned: ${handle.id} (${config.agentType})`);
      console.log(`     Task: ${config.description}`);
    } catch (e) {
      console.log(`  ${i + 1}. ✗ Failed to spawn: ${e}`);
    }
  }

  console.log(`\n✓ Total SubAgents spawned: ${handles.length}`);
  console.log(`✓ Active count: ${orch.activeCount()}`);
  console.log();

  // Real-time monitoring
  console.log('='.repeat(70));
  console.log('Real-time Monitoring (10 snapshots, 1 second interval)');
  console.log('='.repeat(70));
  console.log();

  for (let snapshot = 1; snapshot <= 10; snapshot++) {
    console.log(`--- Snapshot #${snapshot} (${new Date().toLocaleTimeString()}) ---`);

    // Get all SubAgent information
    try {
      const subagents = orch.listSubagents();
      const activeCount = orch.activeCount();
      console.log(`Active: ${activeCount}/${subagents.length}`);
      console.log();

      for (const info of subagents) {
        // Basic info
        console.log(`📋 ${info.id}`);
        console.log(`   Type: ${info.agentType}`);
        console.log(`   State: ${info.state}`);
        console.log(`   Description: ${info.description}`);

        // Current activity
        if (info.currentActivity) {
          const activity = info.currentActivity;
          console.log(`   🔄 Activity: ${activity.activityType}`);
          if (activity.data) {
            // Parse and display activity data
            try {
              const data = JSON.parse(activity.data);
              if (activity.activityType === 'calling_tool') {
                console.log(`      Tool: ${data.tool_name || 'unknown'}`);
              } else if (activity.activityType === 'requesting_llm') {
                console.log(`      Messages: ${data.message_count || 0}`);
              } else if (activity.activityType === 'waiting_for_control') {
                console.log(`      Reason: ${data.reason || 'unknown'}`);
              }
            } catch (e) {
              // Ignore parse errors
            }
          }
        } else {
          console.log(`   🔄 Activity: None`);
        }

        console.log();
      }

      // Show active activities summary
      const activities = orch.getActiveActivities();
      if (activities.length > 0) {
        console.log('Active Activities Summary:');
        for (const entry of activities) {
          console.log(`  • ${entry.id}: ${entry.activity.activityType}`);
        }
        console.log();
      }
    } catch (e) {
      console.log(`✗ Monitoring error: ${e}`);
    }

    // Wait before next snapshot
    await sleep(1000);
  }

  // Demonstrate control operations
  console.log();
  console.log('='.repeat(70));
  console.log('Control Operations Demo');
  console.log('='.repeat(70));
  console.log();

  if (handles.length > 0) {
    const targetId = handles[0].id;

    // Pause
    console.log(`1. Pausing ${targetId}...`);
    try {
      orch.pauseSubagent(targetId);
      await sleep(500);

      const info = orch.getSubagentInfo(targetId);
      if (info) {
        console.log(`   ✓ State: ${info.state}`);
        if (info.currentActivity) {
          console.log(`   ✓ Activity: ${info.currentActivity.activityType}`);
        }
      }
    } catch (e) {
      console.log(`   ✗ Failed: ${e}`);
    }

    console.log();

    // Resume
    console.log(`2. Resuming ${targetId}...`);
    try {
      orch.resumeSubagent(targetId);
      await sleep(500);

      const info = orch.getSubagentInfo(targetId);
      if (info) {
        console.log(`   ✓ State: ${info.state}`);
      }
    } catch (e) {
      console.log(`   ✗ Failed: ${e}`);
    }

    console.log();
  }

  // Query specific SubAgent
  console.log('='.repeat(70));
  console.log('Query Specific SubAgent');
  console.log('='.repeat(70));
  console.log();

  if (handles.length > 0) {
    const targetId = handles[0].id;
    console.log(`Querying ${targetId}...`);

    try {
      const info = orch.getSubagentInfo(targetId);
      if (info) {
        console.log(`  ID: ${info.id}`);
        console.log(`  Type: ${info.agentType}`);
        console.log(`  Description: ${info.description}`);
        console.log(`  State: ${info.state}`);
        console.log(`  Parent ID: ${info.parentId || 'None'}`);
        console.log(`  Created: ${info.createdAt}`);
        console.log(`  Updated: ${info.updatedAt}`);

        if (info.currentActivity) {
          console.log(`  Current Activity:`);
          console.log(`    Type: ${info.currentActivity.activityType}`);
          if (info.currentActivity.data) {
            console.log(`    Data: ${info.currentActivity.data}`);
          }
        }
      }
    } catch (e) {
      console.log(`  ✗ Failed: ${e}`);
    }
  }

  console.log();

  // Get all states
  console.log('='.repeat(70));
  console.log('All SubAgent States');
  console.log('='.repeat(70));
  console.log();

  try {
    const states = orch.getAllStates();
    for (const entry of states) {
      console.log(`  ${entry.id}: ${entry.state}`);
    }
  } catch (e) {
    console.log(`✗ Failed: ${e}`);
  }

  console.log();

  // Wait for all to complete
  console.log('='.repeat(70));
  console.log('Waiting for all SubAgents to complete...');
  console.log('='.repeat(70));
  console.log();

  try {
    orch.waitAll();
    console.log('✓ All SubAgents completed');
  } catch (e) {
    console.log(`✗ Wait failed: ${e}`);
  }

  console.log();

  // Final status
  console.log('='.repeat(70));
  console.log('Final Status');
  console.log('='.repeat(70));
  console.log();

  try {
    const finalStates = orch.getAllStates();
    for (const entry of finalStates) {
      console.log(`  ${entry.id}: ${entry.state}`);
    }
  } catch (e) {
    console.log(`✗ Failed: ${e}`);
  }

  console.log();
  console.log('='.repeat(70));
  console.log('✓ Test completed successfully!');
  console.log('='.repeat(70));
}

main().catch((error) => {
  console.error(`\n\n✗ Test failed with error: ${error}`);
  console.error(error.stack);
  process.exit(1);
});
