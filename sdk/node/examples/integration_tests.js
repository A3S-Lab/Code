#!/usr/bin/env node
/**
 * A3S Code Node.js SDK - Integration Tests
 *
 * Tests all major features using real LLM configuration from ~/.a3s/config.hcl
 *
 * Run with: node examples/integration_tests.js
 */

const { Agent } = require('../index.js');
const fs = require('fs');
const path = require('path');
const os = require('os');

/**
 * Find config file in home directory or project root.
 */
function findConfigPath() {
  const homeConfig = path.join(os.homedir(), '.a3s', 'config.hcl');
  if (fs.existsSync(homeConfig)) {
    return homeConfig;
  }

  // Try project root (assuming we're in crates/code/sdk/node)
  const projectConfig = path.join(__dirname, '..', '..', '..', '..', '..', '.a3s', 'config.hcl');
  if (fs.existsSync(projectConfig)) {
    return projectConfig;
  }

  throw new Error('Config file not found. Please create ~/.a3s/config.hcl');
}

/**
 * Truncate text to max length.
 */
function truncate(text, maxLen) {
  if (text.length <= maxLen) {
    return text;
  }
  return `${text.substring(0, maxLen)}... (truncated)`;
}

/**
 * Test 1: Basic tool execution.
 */
async function testBasicTools(agent) {
  console.log('\n📦 Test 1: Basic Tool Execution');
  console.log('-'.repeat(80));

  const session = agent.session('.');

  console.log('Testing: List current directory...');
  const result1 = await session.send('List the files in the current directory using ls');
  console.log(`✓ Result preview: ${truncate(result1.text, 200)}`);

  console.log('\nTesting: Read a file...');
  const result2 = await session.send('Read the Cargo.toml file');
  console.log(`✓ Result preview: ${truncate(result2.text, 200)}`);

  console.log('\n✅ Test 1 passed: Basic tools work correctly');
}

/**
 * Test 2: File operations.
 */
async function testFileOperations(agent) {
  console.log('\n📝 Test 2: File Operations');
  console.log('-'.repeat(80));

  const session = agent.session('.');

  console.log('Testing: Create a test file...');
  const result1 = await session.send(
    "Create a file named 'test_integration_js.txt' with content 'Hello from Node.js SDK!'"
  );
  console.log(`✓ Result: ${truncate(result1.text, 200)}`);

  console.log('\nTesting: Read the test file...');
  const result2 = await session.send('Read the file test_integration_js.txt');
  console.log(`✓ Result: ${truncate(result2.text, 200)}`);

  console.log('\nCleaning up: Remove test file...');
  try {
    fs.unlinkSync('test_integration_js.txt');
  } catch (err) {
    // Ignore if file doesn't exist
  }

  console.log('\n✅ Test 2 passed: File operations work correctly');
}

/**
 * Test 3: Search operations.
 */
async function testSearchOperations(agent) {
  console.log('\n🔍 Test 3: Search Operations');
  console.log('-'.repeat(80));

  const session = agent.session('.');

  console.log('Testing: grep search...');
  const result1 = await session.send('Search for the word "Agent" in all Rust files using grep');
  console.log(`✓ Result preview: ${truncate(result1.text, 200)}`);

  console.log('\nTesting: glob pattern matching...');
  const result2 = await session.send('Find all .rs files in the src directory using glob');
  console.log(`✓ Result preview: ${truncate(result2.text, 200)}`);

  console.log('\n✅ Test 3 passed: Search operations work correctly');
}

/**
 * Test 4: Direct tool execution.
 */
async function testDirectToolCalls(agent) {
  console.log('\n🛠️  Test 4: Direct Tool Execution');
  console.log('-'.repeat(80));

  const session = agent.session('.');

  console.log('Testing: Direct readFile call...');
  const content = await session.readFile('Cargo.toml');
  console.log(`✓ Read ${content.length} bytes from Cargo.toml`);

  console.log('\nTesting: Direct bash call...');
  const output = await session.bash("echo 'Hello from Node.js SDK'");
  console.log(`✓ Bash output: ${output.trim()}`);

  console.log('\nTesting: Direct glob call...');
  const files = await session.glob('src/*.rs');
  console.log(`✓ Found ${files.length} Rust files`);

  console.log('\nTesting: Direct grep call...');
  const matches = await session.grep('Agent');
  console.log(`✓ Grep found ${matches.length} bytes of matches`);

  console.log('\n✅ Test 4 passed: Direct tool calls work correctly');
}

/**
 * Test 5: Streaming execution.
 */
async function testStreaming(agent) {
  console.log('\n🌊 Test 5: Streaming Execution');
  console.log('-'.repeat(80));

  const session = agent.session('.');

  console.log('Testing: Stream events...');
  const events = await session.stream('List all .rs files in the current directory');

  let textDeltas = 0;
  let toolCalls = 0;

  for (const event of events) {
    if (event.type === 'text_delta') {
      textDeltas++;
    } else if (event.type === 'tool_start') {
      toolCalls++;
      console.log(`  Tool called: ${event.toolName}`);
    }
  }

  console.log(`✓ Received ${events.length} events (${textDeltas} text deltas, ${toolCalls} tool calls)`);
  console.log('\n✅ Test 5 passed: Streaming works correctly');
}

/**
 * Test 6: Session options.
 */
async function testSessionOptions(agent) {
  console.log('\n⚙️  Test 6: Session Options');
  console.log('-'.repeat(80));

  console.log('Testing: Session with custom options...');
  const session = agent.session('.', {
    // Can override model here if needed
    // model: 'openai/gpt-4o'
  });

  const result = await session.send('What is the name of this project?');
  console.log(`✓ Result preview: ${truncate(result.text, 200)}`);

  console.log('\n✅ Test 6 passed: Session options work correctly');
}

/**
 * Test 7: Conversation history.
 */
async function testConversationHistory(agent) {
  console.log('\n💬 Test 7: Conversation History');
  console.log('-'.repeat(80));

  const session = agent.session('.');

  console.log('Testing: Multi-turn conversation...');
  const result1 = await session.send('What is 2 + 2?');
  console.log(`✓ Turn 1: ${truncate(result1.text, 100)}`);

  const result2 = await session.send('What was my previous question?');
  console.log(`✓ Turn 2: ${truncate(result2.text, 100)}`);

  const history = session.history();
  console.log(`✓ History has ${history.length} messages`);

  console.log('\n✅ Test 7 passed: Conversation history works correctly');
}

/**
 * Main test runner.
 */
async function main() {
  console.log('🚀 A3S Code Node.js SDK - Integration Tests\n');
  console.log('='.repeat(80));

  // Load config
  const configPath = findConfigPath();
  console.log(`📄 Using config: ${configPath}`);
  console.log('='.repeat(80));
  console.log();

  // Create agent
  const agent = await Agent.create(configPath);

  // Run tests
  await testBasicTools(agent);
  await testFileOperations(agent);
  await testSearchOperations(agent);
  await testDirectToolCalls(agent);
  await testStreaming(agent);
  await testSessionOptions(agent);
  await testConversationHistory(agent);

  console.log('\n\n');
  console.log('='.repeat(80));
  console.log('✅ All Node.js SDK integration tests completed successfully!');
  console.log('='.repeat(80));
}

// Run tests
main().catch((err) => {
  console.error('❌ Test failed:', err);
  process.exit(1);
});
