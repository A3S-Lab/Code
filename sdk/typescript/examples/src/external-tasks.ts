/**
 * External Task Handling Example
 *
 * Demonstrates how to delegate task execution to external systems:
 * - Configure lane handlers (Internal/External/Hybrid modes)
 * - Poll and process external tasks
 * - Complete external tasks with results
 * - Use case: Secure sandbox execution
 */

import { A3sClient, loadConfigFromDir, SessionLane, TaskHandlerMode } from '@a3s-lab/code';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const configDir = join(__dirname, '..', '..', '..', '..', '.a3s');

// Simulated external system for task execution
class ExternalTaskExecutor {
  async execute(commandType: string, payload: any): Promise<any> {
    console.log(`  [External System] Executing ${commandType}...`);

    // Simulate execution delay
    await new Promise(resolve => setTimeout(resolve, 1000));

    // Simulate different command types
    if (commandType === 'Bash') {
      return {
        stdout: `Executed: ${payload.command}\nOutput from external system`,
        stderr: '',
        exitCode: 0
      };
    }

    return { result: 'Task completed by external system' };
  }
}

async function runExternalTaskExample(): Promise<void> {
  console.log('='.repeat(60));
  console.log('External Task Handling Example');
  console.log('='.repeat(60));
  console.log();

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
    configDir: configDir
  });

  const executor = new ExternalTaskExecutor();

  try {
    // Create a session
    console.log('1. Creating session...');
    const session = await client.createSession({
      name: 'External Task Demo',
      workspace: '/tmp/workspace',
      systemPrompt: 'You are an assistant that executes tasks in a secure external environment.'
    });
    const sessionId = session.sessionId;
    console.log(`✓ Session created: ${sessionId}`);
    console.log();

    // Configure Execute lane for external handling
    console.log('2. Configuring Execute lane for external handling...');
    await client.setLaneHandler(sessionId, SessionLane.SESSION_LANE_EXECUTE, {
      mode: TaskHandlerMode.TASK_HANDLER_MODE_EXTERNAL,
      timeoutMs: 60000  // 60 second timeout
    });
    console.log('✓ Execute lane configured:');
    console.log('  Mode: External');
    console.log('  Timeout: 60s');
    console.log();

    // Start external task processor
    console.log('3. Starting external task processor...');
    let processorRunning = true;

    const processExternalTasks = async () => {
      while (processorRunning) {
        try {
          // Poll for pending tasks
          const tasks = await client.listPendingExternalTasks(sessionId);

          for (const task of tasks.tasks) {
            console.log(`\n📋 External task received:`);
            console.log(`  Task ID: ${task.taskId}`);
            console.log(`  Lane: ${task.lane}`);
            console.log(`  Command: ${task.commandType}`);
            console.log(`  Timeout: ${task.timeoutMs}ms`);
            console.log(`  Remaining: ${task.remainingMs}ms`);

            try {
              // Parse payload
              const payload = JSON.parse(task.payload);
              console.log(`  Payload: ${JSON.stringify(payload, null, 2)}`);

              // Execute in external system
              const result = await executor.execute(task.commandType, payload);

              // Complete the task
              await client.completeExternalTask(sessionId, task.taskId, {
                success: true,
                result: JSON.stringify(result)
              });

              console.log(`  ✓ Task completed successfully`);

            } catch (error) {
              // Report failure
              await client.completeExternalTask(sessionId, task.taskId, {
                success: false,
                error: error instanceof Error ? error.message : 'Unknown error'
              });

              console.log(`  ✗ Task failed: ${error}`);
            }
          }

          // Poll every 500ms
          await new Promise(resolve => setTimeout(resolve, 500));

        } catch (error) {
          if (processorRunning) {
            console.error('Processor error:', error);
          }
        }
      }
    };

    // Start processor in background
    const processorPromise = processExternalTasks();
    console.log('✓ External task processor started');
    console.log();

    // Example 4: Trigger external task execution
    console.log('4. Triggering external task execution...');
    console.log('  Sending request to execute bash command...');

    const response = await client.generate(sessionId, {
      messages: [{
        role: 'user',
        content: 'Run the command: echo "Hello from external system"'
      }]
    });

    console.log(`\n✓ Response received:`);
    console.log(`  ${response.content.substring(0, 200)}...`);
    console.log();

    // Example 5: Check lane handler configuration
    console.log('5. Checking lane handler configuration...');
    const handlerConfig = await client.getLaneHandler(sessionId, SessionLane.SESSION_LANE_EXECUTE);
    console.log('✓ Current configuration:');
    console.log(`  Mode: ${handlerConfig.mode}`);
    console.log(`  Timeout: ${handlerConfig.timeoutMs}ms`);
    console.log();

    // Example 6: Hybrid mode demonstration
    console.log('6. Configuring Hybrid mode for Generate lane...');
    await client.setLaneHandler(sessionId, SessionLane.SESSION_LANE_GENERATE, {
      mode: TaskHandlerMode.TASK_HANDLER_MODE_HYBRID,
      timeoutMs: 120000
    });
    console.log('✓ Generate lane configured:');
    console.log('  Mode: Hybrid (internal execution + external notification)');
    console.log('  Use case: Monitor LLM calls while executing internally');
    console.log();

    // Wait a bit for any pending tasks
    await new Promise(resolve => setTimeout(resolve, 2000));

    // Cleanup
    console.log('7. Cleanup...');
    processorRunning = false;
    await processorPromise;
    await client.destroySession(sessionId);
    console.log('✓ Session destroyed');
    console.log();

    console.log('='.repeat(60));
    console.log('External Task Handling Example Complete');
    console.log('='.repeat(60));
    console.log();
    console.log('Use cases:');
    console.log('  - Execute commands in secure sandboxes');
    console.log('  - Delegate tasks to specialized systems');
    console.log('  - Monitor and audit tool executions');
    console.log('  - Implement custom execution policies');

  } catch (error) {
    console.error('Error:', error);
    throw error;
  }
}

// Run the example
runExternalTaskExample().catch(console.error);
