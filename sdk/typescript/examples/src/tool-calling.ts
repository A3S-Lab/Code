/**
 * Tool Calling — Multi-Step Agent Example
 *
 * Demonstrates the tool() helper and multi-step agent behavior:
 * - tool() for defining client-side tools
 * - maxSteps for multi-step agent loops
 * - onToolCall for logging/intercepting tool calls
 * - onStepFinish for step-level progress tracking
 * - Tools without execute (handled by onToolCall)
 */

import { generateText, streamText, createProvider, tool } from '@a3s-lab/code';

const openai = createProvider({
  name: 'openai',
  apiKey: process.env.OPENAI_API_KEY || 'sk-xxx',
});

// Define tools
const weather = tool({
  description: 'Get current weather for a city',
  parameters: {
    type: 'object',
    properties: {
      city: { type: 'string', description: 'City name' },
    },
    required: ['city'],
  },
  execute: async ({ city }) => {
    // Simulate API call
    const temps: Record<string, number> = {
      Tokyo: 22,
      Paris: 18,
      'New York': 25,
      London: 15,
    };
    return {
      city,
      temperature: temps[city as string] ?? 20,
      unit: 'celsius',
      condition: 'partly cloudy',
    };
  },
});

const calculator = tool({
  description: 'Perform arithmetic calculations',
  parameters: {
    type: 'object',
    properties: {
      expression: { type: 'string', description: 'Math expression to evaluate' },
    },
    required: ['expression'],
  },
  execute: async ({ expression }) => {
    // Simple safe eval for demo
    const result = Function(`"use strict"; return (${expression})`)();
    return { expression, result };
  },
});

async function main() {
  console.log('='.repeat(60));
  console.log('Tool Calling — Multi-Step Agent');
  console.log('='.repeat(60));
  console.log();

  // Example 1: generateText with tools
  console.log('=== Example 1: generateText() with tools ===\n');

  const { text, steps, toolCalls } = await generateText({
    model: openai('gpt-4o'),
    prompt: 'What is the weather in Tokyo and Paris? Also calculate 42 * 17.',
    tools: { weather, calculator },
    maxSteps: 5,
    onStepFinish: (step) => {
      console.log(`  Step ${step.stepIndex}: ${step.toolCalls.length} tool call(s), text: ${step.text.length} chars`);
    },
    onToolCall: ({ toolName, args }) => {
      console.log(`  🔧 Calling ${toolName}(${JSON.stringify(args)})`);
    },
  });

  console.log();
  console.log(`✓ Final response: ${text}`);
  console.log(`  Total steps: ${steps.length}`);
  console.log(`  Total tool calls: ${toolCalls.length}`);
  console.log();

  // Example 2: streamText with tools
  console.log('=== Example 2: streamText() with tools ===\n');

  const result = streamText({
    model: openai('gpt-4o'),
    prompt: 'Check the weather in London and New York. Be brief.',
    tools: { weather },
    maxSteps: 5,
    onToolCall: ({ toolName, args }) => {
      console.log(`  🔧 ${toolName}(${JSON.stringify(args)})`);
    },
  });

  process.stdout.write('  ');
  for await (const chunk of result.textStream) {
    process.stdout.write(chunk);
  }
  console.log('\n');

  const streamSteps = await result.steps;
  console.log(`  Completed in ${streamSteps.length} step(s)`);
  console.log();

  // Example 3: Tool without execute (handled by onToolCall)
  console.log('=== Example 3: Tool without execute ===\n');

  const getUser = tool({
    description: 'Get user profile by ID',
    parameters: {
      type: 'object',
      properties: {
        userId: { type: 'string', description: 'User ID' },
      },
      required: ['userId'],
    },
    // No execute — handled by onToolCall
  });

  const { text: userText } = await generateText({
    model: openai('gpt-4o'),
    prompt: 'Look up user profile for user-123',
    tools: { getUser },
    maxSteps: 3,
    onToolCall: async ({ toolName, args }) => {
      console.log(`  🔧 ${toolName}(${JSON.stringify(args)})`);
      if (toolName === 'getUser') {
        // Return value becomes the tool result
        return { name: 'Alice', role: 'admin', email: 'alice@example.com' };
      }
    },
  });

  console.log(`✓ Response: ${userText}`);
  console.log();

  console.log('='.repeat(60));
  console.log('Tool calling demo complete! ✓');
  console.log('='.repeat(60));
}

main().catch(console.error);
