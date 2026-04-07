// Test that agentic_search tool is available in Node.js SDK.

const { Agent } = require('../index.js');
const path = require('path');
const fs = require('fs');
const os = require('os');

async function testAgenticSearchAvailable() {
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'agentic-search-test-'));

  try {
    fs.writeFileSync(path.join(tmpDir, 'auth.ts'), `
export function authenticate(token: string) {
  // JWT token validation
  return validateJWT(token);
}
`);

    const configPath = path.join(tmpDir, 'agent.hcl');
    fs.writeFileSync(configPath, `
default_model = "anthropic/claude-sonnet-4-20250514"
providers {
  name    = "anthropic"
  api_key = env("ANTHROPIC_API_KEY")
}
`);

    const agent = await Agent.create(configPath);

    // Test 1: default session
    const session1 = agent.session(tmpDir);
    const tools1 = session1.toolNames();
    if (!tools1.includes('agentic_search')) {
      throw new Error('agentic_search not found in default session');
    }
    console.log('✅ agentic_search available in default session');

  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
}

testAgenticSearchAvailable()
  .then(() => { console.log('\n✅ All tests passed'); process.exit(0); })
  .catch((err) => { console.error('\n❌ Test failed:', err.message); process.exit(1); });
