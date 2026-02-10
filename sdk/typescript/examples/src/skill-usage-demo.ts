/**
 * Skill Usage Demo
 *
 * Demonstrates how to use skills in chat scenarios:
 * - Loading skills dynamically
 * - Using skill-provided tools
 * - Combining multiple skills
 * - Skill-based code generation
 */

import { A3sClient, StorageType } from '@a3s-lab/code';

async function main() {
  const client = new A3sClient({ address: 'localhost:4088' });

  try {
    console.log('🎯 Skill Usage Demo\n');

    // Create session
    const session = await client.createSession({
      name: 'skill-demo',
      workspace: '/tmp/skill-workspace',
      storageType: StorageType.STORAGE_TYPE_MEMORY,
    });

    const sessionId = session.sessionId;
    console.log(`Session: ${sessionId}\n`);

    // ========================================================================
    // Step 1: Discover available skills
    // ========================================================================
    console.log('📚 Step 1: Discovering Skills\n');

    const skillsResponse = await client.listSkills();
    console.log(`Found ${skillsResponse.skills.length} skills:\n`);

    for (const skill of skillsResponse.skills) {
      console.log(`  📦 ${skill.name}`);
      console.log(`     ${skill.description}`);
      console.log(`     Tools: ${skill.tools.join(', ')}`);
      console.log();
    }

    // ========================================================================
    // Step 2: Load skills into session
    // ========================================================================
    console.log('🔧 Step 2: Loading Skills\n');

    const loadedSkills: string[] = [];

    for (const skill of skillsResponse.skills) {
      try {
        const loadResult = await client.loadSkill(sessionId, skill.name);
        if (loadResult.success) {
          console.log(`✓ Loaded: ${skill.name}`);
          console.log(`  Tools available: ${loadResult.toolNames.join(', ')}`);
          loadedSkills.push(skill.name);
        }
      } catch (error) {
        console.log(`✗ Failed to load: ${skill.name}`);
      }
    }
    console.log();

    if (loadedSkills.length === 0) {
      console.log('⚠️  No skills loaded. Using default tools only.\n');
    }

    // ========================================================================
    // Step 3: Use skills in conversation
    // ========================================================================
    console.log('💬 Step 3: Using Skills in Conversation\n');

    // Example 1: Code generation with skills
    console.log('User: Create a React component using best practices\n');
    console.log('Assistant: ');

    for await (const chunk of client.streamGenerate(sessionId, [
      {
        role: 'user',
        content: `Create a React component for a todo list with:
- Add/remove/toggle todos
- TypeScript types
- Modern React hooks
- Clean code structure

Use any available skills to help.`
      }
    ])) {
      if (chunk.type === 'content') {
        process.stdout.write(chunk.content);
      } else if (chunk.type === 'tool_call' && chunk.toolCall) {
        console.log(`\n\n🔧 Using tool: ${chunk.toolCall.name}`);
      } else if (chunk.type === 'tool_result' && chunk.toolResult) {
        if (chunk.toolResult.success) {
          console.log('   ✓ Tool executed successfully');
        } else if (chunk.toolResult.error) {
          console.log(`   ✗ Tool error: ${chunk.toolResult.error}`);
        }
      }
    }
    console.log('\n\n');

    // Example 2: Multi-skill usage
    console.log('User: Help me set up a new TypeScript project\n');
    console.log('Assistant: ');

    for await (const chunk of client.streamGenerate(sessionId, [
      {
        role: 'user',
        content: `Help me set up a new TypeScript project with:
1. package.json with necessary dependencies
2. tsconfig.json with strict settings
3. Basic project structure (src/, tests/, etc.)
4. README.md with setup instructions

Use skills if available to create the files.`
      }
    ])) {
      if (chunk.type === 'content') {
        process.stdout.write(chunk.content);
      } else if (chunk.type === 'tool_call' && chunk.toolCall) {
        console.log(`\n\n🔧 Tool: ${chunk.toolCall.name}`);
      }
    }
    console.log('\n\n');

    // ========================================================================
    // Step 4: Skill-specific tasks
    // ========================================================================
    console.log('🎨 Step 4: Skill-Specific Tasks\n');

    // Task 1: Code review
    console.log('User: Review this code and suggest improvements\n');

    const codeToReview = `
function getData(id) {
  return fetch('/api/data/' + id)
    .then(res => res.json())
    .then(data => data)
    .catch(err => console.log(err));
}
`;

    console.log('Code to review:');
    console.log(codeToReview);
    console.log('\nAssistant: ');

    const reviewResponse = await client.generate(sessionId, [
      {
        role: 'user',
        content: `Review this code and suggest improvements:\n\n${codeToReview}\n\nConsider: TypeScript, error handling, async/await, and best practices.`
      }
    ]);

    if (reviewResponse.message?.content) {
      console.log(reviewResponse.message.content);
    }
    console.log('\n');

    // Task 2: Generate tests
    console.log('User: Generate unit tests for the improved code\n');
    console.log('Assistant: ');

    for await (const chunk of client.streamGenerate(sessionId, [
      {
        role: 'user',
        content: 'Now generate comprehensive unit tests for the improved version using Jest and TypeScript.'
      }
    ])) {
      if (chunk.type === 'content') {
        process.stdout.write(chunk.content);
      }
    }
    console.log('\n\n');

    // ========================================================================
    // Step 5: Unload skills
    // ========================================================================
    console.log('🔄 Step 5: Managing Skills\n');

    // Unload a skill
    if (loadedSkills.length > 0) {
      const skillToUnload = loadedSkills[0];
      console.log(`Unloading skill: ${skillToUnload}`);

      const unloadResult = await client.unloadSkill(sessionId, skillToUnload);
      if (unloadResult.success) {
        console.log(`✓ Unloaded: ${skillToUnload}`);
        console.log(`  Removed tools: ${unloadResult.removedTools.join(', ')}`);
      }
    }
    console.log();

    // List currently loaded skills
    const currentSkills = await client.listSkills(sessionId);
    console.log('Currently loaded skills:');
    if (currentSkills.skills.length > 0) {
      currentSkills.skills.forEach(skill => {
        console.log(`  - ${skill.name}`);
      });
    } else {
      console.log('  (none)');
    }
    console.log();

    // ========================================================================
    // Summary
    // ========================================================================
    console.log('📊 Summary\n');

    const usage = await client.getContextUsage(sessionId);
    if (usage.usage) {
      console.log('Session statistics:');
      console.log(`  Messages: ${usage.usage.messageCount}`);
      console.log(`  Total tokens: ${usage.usage.totalTokens}`);
      console.log(`  Skills used: ${loadedSkills.length}`);
    }

    console.log('\n✓ Skill demo completed!');

    // Cleanup
    await client.destroySession(sessionId);

  } catch (error) {
    console.error('Error:', error);
  } finally {
    client.close();
  }
}

main();
