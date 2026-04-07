/**
 * Simple test to verify btw feature works in the examples directory.
 */

import { Agent } from '@a3s-lab/code';
import * as path from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

async function main() {
  console.log('Testing btw feature...\n');

  // Check if env vars are set
  if (!process.env.KIMI_API_KEY || !process.env.KIMI_BASE_URL) {
    console.error('Please set KIMI_API_KEY and KIMI_BASE_URL environment variables');
    process.exit(1);
  }

  console.log('Environment variables found');
  console.log(`API Key: ${process.env.KIMI_API_KEY.slice(0, 10)}...`);
  console.log(`Base URL: ${process.env.KIMI_BASE_URL}\n`);

  // Create agent
  const configPath = path.join(__dirname, 'agent_btw_test.hcl');
  console.log(`Loading config from: ${configPath}`);
  const agent = await Agent.create(configPath);
  console.log('Agent created successfully\n');

  // Create session
  const session = agent.session('.');
  console.log('Session created\n');

  // Test btw
  console.log('Testing btw with simple question...');
  const result = await session.btw('What is 2+2?');
  console.log(`Question: ${result.question}`);
  console.log(`Answer: ${result.answer}`);
  console.log(`Tokens: ${result.totalTokens}\n`);

  console.log('✅ BTW feature works!');
}

main().catch(err => {
  console.error('Error:', err.message);
  process.exit(1);
});
