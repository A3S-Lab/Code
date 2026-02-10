/**
 * Interactive Code Generation Demo
 *
 * Demonstrates interactive code generation with:
 * - File operations (read/write)
 * - Code execution
 * - Iterative refinement
 * - Error handling and debugging
 */

import { A3sClient, StorageType } from '@a3s-lab/code';
import * as fs from 'fs';
import * as path from 'path';

async function main() {
  const client = new A3sClient({ address: 'localhost:4088' });
  const workspaceDir = '/tmp/code-gen-workspace';

  // Ensure workspace exists
  if (!fs.existsSync(workspaceDir)) {
    fs.mkdirSync(workspaceDir, { recursive: true });
  }

  try {
    console.log('🚀 Interactive Code Generation Demo\n');

    // Create session
    console.log('Creating session...');
    const session = await client.createSession({
      name: 'code-generator',
      workspace: workspaceDir,
      systemPrompt: `You are an expert code generator. You can:
- Write clean, well-documented code
- Create files and organize project structure
- Execute code to verify it works
- Debug and fix issues
- Follow best practices and design patterns

Always provide working, production-ready code.`,
      storageType: StorageType.STORAGE_TYPE_FILE,
    });

    const sessionId = session.sessionId;
    console.log(`✓ Session created: ${sessionId}\n`);

    // ========================================================================
    // Scenario 1: Generate a complete module with tests
    // ========================================================================
    console.log('📝 Scenario 1: Generate a complete module\n');
    console.log('User: Create a TypeScript module for a simple calculator with add, subtract, multiply, and divide functions. Include unit tests.\n');

    console.log('Assistant: ');
    let codeGenerated = false;
    let testFileCreated = false;

    for await (const chunk of client.streamGenerate(sessionId, [
      {
        role: 'user',
        content: `Create a TypeScript module for a simple calculator with add, subtract, multiply, and divide functions.

Requirements:
1. Create calculator.ts with the implementation
2. Create calculator.test.ts with comprehensive unit tests
3. Use proper TypeScript types
4. Handle edge cases (division by zero, etc.)
5. Export all functions

Please create the files in the workspace.`
      }
    ])) {
      if (chunk.type === 'content') {
        process.stdout.write(chunk.content);
      } else if (chunk.type === 'tool_call' && chunk.toolCall) {
        console.log(`\n\n🔧 Tool: ${chunk.toolCall.name}`);
        if (chunk.toolCall.name === 'Write' || chunk.toolCall.name === 'write') {
          codeGenerated = true;
          const args = JSON.parse(chunk.toolCall.arguments);
          if (args.file_path?.includes('test')) {
            testFileCreated = true;
          }
        }
      } else if (chunk.type === 'tool_result' && chunk.toolResult) {
        if (chunk.toolResult.success) {
          console.log('   ✓ Success');
        } else if (chunk.toolResult.error) {
          console.log(`   ✗ Error: ${chunk.toolResult.error}`);
        }
      }
    }
    console.log('\n');

    // Verify files were created
    if (codeGenerated) {
      console.log('✓ Code files generated\n');
    }

    // ========================================================================
    // Scenario 2: Read and improve existing code
    // ========================================================================
    console.log('📝 Scenario 2: Code review and improvement\n');

    // Create a sample file with issues
    const sampleCode = `
export function processData(data) {
  var result = [];
  for (var i = 0; i < data.length; i++) {
    if (data[i] != null) {
      result.push(data[i] * 2);
    }
  }
  return result;
}
`;

    const sampleFilePath = path.join(workspaceDir, 'sample.ts');
    fs.writeFileSync(sampleFilePath, sampleCode);
    console.log('Created sample.ts with code that needs improvement\n');

    console.log('User: Review sample.ts and suggest improvements\n');

    console.log('Assistant: ');
    for await (const chunk of client.streamGenerate(sessionId, [
      {
        role: 'user',
        content: `Please read sample.ts, review the code, and suggest improvements. Consider:
- TypeScript best practices
- Modern JavaScript features
- Type safety
- Code clarity

Then create an improved version as sample-improved.ts`
      }
    ])) {
      if (chunk.type === 'content') {
        process.stdout.write(chunk.content);
      } else if (chunk.type === 'tool_call' && chunk.toolCall) {
        console.log(`\n\n🔧 Tool: ${chunk.toolCall.name}`);
      } else if (chunk.type === 'tool_result' && chunk.toolResult) {
        if (chunk.toolResult.success) {
          console.log('   ✓ Success');
        } else if (chunk.toolResult.error) {
          console.log(`   ✗ Error: ${chunk.toolResult.error}`);
        }
      }
    }
    console.log('\n');

    // ========================================================================
    // Scenario 3: Iterative refinement
    // ========================================================================
    console.log('📝 Scenario 3: Iterative refinement\n');

    console.log('User: Add error handling and logging to the calculator module\n');

    console.log('Assistant: ');
    for await (const chunk of client.streamGenerate(sessionId, [
      {
        role: 'user',
        content: `Update calculator.ts to add:
1. Proper error handling with custom error types
2. Input validation
3. Logging for operations
4. JSDoc comments

Keep the existing functionality but make it production-ready.`
      }
    ])) {
      if (chunk.type === 'content') {
        process.stdout.write(chunk.content);
      } else if (chunk.type === 'tool_call' && chunk.toolCall) {
        console.log(`\n\n🔧 Tool: ${chunk.toolCall.name}`);
      }
    }
    console.log('\n');

    // ========================================================================
    // Scenario 4: Generate project structure
    // ========================================================================
    console.log('📝 Scenario 4: Generate project structure\n');

    console.log('User: Create a complete project structure for a REST API\n');

    console.log('Assistant: ');
    for await (const chunk of client.streamGenerate(sessionId, [
      {
        role: 'user',
        content: `Create a basic REST API project structure with:
- src/index.ts (entry point)
- src/routes/users.ts (user routes)
- src/controllers/userController.ts (user controller)
- src/models/user.ts (user model)
- src/middleware/auth.ts (auth middleware)
- package.json (with dependencies)
- tsconfig.json (TypeScript config)
- .env.example (environment variables)

Use Express.js and TypeScript. Create all files with basic implementations.`
      }
    ])) {
      if (chunk.type === 'content') {
        process.stdout.write(chunk.content);
      } else if (chunk.type === 'tool_call' && chunk.toolCall) {
        console.log(`\n\n🔧 Tool: ${chunk.toolCall.name}`);
      } else if (chunk.type === 'tool_result' && chunk.toolResult) {
        if (chunk.toolResult.success) {
          console.log('   ✓ Created');
        } else if (chunk.toolResult.error) {
          console.log(`   ✗ Error: ${chunk.toolResult.error}`);
        }
      }
    }
    console.log('\n');

    // ========================================================================
    // Scenario 5: Debug and fix code
    // ========================================================================
    console.log('📝 Scenario 5: Debug and fix code\n');

    // Create buggy code
    const buggyCode = `
export async function fetchUserData(userId: string) {
  const response = await fetch(\`https://api.example.com/users/\${userId}\`);
  const data = response.json();
  return data.user.name;
}
`;

    const buggyFilePath = path.join(workspaceDir, 'buggy.ts');
    fs.writeFileSync(buggyFilePath, buggyCode);
    console.log('Created buggy.ts with issues\n');

    console.log('User: Find and fix all bugs in buggy.ts\n');

    console.log('Assistant: ');
    for await (const chunk of client.streamGenerate(sessionId, [
      {
        role: 'user',
        content: `Read buggy.ts and identify all bugs and issues. Then create a fixed version as buggy-fixed.ts with:
1. All bugs fixed
2. Proper error handling
3. Type safety
4. Best practices applied

Explain what was wrong and how you fixed it.`
      }
    ])) {
      if (chunk.type === 'content') {
        process.stdout.write(chunk.content);
      } else if (chunk.type === 'tool_call' && chunk.toolCall) {
        console.log(`\n\n🔧 Tool: ${chunk.toolCall.name}`);
      }
    }
    console.log('\n');

    // ========================================================================
    // Summary
    // ========================================================================
    console.log('📊 Summary\n');

    // List generated files
    console.log('Generated files:');
    const files = fs.readdirSync(workspaceDir);
    files.forEach(file => {
      const filePath = path.join(workspaceDir, file);
      const stats = fs.statSync(filePath);
      if (stats.isFile()) {
        console.log(`   - ${file} (${stats.size} bytes)`);
      }
    });
    console.log();

    // Context usage
    const usage = await client.getContextUsage(sessionId);
    if (usage.usage) {
      console.log('Context usage:');
      console.log(`   Total tokens: ${usage.usage.totalTokens}`);
      console.log(`   Messages: ${usage.usage.messageCount}`);
    }
    console.log();

    console.log(`✓ All scenarios completed!`);
    console.log(`✓ Workspace: ${workspaceDir}`);
    console.log(`✓ Session: ${sessionId}`);

  } catch (error) {
    console.error('Error:', error);
  } finally {
    client.close();
  }
}

main();
