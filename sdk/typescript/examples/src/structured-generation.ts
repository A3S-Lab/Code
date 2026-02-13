/**
 * Structured Generation Example
 *
 * Demonstrates structured output in both styles:
 * - Session-based: client.generateStructured(), client.streamGenerateStructured()
 * - High-level: generateObject(), streamObject()
 */

import {
  A3sClient,
  generateObject,
  streamObject,
  createProvider,
} from '@a3s-lab/code';

async function main() {
  console.log('='.repeat(60));
  console.log('Structured Generation Example');
  console.log('='.repeat(60));
  console.log();

  // ========================================================================
  // Part 1: Session-Based Structured Generation
  // ========================================================================
  console.log('=== Part 1: Session-Based (A3sClient) ===\n');

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
  });

  try {
    const session = await client.createSession({
      name: 'structured-demo',
      workspace: '/tmp/structured-test',
      systemPrompt: 'You are a helpful assistant that returns structured data.',
    });
    const sessionId = session.sessionId;
    console.log(`Session: ${sessionId}\n`);

    // Unary structured generation
    console.log('1. generateStructured() — Extract person info...');
    const personSchema = JSON.stringify({
      type: 'object',
      properties: {
        name: { type: 'string', description: 'Full name' },
        age: { type: 'integer', description: 'Age in years' },
        email: { type: 'string', format: 'email' },
        skills: { type: 'array', items: { type: 'string' } },
      },
      required: ['name', 'age', 'email', 'skills'],
    });

    const personResponse = await client.generateStructured(
      sessionId,
      [{ role: 'user', content: "Extract the person's info: John Smith is a 32-year-old developer at john@example.com who knows Python, Rust, and TypeScript." }],
      personSchema,
    );

    if (personResponse.data) {
      const data = JSON.parse(personResponse.data);
      console.log(`✓ Name: ${data.name}, Age: ${data.age}`);
      console.log(`  Email: ${data.email}`);
      console.log(`  Skills: ${data.skills?.join(', ')}`);
    }
    console.log();

    // Streaming structured generation
    console.log('2. streamGenerateStructured() — Code review...');
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
      [{ role: 'user', content: 'Review this code:\n```python\ndef divide(a, b):\n    return a / b\n\nresult = divide(10, 0)\nprint(result)\n```' }],
      reviewSchema,
    )) {
      if (chunk.data) {
        finalData = chunk.data;
        process.stdout.write('.');
      }
      if (chunk.done) console.log(' done!');
    }

    if (finalData) {
      const data = JSON.parse(finalData);
      console.log(`✓ Summary: ${data.summary}`);
      console.log(`  Score: ${data.score}/100`);
      for (const issue of data.issues || []) {
        console.log(`  [${issue.severity.toUpperCase()}] ${issue.description}`);
      }
    }
    console.log();

    await client.destroySession(sessionId);

  } finally {
    client.close();
  }

  // ========================================================================
  // Part 2: High-Level Structured Generation
  // ========================================================================
  console.log('=== Part 2: High-Level API ===\n');

  const openai = createProvider({
    name: 'openai',
    apiKey: process.env.OPENAI_API_KEY || 'sk-xxx',
  });

  // generateObject — auto session
  console.log('3. generateObject() — Task list...');
  const { object: taskList } = await generateObject({
    model: openai('gpt-4o'),
    schema: JSON.stringify({
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
    }),
    prompt: 'Create a task list for building a REST API with authentication.',
  });

  console.log('✓ Task list:');
  for (const task of (taskList as any).tasks || []) {
    console.log(`  [${task.priority.toUpperCase()}] ${task.title} (${task.estimated_hours}h)`);
  }
  console.log(`  Total: ${(taskList as any).total_hours}h`);
  console.log();

  // streamObject — auto session
  console.log('4. streamObject() — Project analysis...');
  const { partialStream, object: finalObject } = streamObject({
    model: openai('gpt-4o'),
    schema: JSON.stringify({
      type: 'object',
      properties: {
        summary: { type: 'string' },
        files: { type: 'array', items: { type: 'string' } },
        complexity: { type: 'string', enum: ['low', 'medium', 'high'] },
      },
      required: ['summary', 'files', 'complexity'],
    }),
    prompt: 'Analyze a typical Express.js REST API project structure.',
  });

  process.stdout.write('   Streaming: ');
  for await (const _partial of partialStream) {
    process.stdout.write('.');
  }
  console.log(' done!');

  const analysis = (await finalObject) as any;
  console.log(`✓ Summary: ${analysis.summary}`);
  console.log(`  Complexity: ${analysis.complexity}`);
  console.log(`  Files: ${analysis.files?.join(', ')}`);
  console.log();

  console.log('='.repeat(60));
  console.log('Structured generation complete! ✓');
  console.log('='.repeat(60));
}

main().catch(console.error);
