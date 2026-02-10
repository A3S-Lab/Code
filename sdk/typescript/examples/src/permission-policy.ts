/**
 * Permission Policy Example
 *
 * Demonstrates how to:
 * - Set permission policies
 * - Allow/deny specific tool executions
 * - Check permissions before execution
 * - Add permission rules dynamically
 */

import { A3sClient } from '@a3s-lab/code';

async function permissionPolicyExample(): Promise<void> {
  console.log('='.repeat(60));
  console.log('Permission Policy Example');
  console.log('='.repeat(60));
  console.log();

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
  });

  try {
    // Create a session
    console.log('1. Creating session...');
    const session = await client.createSession({
      name: 'permission-demo',
      workspace: '/tmp/permission-test',
      systemPrompt: 'You are a helpful assistant.',
    });
    console.log(`✓ Session created: ${session.sessionId}`);
    console.log();

    // Set a permission policy
    console.log('2. Setting permission policy...');
    await client.setPermissionPolicy(session.sessionId, {
      defaultDecision: 'PERMISSION_DECISION_ASK',
      rules: [
        {
          pattern: 'read(*)',
          decision: 'PERMISSION_DECISION_ALLOW',
          description: 'Allow all read operations',
        },
        {
          pattern: 'bash(rm:*)',
          decision: 'PERMISSION_DECISION_DENY',
          description: 'Deny all rm commands',
        },
        {
          pattern: 'write(*)',
          decision: 'PERMISSION_DECISION_ASK',
          description: 'Ask before writing files',
        },
      ],
    });
    console.log('✓ Permission policy set');
    console.log();

    // Get the policy
    console.log('3. Getting permission policy...');
    const policy = await client.getPermissionPolicy(session.sessionId);
    console.log('✓ Current policy:');
    console.log(`  Default decision: ${policy.policy?.defaultDecision}`);
    console.log(`  Rules: ${policy.policy?.rules.length}`);
    policy.policy?.rules.forEach(rule => {
      console.log(`    - ${rule.pattern}: ${rule.decision}`);
    });
    console.log();

    // Check specific permissions
    console.log('4. Checking permissions...');

    const readCheck = await client.checkPermission(
      session.sessionId,
      'read',
      { path: '/tmp/test.txt' }
    );
    console.log(`  read(/tmp/test.txt): ${readCheck.decision}`);

    const rmCheck = await client.checkPermission(
      session.sessionId,
      'bash',
      { command: 'rm -rf /' }
    );
    console.log(`  bash(rm -rf /): ${rmCheck.decision}`);

    const writeCheck = await client.checkPermission(
      session.sessionId,
      'write',
      { path: '/tmp/output.txt' }
    );
    console.log(`  write(/tmp/output.txt): ${writeCheck.decision}`);
    console.log();

    // Add a new rule dynamically
    console.log('5. Adding new permission rule...');
    await client.addPermissionRule(session.sessionId, {
      pattern: 'bash(echo:*)',
      decision: 'PERMISSION_DECISION_ALLOW',
      description: 'Allow echo commands',
    });
    console.log('✓ Rule added');
    console.log();

    // Test with actual generation
    console.log('6. Testing with generation (allowed operation)...');
    const response = await client.generate(session.sessionId, [
      {
        role: 'user',
        content: 'Read the file /tmp/test.txt',
      },
    ]);
    console.log('✓ Generation completed (read allowed)');
    console.log();

    // Clean up
    await client.destroySession(session.sessionId);
    console.log('✓ Session destroyed');
    console.log();

    console.log('='.repeat(60));
    console.log('Permission policy example completed! ✓');
    console.log('='.repeat(60));
  } catch (error) {
    console.error('✗ Error:', (error as Error).message);
    throw error;
  } finally {
    client.close();
  }
}

// Run if executed directly
if (import.meta.url === `file://${process.argv[1]}`) {
  permissionPolicyExample().catch(error => {
    console.error('Fatal error:', error);
    process.exit(1);
  });
}

export { permissionPolicyExample };
