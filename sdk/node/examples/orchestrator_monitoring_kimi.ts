/**
 * Orchestrator Real-time Monitoring Demo with Kimi Model
 *
 * This example demonstrates:
 * 1. Creating an orchestrator with Kimi LLM
 * 2. Spawning multiple SubAgents with different tasks
 * 3. Real-time monitoring of SubAgent states and activities
 * 4. Dynamic control (pause, resume, cancel)
 * 5. Querying SubAgent information and activities
 *
 * Requirements:
 * - Set MOONSHOT_API_KEY environment variable
 * - npm install @a3s-lab/code
 *
 * Usage:
 * - export MOONSHOT_API_KEY=your_api_key
 * - npx tsx orchestrator_monitoring_kimi.ts
 */

import { Orchestrator, SubAgentConfig, SubAgentInfo } from '@a3s-lab/code';

async function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function main() {
  console.log('=== Orchestrator Real-time Monitoring with Kimi ===\n');

  // Check API key
  const apiKey = process.env.MOONSHOT_API_KEY;
  if (!apiKey) {
    console.error('Error: MOONSHOT_API_KEY environment variable not set');
    console.error('Please set it with: export MOONSHOT_API_KEY=your_api_key');
    process.exit(1);
  }

  // 1. Create orchestrator
  console.log('✓ Creating Orchestrator with memory-based event communication\n');
  const orchestrator = Orchestrator.create();

  // 2. Configure SubAgents with different tasks
  const configs: SubAgentConfig[] = [
    {
      agentType: 'explore',
      description: '探索 TypeScript 代码库',
      prompt: '使用 glob 工具查找所有 TypeScript 文件，并统计文件数量',
      permissive: true,
      maxSteps: 5,
    },
    {
      agentType: 'analyze',
      description: '分析代码质量',
      prompt: '使用 grep 工具搜索 TODO 和 FIXME 注释',
      permissive: true,
      maxSteps: 5,
    },
    {
      agentType: 'document',
      description: '生成文档',
      prompt: '使用 read 工具读取 package.json 文件并总结内容',
      permissive: true,
      maxSteps: 5,
    },
  ];

  // 3. Spawn SubAgents
  console.log('Spawning 3 SubAgents...');
  const handles = [];
  for (const config of configs) {
    const handle = orchestrator.spawnSubagent(config);
    handles.push(handle);
    console.log(`  ✓ Spawned: ${handle.id} (${config.agentType})`);
  }

  console.log(`\n✓ Total active SubAgents: ${orchestrator.activeCount()}\n`);

  // 4. Real-time monitoring loop
  console.log('=== Real-time Monitoring (5 snapshots) ===\n');

  for (let snapshot = 1; snapshot <= 5; snapshot++) {
    console.log(`--- Snapshot #${snapshot} ---`);
    console.log(`Time: ${new Date().toLocaleTimeString()}`);

    // Get all SubAgent information
    const subagents = orchestrator.listSubagents();
    console.log(`Active SubAgents: ${orchestrator.activeCount()}/${subagents.length}\n`);

    for (const info of subagents) {
      console.log(`SubAgent: ${info.id}`);
      console.log(`  Type: ${info.agentType}`);
      console.log(`  Description: ${info.description}`);
      console.log(`  State: ${info.state}`);
      console.log(`  Created: ${info.createdAt}`);

      if (info.currentActivity) {
        const activity = info.currentActivity;
        console.log(`  Current Activity: ${activity.activityType}`);
        if (activity.data) {
          console.log(`    Data: ${activity.data}`);
        }
      } else {
        console.log(`  Current Activity: None`);
      }
      console.log();
    }

    // Get all active activities
    const activities = orchestrator.getActiveActivities();
    if (activities.length > 0) {
      console.log('Active Activities:');
      for (const entry of activities) {
        console.log(`  ${entry.id}: ${entry.activity.activityType}`);
        if (entry.activity.data) {
          console.log(`    ${entry.activity.data}`);
        }
      }
      console.log();
    }

    // Wait before next snapshot
    await sleep(1000);
  }

  // 5. Demonstrate control operations
  console.log('\n=== Control Operations Demo ===\n');

  if (handles.length > 0) {
    const firstId = handles[0].id;

    // Pause
    console.log(`Pausing ${firstId}...`);
    orchestrator.pauseSubagent(firstId);
    await sleep(500);

    let info = orchestrator.getSubagentInfo(firstId);
    if (info) {
      console.log(`  State after pause: ${info.state}`);
      if (info.currentActivity) {
        console.log(`  Activity: ${info.currentActivity.activityType}`);
      }
    }
    console.log();

    // Resume
    console.log(`Resuming ${firstId}...`);
    orchestrator.resumeSubagent(firstId);
    await sleep(500);

    info = orchestrator.getSubagentInfo(firstId);
    if (info) {
      console.log(`  State after resume: ${info.state}`);
    }
    console.log();
  }

  // 6. Query specific SubAgent
  console.log('=== Query Specific SubAgent ===\n');
  if (handles.length > 0) {
    const targetId = handles[0].id;
    const info = orchestrator.getSubagentInfo(targetId);

    if (info) {
      console.log(`SubAgent Details:`);
      console.log(`  ID: ${info.id}`);
      console.log(`  Type: ${info.agentType}`);
      console.log(`  Description: ${info.description}`);
      console.log(`  State: ${info.state}`);
      console.log(`  Parent ID: ${info.parentId || 'None'}`);
      console.log(`  Created At: ${info.createdAt}`);
      console.log(`  Updated At: ${info.updatedAt}`);

      if (info.currentActivity) {
        console.log(`  Current Activity:`);
        console.log(`    Type: ${info.currentActivity.activityType}`);
        if (info.currentActivity.data) {
          console.log(`    Data: ${info.currentActivity.data}`);
        }
      }
      console.log();
    }
  }

  // 7. Get all states
  console.log('=== All SubAgent States ===\n');
  const states = orchestrator.getAllStates();
  for (const entry of states) {
    console.log(`${entry.id}: ${entry.state}`);
  }
  console.log();

  // 8. Wait for all to complete
  console.log('Waiting for all SubAgents to complete...');
  orchestrator.waitAll();

  // 9. Final status
  console.log('\n=== Final Status ===\n');
  const finalStates = orchestrator.getAllStates();
  for (const entry of finalStates) {
    console.log(`${entry.id}: ${entry.state}`);
  }

  console.log('\n✓ Demo completed successfully!');
}

main().catch((error) => {
  console.error('Error:', error);
  process.exit(1);
});
