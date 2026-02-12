/**
 * Structured Generation Example
 *
 * Demonstrates how to generate structured output with JSON Schema:
 * - Unary structured generation
 * - Streaming structured generation
 * - Using schemas for type-safe LLM output
 */

import { A3sClient } from '@a3s-lab/code';

async function structuredGenerationExample(): Promise<void> {
  console.log('='.repeat(60));
  console.log('Structured Generation Example');
  console.log('='.repeat(60));
  console.log();

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
  });

  try {
    // Create a session
    console.log('1. Creating session...');
    const session = await client.createSession({
      name: 'structured-demo',
      workspace: '/tmp/structured-test',
      systemPrompt: 'You are a helpful assistant that returns structured data.',
    });
    const sessionId = session.sessionId;
    console.log(`✓ Session created: ${sessionId}`);
    console.log();

    // Example 1: Extract structured data from text
    console.log('2. Extracting structured data from text...');
    const personSchema = JSON.stringify({
      type: 'object',
      properties: {
        name: { type: 'string', description: 'Full name' },
        age: { type: 'integer', description: 'Age in years' },
        email: { type: 'string', format: 'email' },
        skills: {
          type: 'array',
          items: { type: 'string' },
          description: 'List of technical skills',
        },
      },
      required: ['name', 'age', 'email', 'skills'],
    });

    const personResponse = await client.generateStructured(
      sessionId,
      [
        {
          role: 'ROLE_USER',
          content:
            'Extract the person\'s info: John Smith is a 32-year-old ' +
            'developer at john@example.com who knows Python, Rust, and TypeScript.',
        },
      ],
      personSchema,
    );

    console.log('✓ Structured response:');
    if (personResponse.data) {
      const data = JSON.parse(personResponse.data);
      console.log(`  Name: ${data.name}`);
      console.log(`  Age: ${data.age}`);
      console.log(`  Email: ${data.email}`);
      console.log(`  Skills: ${data.skills?.join(', ')}`);
    }
    console.log();

    // Example 2: Generate a list of items
    console.log('3. Generating a structured task list...');
    const taskSchema = JSON.stringify({
      type: 'object',
      properties: {
        tasks: {
          type: 'array',
          items: {
            type: 'object',
            properties: {
              title: { type: 'string' },
              priority: { type: 'string', enum: ['high', 'medium', 'low'] },
              estimated_hours: { type: 'number' },
            },
            required: ['title', 'priority', 'estimated_hours'],
          },
        },
        total_hours: { type: 'number' },
      },
      required: ['tasks', 'total_hours'],
    });

    const taskResponse = await client.generateStructured(
      sessionId,
      [{ role: 'ROLE_USER', content: 'Create a task list for building a REST API with authentication.' }],
      taskSchema,
    );

    console.log('✓ Task list:');
    if (taskResponse.data) {
      const data = JSON.parse(taskResponse.data);
      for (const task of data.tasks || []) {
        console.log(`  [${task.priority.toUpperCase()}] ${task.title} (${task.estimated_hours}h)`);
      }
      console.log(`  Total: ${data.total_hours}h`);
    }
    console.log();

    // Example 3: Streaming structured generation
    console.log('4. Streaming structured generation...');
    const reviewSchema = JSON.stringify({
      type: 'object',
      properties: {
        summary: { type: 'string' },
        issues: {
          type: 'array',
          items: {
            type: 'object',
            properties: {
              severity: { type: 'string', enum: ['critical', 'warning', 'info'] },
              description: { type: 'string' },
              line: { type: 'integer' },
            },
            required: ['severity', 'description'],
          },
        },
        score: { type: 'integer', minimum: 0, maximum: 100 },
      },
      required: ['summary', 'issues', 'score'],
    });

    process.stdout.write('   Streaming: ');
    let finalData = '';
    for await (const chunk of client.streamGenerateStructured(
      sessionId,
      [
        {
          role: 'ROLE_USER',
          content:
            'Review this code:\n```python\ndef divide(a, b):\n    return a / b\n\n' +
            'result = divide(10, 0)\nprint(result)\n```',
        },
      ],
      reviewSchema,
    )) {
      if (chunk.data) {
        finalData = chunk.data;
        process.stdout.write('.');
      }
      if (chunk.done) {
        console.log(' done!');
        break;
      }
    }

    if (finalData) {
      const data = JSON.parse(finalData);
      console.log('✓ Code review:');
      console.log(`  Summary: ${data.summary}`);
      console.log(`  Score: ${data.score}/100`);
      for (const issue of data.issues || []) {
        const lineInfo = issue.line ? ` (line ${issue.line})` : '';
        console.log(`  [${issue.severity.toUpperCase()}]${lineInfo} ${issue.description}`);
      }
    }
    console.log();

    // Clean up
    console.log('5. Cleaning up...');
    await client.destroySession(sessionId);
    console.log('✓ Session destroyed');
    console.log();

    console.log('='.repeat(60));
    console.log('Structured generation example complete! ✓');
    console.log('='.repeat(60));
  } catch (error) {
    console.error('Error:', error);
  }
}

structuredGenerationExample();
