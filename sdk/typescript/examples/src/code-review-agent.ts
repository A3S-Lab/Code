/**
 * Code Review Agent - Complete Example
 *
 * A comprehensive example that combines multiple features:
 * - Persistent file storage
 * - Read-only permissions
 * - HITL confirmation for git commands
 * - Task tracking for review items
 * - Context management
 * - Provider configuration
 *
 * This demonstrates how to build a secure, production-ready code review agent.
 */

import { A3sClient, loadConfigFromDir, StorageType, TimeoutAction, SessionLane } from '@a3s-lab/code';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const configDir = join(__dirname, '..', '..', '..', '..', '.a3s');

async function createCodeReviewAgent(): Promise<string> {
  console.log('='.repeat(60));
  console.log('Code Review Agent - Complete Example');
  console.log('='.repeat(60));
  console.log();

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
    configDir: configDir
  });

  try {
    // Step 1: Create persistent session with file storage
    console.log('Step 1: Creating persistent session...');
    const session = await client.createSession({
      name: 'Code Review Agent',
      workspace: '/tmp/code-review-workspace',
      storageType: StorageType.STORAGE_TYPE_FILE,  // Persist across restarts
      systemPrompt: `You are a code review assistant. Your role is to:
- Analyze code for bugs, security issues, and best practices
- Provide constructive feedback
- Track review progress using the task list
- Focus on code quality and maintainability

You have read-only access to the codebase. You can read files, search code, and run git commands to view history, but you cannot modify files or push changes.`,
      maxContextLength: 200000,
      autoCompact: true  // Auto-manage context
    });

    const sessionId = session.sessionId;
    console.log(`✓ Session created: ${sessionId}`);
    console.log('  Storage: File (persistent)');
    console.log('  Workspace: /tmp/code-review-workspace');
    console.log();

    // Step 2: Configure read-only permissions
    console.log('Step 2: Configuring read-only permissions...');
    await client.setPermissionPolicy(sessionId, {
      // Allow: Read operations and safe git commands
      allowRules: [
        'Read(*)',
        'Glob(*)',
        'Grep(*)',
        'Bash(git:log*)',
        'Bash(git:diff*)',
        'Bash(git:show*)',
        'Bash(git:status*)',
        'Bash(git:branch*)',
        'Bash(ls:*)',
        'Bash(pwd:*)',
        'Bash(cat:*)',
        'Bash(head:*)',
        'Bash(tail:*)'
      ],

      // Deny: Write operations and dangerous git commands
      denyRules: [
        'Write(*)',
        'Edit(*)',
        'Bash(git:push*)',
        'Bash(git:commit*)',
        'Bash(git:reset*)',
        'Bash(rm:*)',
        'Bash(sudo:*)'
      ],

      // Ask: Other bash commands (for safety)
      askRules: [
        'Bash(*)'
      ]
    });
    console.log('✓ Permissions configured:');
    console.log('  ✓ Read-only access to codebase');
    console.log('  ✓ Safe git commands allowed');
    console.log('  ✓ Write operations blocked');
    console.log();

    // Step 3: Configure HITL for git commands
    console.log('Step 3: Configuring HITL confirmation...');
    await client.setConfirmationPolicy(sessionId, {
      enabled: true,

      // Auto-approve read operations
      autoApproveTools: ['Read', 'Glob', 'Grep'],

      // Require confirmation for bash commands
      requireConfirmTools: ['Bash'],

      defaultTimeoutMs: 30000,
      timeoutAction: TimeoutAction.TIMEOUT_ACTION_REJECT,

      // YOLO mode for Query lane (read operations)
      yoloLanes: [SessionLane.SESSION_LANE_QUERY]
    });
    console.log('✓ HITL configured:');
    console.log('  ✓ Auto-approve: Read, Glob, Grep');
    console.log('  ✓ Require confirm: Bash commands');
    console.log('  ✓ YOLO lanes: Query (read operations)');
    console.log();

    // Step 4: Set up review task list
    console.log('Step 4: Setting up review task list...');
    await client.setTodos(sessionId, [
      {
        id: '1',
        content: 'Review authentication module for security issues',
        status: 'pending',
        priority: 'high'
      },
      {
        id: '2',
        content: 'Check for SQL injection vulnerabilities',
        status: 'pending',
        priority: 'high'
      },
      {
        id: '3',
        content: 'Verify error handling and logging',
        status: 'pending',
        priority: 'medium'
      },
      {
        id: '4',
        content: 'Review API endpoint security',
        status: 'pending',
        priority: 'high'
      },
      {
        id: '5',
        content: 'Check code style and best practices',
        status: 'pending',
        priority: 'low'
      }
    ]);
    console.log('✓ Review tasks created:');
    console.log('  - 3 high priority tasks');
    console.log('  - 1 medium priority task');
    console.log('  - 1 low priority task');
    console.log();

    // Step 5: Start code review
    console.log('Step 5: Starting code review...');
    console.log('  Analyzing codebase structure...');

    const structureResponse = await client.generate(sessionId, {
      messages: [{
        role: 'user',
        content: 'Please analyze the codebase structure. List the main directories and files, and identify the authentication module.'
      }]
    });

    console.log('✓ Structure analysis complete');
    console.log(`  ${structureResponse.content.substring(0, 200)}...`);
    console.log();

    // Step 6: Review authentication module
    console.log('Step 6: Reviewing authentication module...');
    const authReviewResponse = await client.generate(sessionId, {
      messages: [{
        role: 'user',
        content: 'Review the authentication module (src/auth/) for security issues. Check for: password hashing, session management, input validation, and SQL injection vulnerabilities. Mark task 1 as in_progress.'
      }]
    });

    console.log('✓ Authentication review complete');
    console.log(`  ${authReviewResponse.content.substring(0, 200)}...`);
    console.log();

    // Step 7: Check context usage
    console.log('Step 7: Monitoring context usage...');
    const usage = await client.getContextUsage(sessionId);
    const usagePercent = (usage.totalTokens / 200000) * 100;

    console.log('✓ Context status:');
    console.log(`  Tokens: ${usage.totalTokens} / 200000 (${usagePercent.toFixed(1)}%)`);
    console.log(`  Messages: ${usage.messageCount}`);

    if (usagePercent > 75) {
      console.log('  ⚠️ Context getting full, auto-compact will trigger soon');
    }
    console.log();

    // Step 8: Get task progress
    console.log('Step 8: Checking review progress...');
    const todos = await client.getTodos(sessionId);
    const stats = {
      total: todos.todos.length,
      completed: todos.todos.filter(t => t.status === 'completed').length,
      inProgress: todos.todos.filter(t => t.status === 'in_progress').length,
      pending: todos.todos.filter(t => t.status === 'pending').length
    };

    console.log('✓ Review progress:');
    console.log(`  Total tasks: ${stats.total}`);
    console.log(`  Completed: ${stats.completed}`);
    console.log(`  In Progress: ${stats.inProgress}`);
    console.log(`  Pending: ${stats.pending}`);
    console.log(`  Progress: ${((stats.completed / stats.total) * 100).toFixed(1)}%`);
    console.log();

    // Step 9: Generate review summary
    console.log('Step 9: Generating review summary...');
    const summaryResponse = await client.generate(sessionId, {
      messages: [{
        role: 'user',
        content: 'Provide a summary of the code review so far. What issues have been found? What tasks are remaining?'
      }]
    });

    console.log('✓ Review summary:');
    console.log(`  ${summaryResponse.content}`);
    console.log();

    // Step 10: Session info
    console.log('Step 10: Session information...');
    console.log('✓ Session details:');
    console.log(`  ID: ${sessionId}`);
    console.log(`  Name: ${session.config?.name}`);
    console.log(`  Storage: File (persists across restarts)`);
    console.log(`  Workspace: ${session.config?.workspace}`);
    console.log(`  Created: ${new Date(session.createdAt * 1000).toLocaleString()}`);
    console.log();

    console.log('='.repeat(60));
    console.log('Code Review Agent Setup Complete');
    console.log('='.repeat(60));
    console.log();
    console.log('The agent is now ready for code review tasks.');
    console.log('Key features enabled:');
    console.log('  ✓ Persistent storage (survives restarts)');
    console.log('  ✓ Read-only permissions (safe)');
    console.log('  ✓ HITL confirmation (controlled)');
    console.log('  ✓ Task tracking (organized)');
    console.log('  ✓ Auto-context management (efficient)');
    console.log();
    console.log(`Session ID: ${sessionId}`);
    console.log('You can resume this session later by reconnecting to the agent.');

    return sessionId;

  } catch (error) {
    console.error('Error:', error);
    throw error;
  }
}

// Run the example
createCodeReviewAgent()
  .then(sessionId => {
    console.log();
    console.log('✓ Code review agent is ready!');
    console.log(`  Session ID: ${sessionId}`);
  })
  .catch(console.error);
