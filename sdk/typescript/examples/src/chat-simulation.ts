/**
 * Chat Simulation Demo
 *
 * Demonstrates a realistic chat scenario with:
 * - Multi-turn conversation
 * - Skill loading and usage
 * - Code generation
 * - Streaming responses
 * - Context management
 * - Tool execution
 */

import { A3sClient, StorageType } from '@a3s-lab/code';

// Helper function to print with formatting
function printHeader(title: string) {
  console.log('\n' + '='.repeat(60));
  console.log(`  ${title}`);
  console.log('='.repeat(60) + '\n');
}

function printUser(message: string) {
  console.log(`\x1b[36m👤 User:\x1b[0m ${message}\n`);
}

function printAssistant(message: string) {
  console.log(`\x1b[32m🤖 Assistant:\x1b[0m ${message}\n`);
}

function printSystem(message: string) {
  console.log(`\x1b[33m⚙️  System:\x1b[0m ${message}\n`);
}

function printTool(name: string, result: string) {
  console.log(`\x1b[35m🔧 Tool [${name}]:\x1b[0m`);
  console.log(`   ${result.substring(0, 200)}${result.length > 200 ? '...' : ''}\n`);
}

async function main() {
  const client = new A3sClient({ address: 'localhost:4088' });

  try {
    printHeader('Chat Simulation - Code Assistant');

    // ========================================================================
    // Step 1: Create a persistent chat session
    // ========================================================================
    printSystem('Creating chat session...');

    const session = await client.createSession({
      name: 'code-assistant-chat',
      workspace: '/tmp/chat-workspace',
      systemPrompt: `You are a helpful coding assistant. You can:
- Write and explain code in multiple languages
- Help debug issues
- Suggest best practices
- Use tools to read/write files and execute commands

Always be concise and provide working code examples.`,
      storageType: StorageType.STORAGE_TYPE_FILE,
      autoCompact: true,
    });

    const sessionId = session.sessionId;
    printSystem(`Session created: ${sessionId}`);

    // ========================================================================
    // Step 2: List and load available skills
    // ========================================================================
    printHeader('Loading Skills');

    const skillsResponse = await client.listSkills();
    if (skillsResponse.skills.length > 0) {
      printSystem(`Available skills: ${skillsResponse.skills.map(s => s.name).join(', ')}`);

      // Try to load a skill if available
      for (const skill of skillsResponse.skills) {
        try {
          const loadResult = await client.loadSkill(sessionId, skill.name);
          if (loadResult.success) {
            printSystem(`Loaded skill: ${skill.name} (tools: ${loadResult.toolNames.join(', ')})`);
          }
        } catch (e) {
          // Skill might not be loadable, continue
        }
      }
    } else {
      printSystem('No skills available, using default tools');
    }

    // ========================================================================
    // Step 3: Multi-turn conversation - Code Generation
    // ========================================================================
    printHeader('Conversation: Code Generation');

    // Turn 1: Ask for a simple function
    printUser('Write a TypeScript function that validates an email address');

    const response1 = await client.generate(sessionId, [
      { role: 'user', content: 'Write a TypeScript function that validates an email address' }
    ]);

    if (response1.message?.content) {
      printAssistant(response1.message.content);
    }

    // Check for tool calls
    if (response1.toolCalls && response1.toolCalls.length > 0) {
      for (const toolCall of response1.toolCalls) {
        if (toolCall.result) {
          printTool(toolCall.name, toolCall.result.output || toolCall.result.error);
        }
      }
    }

    // Turn 2: Follow-up question
    printUser('Can you add unit tests for this function?');

    const response2 = await client.generate(sessionId, [
      { role: 'user', content: 'Can you add unit tests for this function?' }
    ]);

    if (response2.message?.content) {
      printAssistant(response2.message.content);
    }

    // ========================================================================
    // Step 4: Streaming response - Complex code generation
    // ========================================================================
    printHeader('Streaming: Complex Code Generation');

    printUser('Create a REST API endpoint for user registration with validation');

    process.stdout.write('\x1b[32m🤖 Assistant:\x1b[0m ');

    let fullResponse = '';
    for await (const chunk of client.streamGenerate(sessionId, [
      { role: 'user', content: 'Create a REST API endpoint for user registration with validation. Use Express.js and TypeScript.' }
    ])) {
      if (chunk.type === 'content') {
        process.stdout.write(chunk.content);
        fullResponse += chunk.content;
      } else if (chunk.type === 'tool_call' && chunk.toolCall) {
        console.log(`\n\x1b[35m🔧 Calling tool: ${chunk.toolCall.name}\x1b[0m`);
      } else if (chunk.type === 'tool_result' && chunk.toolResult) {
        const output = chunk.toolResult.output || chunk.toolResult.error || '';
        console.log(`\x1b[35m   Result: ${output.substring(0, 100)}...\x1b[0m`);
      }
    }
    console.log('\n');

    // ========================================================================
    // Step 5: Code debugging scenario
    // ========================================================================
    printHeader('Conversation: Debugging');

    const buggyCode = `
function calculateTotal(items) {
  let total = 0;
  for (let i = 0; i <= items.length; i++) {
    total += items[i].price * items[i].quantity;
  }
  return total;
}
`;

    printUser(`I have a bug in this code, can you help?\n${buggyCode}`);

    const debugResponse = await client.generate(sessionId, [
      { role: 'user', content: `I have a bug in this code, can you help find and fix it?\n\n${buggyCode}` }
    ]);

    if (debugResponse.message?.content) {
      printAssistant(debugResponse.message.content);
    }

    // ========================================================================
    // Step 6: Context management
    // ========================================================================
    printHeader('Context Management');

    // Check context usage
    const contextUsage = await client.getContextUsage(sessionId);
    if (contextUsage.usage) {
      printSystem(`Context usage:`);
      console.log(`   Total tokens: ${contextUsage.usage.totalTokens}`);
      console.log(`   Prompt tokens: ${contextUsage.usage.promptTokens}`);
      console.log(`   Completion tokens: ${contextUsage.usage.completionTokens}`);
      console.log(`   Message count: ${contextUsage.usage.messageCount}`);
    }

    // Get conversation history
    const messagesResponse = await client.getMessages(sessionId, 5);
    printSystem(`Recent messages (${messagesResponse.totalCount} total):`);
    for (const msg of messagesResponse.messages) {
      const preview = msg.content[0]?.text?.text?.substring(0, 50) || '[complex content]';
      console.log(`   [${msg.role}] ${preview}...`);
    }

    // ========================================================================
    // Step 7: Event subscription (background monitoring)
    // ========================================================================
    printHeader('Event Monitoring');

    printSystem('Starting event monitor for next generation...');

    // Start event subscription
    const eventPromise = (async () => {
      let eventCount = 0;
      for await (const event of client.subscribeEvents(sessionId)) {
        console.log(`   📡 Event: ${event.type} - ${event.message || 'no message'}`);
        eventCount++;
        if (eventCount >= 5 || event.type === 'EVENT_TYPE_GENERATION_COMPLETED') {
          break;
        }
      }
    })();

    // Small delay to ensure subscription is active
    await new Promise(resolve => setTimeout(resolve, 100));

    // Trigger a simple generation
    printUser('What is the time complexity of quicksort?');

    const quickResponse = await client.generate(sessionId, [
      { role: 'user', content: 'What is the time complexity of quicksort? Answer briefly.' }
    ]);

    if (quickResponse.message?.content) {
      printAssistant(quickResponse.message.content);
    }

    // Wait for events with timeout
    await Promise.race([
      eventPromise,
      new Promise(resolve => setTimeout(resolve, 2000))
    ]);

    // ========================================================================
    // Step 8: Skill-based generation (if skills are loaded)
    // ========================================================================
    printHeader('Skill-Based Generation');

    printUser('Using any loaded skills, help me create a React component');

    process.stdout.write('\x1b[32m🤖 Assistant:\x1b[0m ');

    for await (const chunk of client.streamGenerate(sessionId, [
      { role: 'user', content: 'Create a React component for a user profile card with avatar, name, and bio. Use TypeScript and modern React patterns.' }
    ])) {
      if (chunk.type === 'content') {
        process.stdout.write(chunk.content);
      }
    }
    console.log('\n');

    // ========================================================================
    // Step 9: Final context check and cleanup
    // ========================================================================
    printHeader('Session Summary');

    // Final context usage
    const finalUsage = await client.getContextUsage(sessionId);
    if (finalUsage.usage) {
      console.log('Final context usage:');
      console.log(`   Total tokens: ${finalUsage.usage.totalTokens}`);
      console.log(`   Messages: ${finalUsage.usage.messageCount}`);
    }

    // Compact context if needed
    if (finalUsage.usage && finalUsage.usage.totalTokens > 10000) {
      printSystem('Compacting context...');
      const compactResult = await client.compactContext(sessionId);
      if (compactResult.success && compactResult.before && compactResult.after) {
        console.log(`   Before: ${compactResult.before.totalTokens} tokens`);
        console.log(`   After: ${compactResult.after.totalTokens} tokens`);
      }
    }

    // Session info
    const sessionInfo = await client.getSession(sessionId);
    if (sessionInfo.session) {
      console.log('\nSession info:');
      console.log(`   ID: ${sessionInfo.session.sessionId}`);
      console.log(`   State: ${sessionInfo.session.state}`);
      console.log(`   Created: ${new Date(sessionInfo.session.createdAt).toISOString()}`);
    }

    // Ask if user wants to keep the session
    printSystem('Session will be preserved for future conversations.');
    printSystem(`To resume, use session ID: ${sessionId}`);

    // Uncomment to destroy session:
    // await client.destroySession(sessionId);
    // printSystem('Session destroyed');

  } catch (error) {
    console.error('\x1b[31mError:\x1b[0m', error);
  } finally {
    client.close();
  }
}

main();
