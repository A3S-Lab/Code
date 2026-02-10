/**
 * HITL (Human-in-the-Loop) Confirmation Example
 *
 * Demonstrates how to configure and handle tool execution confirmations:
 * - Setting confirmation policies
 * - Auto-approve vs require-confirm tools
 * - Handling confirmation requests
 * - Timeout behavior
 */

import { A3sClient, loadConfigFromDir, TimeoutAction, SessionLane } from '@a3s-lab/code';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import * as readline from 'readline';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const configDir = join(__dirname, '..', '..', '..', '..', '.a3s');

// Helper function to ask user for confirmation
function askUser(question: string): Promise<boolean> {
  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout
  });

  return new Promise((resolve) => {
    rl.question(`${question} (y/n): `, (answer) => {
      rl.close();
      resolve(answer.toLowerCase() === 'y' || answer.toLowerCase() === 'yes');
    });
  });
}

async function runHitlExample(): Promise<void> {
  console.log('='.repeat(60));
  console.log('HITL (Human-in-the-Loop) Confirmation Example');
  console.log('='.repeat(60));
  console.log();

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
    configDir: configDir
  });

  try {
    // Create a session
    console.log('1. Creating session...');
    const session = await client.createSession({
      name: 'HITL Demo Session',
      workspace: '/tmp/workspace',
      systemPrompt: 'You are a helpful assistant that needs user approval for sensitive operations.'
    });
    const sessionId = session.sessionId;
    console.log(`✓ Session created: ${sessionId}`);
    console.log();

    // Configure HITL policy
    console.log('2. Configuring HITL policy...');
    await client.setConfirmationPolicy(sessionId, {
      enabled: true,

      // Auto-approve safe read operations
      autoApproveTools: ['Read', 'Glob', 'Grep'],

      // Require confirmation for write operations
      requireConfirmTools: ['Bash', 'Write', 'Edit'],

      // 30 second timeout
      defaultTimeoutMs: 30000,

      // Reject on timeout (safer default)
      timeoutAction: TimeoutAction.TIMEOUT_ACTION_REJECT,

      // YOLO mode: auto-approve Query lane tasks
      yoloLanes: [SessionLane.SESSION_LANE_QUERY]
    });
    console.log('✓ HITL policy configured:');
    console.log('  - Auto-approve: Read, Glob, Grep');
    console.log('  - Require confirm: Bash, Write, Edit');
    console.log('  - Timeout: 30s (reject on timeout)');
    console.log('  - YOLO lanes: Query');
    console.log();

    // Subscribe to events to handle confirmations
    console.log('3. Setting up event handler...');
    const eventStream = client.subscribeEvents(sessionId, ['ToolExecutionPending']);

    // Handle confirmation requests in background
    (async () => {
      try {
        for await (const event of eventStream) {
          if (event.type === 'ToolExecutionPending') {
            const { toolName, args, confirmationId } = event.data;

            console.log('\n⚠️  Tool execution pending confirmation:');
            console.log(`  Tool: ${toolName}`);
            console.log(`  Args: ${JSON.stringify(args, null, 2)}`);
            console.log();

            // Ask user for approval
            const approved = await askUser('Approve this tool execution?');

            // Send confirmation
            await client.confirmToolExecution(sessionId, confirmationId, {
              approved,
              reason: approved ? 'User approved' : 'User rejected'
            });

            console.log(approved ? '✓ Approved' : '✗ Rejected');
            console.log();
          }
        }
      } catch (error) {
        // Event stream closed
      }
    })();

    console.log('✓ Event handler ready');
    console.log();

    // Example 4: Test auto-approve (Read operation)
    console.log('4. Testing auto-approve (Read operation)...');
    console.log('  This should execute without confirmation');
    const readResponse = await client.generate(sessionId, {
      messages: [{
        role: 'user',
        content: 'Read the file /tmp/test.txt'
      }]
    });
    console.log(`✓ Read operation completed (auto-approved)`);
    console.log();

    // Example 5: Test require-confirm (Bash operation)
    console.log('5. Testing require-confirm (Bash operation)...');
    console.log('  This will require your confirmation');
    const bashResponse = await client.generate(sessionId, {
      messages: [{
        role: 'user',
        content: 'Run: ls -la /tmp'
      }]
    });
    console.log(`✓ Bash operation completed`);
    console.log();

    // Example 6: Get current policy
    console.log('6. Getting current confirmation policy...');
    const policy = await client.getConfirmationPolicy(sessionId);
    console.log('✓ Current policy:');
    console.log(`  Enabled: ${policy.enabled}`);
    console.log(`  Auto-approve tools: ${policy.autoApproveTools?.join(', ')}`);
    console.log(`  Require confirm tools: ${policy.requireConfirmTools?.join(', ')}`);
    console.log();

    // Cleanup
    console.log('7. Cleanup...');
    await client.destroySession(sessionId);
    console.log('✓ Session destroyed');
    console.log();

    console.log('='.repeat(60));
    console.log('HITL Example Complete');
    console.log('='.repeat(60));

  } catch (error) {
    console.error('Error:', error);
    throw error;
  }
}

// Run the example
runHitlExample().catch(console.error);
