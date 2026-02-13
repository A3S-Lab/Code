/**
 * Tool Calling — Multi-Step Agent Example
 *
 * Shows how to use client-side tools with session.generateText()
 * and session.streamText(). The session handles multi-step tool
 * execution loops automatically via maxSteps.
 */

import { A3sClient, createProvider, tool } from '@a3s-lab/code';

const A3S_ADDRESS = process.env.A3S_ADDRESS || 'localhost:4088';

async function main() {
  console.log('=== Tool Calling with Session ===\n');

  const client = new A3sClient({ address: A3S_ADDRESS });
  const openai = createProvider({
    name: 'openai',
    apiKey: process.env.OPENAI_API_KEY || '',
  });

  // Define tools
  const weatherTool = tool({
    description: 'Get the current weather for a city',
    parameters: {
      type: 'object',
      properties: {
        city: { type: 'string', description: 'City name' },
      },
      required: ['city'],
    },
    execute: async ({ city }) => {
      console.log(`  [tool] Getting weather for ${city}...`);
      return {
        city,
        temperature: Math.round(Math.random() * 30 + 5),
        condition: ['sunny', 'cloudy', 'rainy'][Math.floor(Math.random() * 3)],
        humidity: Math.round(Math.random() * 60 + 30),
      };
    },
  });

  const calculatorTool = tool({
    description: 'Evaluate a math expression',
    parameters: {
      type: 'object',
      properties: {
        expression: { type: 'string', description: 'Math expression to evaluate' },
      },
      required: ['expression'],
    },
    execute: async ({ expression }) => {
      console.log(`  [tool] Calculating: ${expression}`);
      try {
        const result = Function(`"use strict"; return (${expression})`)();
        return { expression, result };
      } catch {
        return { expression, error: 'Invalid expression' };
      }
    },
  });

  const tools = { weather: weatherTool, calculator: calculatorTool };

  // --- Example 1: generateText with tools ---
  console.log('--- generateText with tools ---');
  {
    await using session = await client.createSession({
      model: openai('gpt-4o'),
      workspace: '/tmp/a3s-workspace',
      system: 'You are a helpful assistant with access to weather and calculator tools.',
    });

    const { text, steps, toolCalls } = await session.generateText({
      prompt: 'What is the weather in Tokyo and Paris? Also, what is 42 * 17?',
      tools,
      maxSteps: 5,
      onStepFinish: (step) => {
        console.log(`  [Step ${step.stepIndex}] text: ${step.text.length} chars, tools: ${step.toolCalls.length}`);
      },
    });

    console.log('\nFinal response:', text);
    console.log(`Completed in ${steps.length} steps, ${toolCalls.length} tool calls\n`);
  }

  // --- Example 2: streamText with tools ---
  console.log('--- streamText with tools ---');
  {
    await using session = await client.createSession({
      model: openai('gpt-4o'),
      workspace: '/tmp/a3s-workspace',
    });

    const { textStream, steps } = session.streamText({
      prompt: 'Compare the weather in London and New York.',
      tools: { weather: weatherTool },
      maxSteps: 3,
      onToolCall: ({ toolName, args }) => {
        console.log(`  [onToolCall] ${toolName}(${JSON.stringify(args)})`);
      },
    });

    process.stdout.write('Streaming: ');
    for await (const chunk of textStream) {
      process.stdout.write(chunk);
    }
    console.log('\n');
    console.log('Steps:', (await steps).length);
  }

  // --- Example 3: Tool without execute (handled by onToolCall) ---
  console.log('--- Tool without execute (manual handling) ---');
  {
    await using session = await client.createSession({
      model: openai('gpt-4o'),
      workspace: '/tmp/a3s-workspace',
    });

    const manualTool = tool({
      description: 'Look up a user by name',
      parameters: {
        type: 'object',
        properties: {
          name: { type: 'string', description: 'User name to look up' },
        },
        required: ['name'],
      },
      // No execute — result provided by onToolCall
    });

    const { text } = await session.generateText({
      prompt: 'Look up the user "Alice"',
      tools: { lookupUser: manualTool },
      maxSteps: 3,
      onToolCall: ({ toolName, args }) => {
        console.log(`  [manual] ${toolName}(${JSON.stringify(args)})`);
        // Return the result directly from the callback
        return { id: 1, name: args.name, email: `${args.name}@example.com` };
      },
    });

    console.log('Response:', text);
  }

  client.close();
  console.log('\nDone!');
}

main().catch(console.error);
