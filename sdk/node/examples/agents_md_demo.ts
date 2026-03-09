/**
 * Example: AGENTS.md Auto-Loading
 *
 * Demonstrates automatic loading of AGENTS.md from workspace root.
 * Similar to Claude Code's CLAUDE.md mechanism.
 */

import { Agent } from '..';
import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';

async function main() {
  console.log('🚀 AGENTS.md Auto-Loading Example\n');

  // Create a temporary workspace with AGENTS.md
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'agents-md-test-'));
  console.log(`📁 Workspace: ${tmpDir}\n`);

  // Write AGENTS.md with project-specific instructions
  const agentsMdContent = `# Project Instructions

This is a TypeScript project using Node.js and Express.

## Code Style
- Use TypeScript strict mode
- Prefer async/await over callbacks
- Use ESLint and Prettier
- Write tests with Jest

## Architecture
- Follow MVC pattern
- Use dependency injection
- Keep controllers thin
- Business logic in services

## Testing
- Unit tests for all services
- Integration tests for API endpoints
- Minimum 80% code coverage

## Security
- Validate all user input
- Use parameterized queries
- Never log sensitive data
- Follow OWASP Top 10 guidelines
`;

  fs.writeFileSync(path.join(tmpDir, 'AGENTS.md'), agentsMdContent);
  console.log('✅ Created AGENTS.md in workspace\n');

  // Create agent and session
  const agent = await Agent.create('agent.hcl');
  const session = agent.session(tmpDir, {
    builtinSkills: true,
  });

  console.log('📝 Sending prompt to agent...\n');

  // Send a prompt - the agent should follow AGENTS.md instructions
  const result = await session.send(
    'Create a new user registration endpoint with validation and tests'
  );

  console.log('✅ Agent response:\n');
  console.log(result.text);
  console.log(`\n📊 Stats: ${result.toolCallsCount} tools, ${result.totalTokens} tokens`);

  // Cleanup
  fs.rmSync(tmpDir, { recursive: true, force: true });
  console.log('\n🧹 Cleaned up temporary workspace');
}

main().catch(console.error);
