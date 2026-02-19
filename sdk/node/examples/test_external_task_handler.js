/**
 * A3S Code Node.js SDK - External Task Handler Integration Test
 *
 * Demonstrates the Multi-Machine External Task pattern:
 * 1. Coordinator creates a session with Execute lane set to External mode
 * 2. Agent sends a prompt that triggers bash/write/edit tool calls
 * 3. Coordinator polls pendingExternalTasks() in a background interval
 * 4. Coordinator processes tasks locally (simulating a remote worker)
 * 5. Coordinator completes tasks via completeExternalTask()
 *
 * Run with: node examples/test_external_task_handler.js
 */

const { Agent } = require('../index.js');
const { execSync } = require('child_process');
const path = require('path');
const fs = require('fs');
const os = require('os');

function findConfig() {
  const homeConfig = path.join(os.homedir(), '.a3s', 'config.hcl');
  if (fs.existsSync(homeConfig)) return homeConfig;

  const projectConfig = path.resolve(__dirname, '..', '..', '..', '..', '..', '.a3s', 'config.hcl');
  if (fs.existsSync(projectConfig)) return projectConfig;

  throw new Error('Config file not found');
}

/**
 * Simulate a remote worker executing a bash command.
 */
function workerExecuteBash(command, workingDir = '.') {
  try {
    const output = execSync(command, {
      cwd: workingDir,
      encoding: 'utf-8',
      timeout: 30000,
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    return { success: true, output, exitCode: 0, error: null };
  } catch (e) {
    return {
      success: false,
      output: e.stdout || '',
      exitCode: e.status || 1,
      error: e.stderr || e.message,
    };
  }
}

async function testExternalTaskHandler() {
  console.log('\n📦 Test 1: External Task Handler (Execute Lane → External)');
  console.log('-'.repeat(80));

  const configPath = findConfig();
  const agent = await Agent.create(configPath);

  // 1. Create session with queue enabled
  const session = agent.session('.', {
    queueConfig: {
      enableAllFeatures: true,
      timeoutMs: 60000,
    },
  });

  // 2. Route Execute lane to External mode
  await session.setLaneHandler('execute', {
    mode: 'external',
    timeoutMs: 60000,
  });

  console.log('✓ Session created with Execute lane → External mode');
  console.log('  Query lane: Internal (read, glob, grep run locally)');
  console.log('  Execute lane: External (bash, write, edit → ExternalTask)');
  console.log();

  // 3. Start a background poller for external tasks
  const start = Date.now();
  let externalTasksProcessed = 0;
  let stopPolling = false;

  const pollInterval = setInterval(async () => {
    if (stopPolling) return;

    try {
      const tasks = await session.pendingExternalTasks();
      if (!Array.isArray(tasks)) return;

      for (const task of tasks) {
        const taskId = task.task_id;
        const cmdType = task.command_type;
        console.log(`  📥 ExternalTaskPending: ${taskId.slice(0, 8)} (${cmdType})`);

        // Execute the task (simulating a remote worker)
        let result;
        if (cmdType === 'bash') {
          const cmd = task.payload?.command || "echo 'no command'";
          const cwd = task.payload?.working_dir || '.';
          result = workerExecuteBash(cmd, cwd);
        } else {
          result = {
            success: true,
            output: `External handler processed: ${cmdType}`,
            exitCode: 0,
            error: null,
          };
        }

        console.log(`  🔧 Worker result: success=${result.success}, exit_code=${result.exitCode}`);
        const preview = (result.output || '').trim().slice(0, 60);
        if (preview) console.log(`     Output: ${preview}`);

        // Complete the external task
        const completed = await session.completeExternalTask(taskId, {
          success: result.success,
          result: { output: result.output, exit_code: result.exitCode },
          error: result.error,
        });

        if (completed) {
          externalTasksProcessed++;
          console.log(`  📤 Task ${taskId.slice(0, 8)} completed and returned to agent`);
        }
      }
    } catch (e) {
      // Session might be done
      if (!e.message?.includes('exhausted')) {
        console.log(`  ⚠ Poll error: ${e.message}`);
      }
    }
  }, 200); // Poll every 200ms

  // 4. Send the prompt — agent will produce ExternalTask objects for bash calls
  //    Note: Node SDK stream() collects all events, so we use send() + polling
  let result;
  try {
    result = await session.send(
      "Run these bash commands and tell me the results:\n" +
      "1. echo 'Hello from external worker'\n" +
      "2. date '+%Y-%m-%d %H:%M:%S'\n" +
      "3. uname -s"
    );
  } finally {
    stopPolling = true;
    clearInterval(pollInterval);
  }

  const duration = (Date.now() - start) / 1000;

  console.log();
  console.log('-'.repeat(80));
  console.log('📊 Results:');
  console.log(`  Duration: ${duration.toFixed(2)}s`);
  console.log(`  External tasks processed: ${externalTasksProcessed}`);
  console.log(`  Response length: ${result.text.length} chars`);
  console.log(`  Tool calls: ${result.toolCallsCount}`);

  return session;
}

async function testHybridMode(session) {
  console.log('\n\n📦 Test 2: Hybrid Mode (Execute Lane → Hybrid)');
  console.log('-'.repeat(80));

  // Switch to Hybrid mode
  await session.setLaneHandler('execute', {
    mode: 'hybrid',
    timeoutMs: 60000,
  });

  console.log('✓ Execute lane switched to Hybrid mode');
  console.log('  Tools execute locally AND emit ExternalTaskPending events');
  console.log();

  const start = Date.now();
  const result = await session.send("Run: echo 'hybrid mode test'");
  const duration = (Date.now() - start) / 1000;

  console.log(`✓ Completed in ${duration.toFixed(2)}s`);
  console.log(`  Response: ${result.text.slice(0, 120)}`);
  console.log(`  Tool calls: ${result.toolCallsCount}`);
}

async function testDynamicLaneSwitching(session) {
  console.log('\n\n📦 Test 3: Dynamic Lane Switching');
  console.log('-'.repeat(80));

  // Switch back to Internal
  await session.setLaneHandler('execute', {
    mode: 'internal',
    timeoutMs: 60000,
  });

  console.log('✓ Execute lane switched back to Internal mode');

  const start = Date.now();
  const result = await session.send("Run: echo 'back to internal mode'");
  const duration = (Date.now() - start) / 1000;

  console.log(`✓ Completed in ${duration.toFixed(2)}s`);
  console.log(`  Response: ${result.text.slice(0, 120)}`);
}

async function main() {
  console.log('🚀 A3S Code - External Task Handler Integration Test\n');
  console.log('='.repeat(80));
  console.log(`📄 Config: ${findConfig()}`);
  console.log('='.repeat(80));

  const session = await testExternalTaskHandler();
  await testHybridMode(session);
  await testDynamicLaneSwitching(session);

  console.log('\n' + '='.repeat(80));
  console.log('✅ All external task handler tests completed!');
  console.log('='.repeat(80));
}

main().catch(console.error);
