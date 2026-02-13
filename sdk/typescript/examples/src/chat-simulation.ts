/**
 * Chat Simulation — Multi-Turn Conversation
 *
 * Demonstrates both API styles for multi-turn chat:
 * - Session-based: A3sClient with manual session management
 * - High-level: createChat() with automatic session management
 *
 * Both support tool calling, streaming, and context management.
 */

import {
  A3sClient,
  createChat,
  createProvider,
  tool,
  StorageType,
} from '@a3s-lab/code';

function printUser(message: string) {
  console.log(`\x1b[36m👤 User:\x1b[0m ${message}\n`);
}

function printAssistant(message: string) {
  console.log(`\x1b[32m🤖 Assistant:\x1b[0m ${message}\n`);
}

async function main() {
  console.log('='.repeat(60));
  console.log('Chat Simulation — Multi-Turn Conversation');
  console.log('='.repeat(60));
  console.log();

  // ========================================================================
  // Part 1: Session-Based Chat (A3sClient)
  // ========================================================================
  console.log('=== Part 1: Session-Based Chat (A3sClient) ===\n');

  const client = new A3sClient({ address: 'localhost:4088' });

  try {
    // Create a persistent session
    const session = await client.createSession({
      name: 'code-assistant-chat',
      workspace: '/tmp/chat-workspace',
      systemPrompt: `You are a helpful coding assistant. Be concise.`,
      storageType: StorageType.STORAGE_TYPE_FILE,
      autoCompact: true,
    });
    const sessionId = session.sessionId;
    console.log(`Session created: ${sessionId}\n`);

    // Turn 1: Generate
    printUser('Write a TypeScript function that validates an email address');
    const response1 = await client.generate(sessionId, [
      { role: 'user', content: 'Write a TypeScript function that validates an email address' },
    ]);
    if (response1.message?.content) {
      printAssistant(response1.message.content);
    }

    // Turn 2: Follow-up (session remembers context)
    printUser('Can you add unit tests for this function?');
    const response2 = await client.generate(sessionId, [
      { role: 'user', content: 'Can you add unit tests for this function?' },
    ]);
    if (response2.message?.content) {
      printAssistant(response2.message.content);
    }

    // Turn 3: Streaming
    printUser('Create a REST API endpoint for user registration');
    process.stdout.write('\x1b[32m🤖 Assistant:\x1b[0m ');
    for await (const chunk of client.streamGenerate(sessionId, [
      { role: 'user', content: 'Create a REST API endpoint for user registration. Be brief.' },
    ])) {
      if (chunk.type === 'content') {
        process.stdout.write(chunk.content);
      }
    }
    console.log('\n');

    // Context management
    const usage = await client.getContextUsage(sessionId);
    if (usage.usage) {
      console.log(`Context: ${usage.usage.totalTokens} tokens, ${usage.usage.messageCount} messages`);
    }

    // Keep session for later (persistent storage)
    console.log(`Session preserved: ${sessionId}\n`);

  } finally {
    client.close();
  }

  // ========================================================================
  // Part 2: High-Level Chat (createChat)
  // ========================================================================
  console.log('=== Part 2: High-Level Chat (createChat) ===\n');
  console.log('Same conversation, but with automatic session management.\n');

  const openai = createProvider({
    name: 'openai',
    apiKey: process.env.OPENAI_API_KEY || 'sk-xxx',
  });

  const chat = createChat({
    model: openai('gpt-4o'),
    workspace: '/tmp/chat-workspace',
    system: 'You are a helpful coding assistant. Be concise.',
    tools: {
      lookup: tool({
        description: 'Look up documentation for a library',
        parameters: {
          type: 'object',
          properties: {
            library: { type: 'string', description: 'Library name' },
          },
          required: ['library'],
        },
        execute: async ({ library }) => ({
          library,
          docs: `Documentation for ${library}: https://docs.example.com/${library}`,
        }),
      }),
    },
    maxSteps: 3,
    onToolCall: ({ toolName, args }) => {
      console.log(`  🔧 Tool: ${toolName}(${JSON.stringify(args)})`);
    },
  });

  try {
    // Turn 1: send()
    printUser('Write a function to parse JSON safely');
    const { text: reply1, steps } = await chat.send('Write a function to parse JSON safely');
    printAssistant(reply1);
    console.log(`  (${steps.length} step(s))\n`);

    // Turn 2: stream()
    printUser('Now add error handling');
    process.stdout.write('\x1b[32m🤖 Assistant:\x1b[0m ');
    const { textStream } = chat.stream('Now add error handling');
    for await (const chunk of textStream) {
      process.stdout.write(chunk);
    }
    console.log('\n');

    // Turn 3: Tool usage
    printUser('Look up the docs for zod');
    const { text: reply3 } = await chat.send('Look up the docs for zod');
    printAssistant(reply3);

    // Context management
    const chatUsage = await chat.getUsage();
    if (chatUsage) {
      console.log(`Context: ${chatUsage.totalTokens} tokens, ${chatUsage.messageCount} messages`);
    }

  } finally {
    await chat.close();
    console.log('✓ Chat closed');
  }

  console.log();
  console.log('='.repeat(60));
  console.log('Chat simulation complete! ✓');
  console.log('='.repeat(60));
}

main().catch(console.error);
