/**
 * Skill Management Example
 *
 * Demonstrates how to:
 * - List available skills
 * - Load skills dynamically
 * - Use skill capabilities
 * - Unload skills
 */

import { A3sClient } from '@a3s-lab/code';

async function skillManagementExample(): Promise<void> {
  console.log('='.repeat(60));
  console.log('Skill Management Example');
  console.log('='.repeat(60));
  console.log();

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
  });

  try {
    // Create a session
    console.log('1. Creating session...');
    const session = await client.createSession({
      name: 'skill-demo',
      workspace: '/tmp/skill-test',
      systemPrompt: 'You are a helpful assistant with access to skills.',
    });
    console.log(`✓ Session created: ${session.sessionId}`);
    console.log();

    // List available skills
    console.log('2. Listing available skills...');
    const skillsList = await client.listSkills();
    console.log(`✓ Found ${skillsList.skills.length} skills:`);
    skillsList.skills.forEach(skill => {
      console.log(`  - ${skill.name}: ${skill.description}`);
    });
    console.log();

    // Load a skill
    console.log('3. Loading "remotion-best-practices" skill...');
    const loadResult = await client.loadSkill(
      session.sessionId,
      'remotion-best-practices'
    );
    console.log(`✓ Skill loaded: ${loadResult.success}`);
    console.log(`  Message: ${loadResult.message}`);
    console.log();

    // Use the skill in a generation
    console.log('4. Using the skill...');
    const response = await client.generate(session.sessionId, [
      {
        role: 'user',
        content: 'How do I create a video with Remotion?',
      },
    ]);
    console.log('✓ Response:');
    console.log(`  ${response.message?.content.substring(0, 200)}...`);
    console.log();

    // List skills again to see loaded skills
    console.log('5. Listing skills (after loading)...');
    const skillsListAfter = await client.listSkills(session.sessionId);
    console.log(`✓ Session has ${skillsListAfter.skills.length} skills loaded`);
    console.log();

    // Unload the skill
    console.log('6. Unloading skill...');
    const unloadResult = await client.unloadSkill(
      session.sessionId,
      'remotion-best-practices'
    );
    console.log(`✓ Skill unloaded: ${unloadResult.success}`);
    console.log();

    // Clean up
    await client.destroySession(session.sessionId);
    console.log('✓ Session destroyed');
    console.log();

    console.log('='.repeat(60));
    console.log('Skill management example completed! ✓');
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
  skillManagementExample().catch(error => {
    console.error('Fatal error:', error);
    process.exit(1);
  });
}

export { skillManagementExample };
