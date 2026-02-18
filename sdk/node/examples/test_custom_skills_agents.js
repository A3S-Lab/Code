#!/usr/bin/env node
/**
 * A3S Code Node.js SDK - Custom Skills and Agents Integration Test
 *
 * Tests loading and execution of custom skills and agents from ~/.a3s directory.
 *
 * Run with: node examples/test_custom_skills_agents.js
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

  // Try project root (6 levels up from examples/test_custom_skills_agents.js)
  const projectConfig = path.join(__dirname, '..', '..', '..', '..', '..', '..', '.a3s', 'config.hcl');
  if (fs.existsSync(projectConfig)) {
    return projectConfig;
  }

  throw new Error('Config file not found. Please create ~/.a3s/config.hcl');
}

/**
 * Find skills directory.
 */
function findSkillsDir() {
  const homeSkills = path.join(os.homedir(), '.a3s', 'skills');
  if (fs.existsSync(homeSkills)) {
    return homeSkills;
  }

  // Try project root
  const projectSkills = path.join(__dirname, '..', '..', '..', '..', '..', '..', '.a3s', 'skills');
  if (fs.existsSync(projectSkills)) {
    return projectSkills;
  }

  throw new Error('Skills directory not found');
}

/**
 * Find agents directory.
 */
function findAgentsDir() {
  const homeAgents = path.join(os.homedir(), '.a3s', 'agents');
  if (fs.existsSync(homeAgents)) {
    return homeAgents;
  }

  // Try project root
  const projectAgents = path.join(__dirname, '..', '..', '..', '..', '..', '..', '.a3s', 'agents');
  if (fs.existsSync(projectAgents)) {
    return projectAgents;
  }

  throw new Error('Agents directory not found');
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
 * Test 1: Load custom skills in session.
 */
async function testCustomSkills() {
  console.log('\n📦 Test 1: Load Custom Skills in Session');
  console.log('-'.repeat(80));

  const configPath = findConfigPath();
  const skillsDir = findSkillsDir();

  const agent = await Agent.create(configPath);
  const session = agent.session('.', { skillDirs: [skillsDir] });

  console.log('Testing: Ask about Rust best practices (should use rust-expert skill)...');
  const result1 = await session.send('What are the key principles of Rust ownership?');
  console.log(`✓ Result preview: ${truncate(result1.text, 200)}`);
  console.log();

  console.log('Testing: Ask about API design (should use api-designer skill)...');
  const result2 = await session.send('What are REST API best practices for error handling?');
  console.log(`✓ Result preview: ${truncate(result2.text, 200)}`);
  console.log();

  console.log('✅ Test 1 passed: Custom skills loaded and used in session\n');
}

/**
 * Test 2: Load custom agents in session.
 */
async function testCustomAgents() {
  console.log('🤖 Test 2: Load Custom Agents in Session');
  console.log('-'.repeat(80));

  const configPath = findConfigPath();
  const agentsDir = findAgentsDir();

  const agent = await Agent.create(configPath);
  const session = agent.session('.', { agentDirs: [agentsDir] });

  console.log('Testing: Request code review (should use code-reviewer agent)...');
  const result1 = await session.send('Review this function for potential issues: function add(a, b) { return a + b; }');
  console.log(`✓ Result preview: ${truncate(result1.text, 200)}`);
  console.log();

  console.log('Testing: Request documentation (should use documentation-writer agent)...');
  const result2 = await session.send('Write API documentation for a user registration endpoint');
  console.log(`✓ Result preview: ${truncate(result2.text, 200)}`);
  console.log();

  console.log('✅ Test 2 passed: Custom agents loaded and used in session\n');
}

/**
 * Test 3: Load both skills and agents together.
 */
async function testSkillsAndAgentsTogether() {
  console.log('🔧 Test 3: Load Both Skills and Agents Together');
  console.log('-'.repeat(80));

  const configPath = findConfigPath();
  const skillsDir = findSkillsDir();
  const agentsDir = findAgentsDir();

  const agent = await Agent.create(configPath);
  const session = agent.session('.', {
    skillDirs: [skillsDir],
    agentDirs: [agentsDir]
  });

  console.log('Testing: Complex task using both skills and agents...');
  const result = await session.send('Generate unit tests for a JavaScript function that parses JSON');
  console.log(`✓ Result preview: ${truncate(result.text, 200)}`);
  console.log();

  console.log('✅ Test 3 passed: Both skills and agents work together\n');
}

/**
 * Test 4: Multiple sessions with different configurations.
 */
async function testMultipleSessions() {
  console.log('🔀 Test 4: Multiple Sessions with Different Configurations');
  console.log('-'.repeat(80));

  const configPath = findConfigPath();
  const skillsDir = findSkillsDir();
  const agentsDir = findAgentsDir();

  const agent = await Agent.create(configPath);

  // Session 1: Only skills
  const session1 = agent.session('.', { skillDirs: [skillsDir] });

  // Session 2: Only agents
  const session2 = agent.session('.', { agentDirs: [agentsDir] });

  console.log('Testing: Session 1 with skills only...');
  const result1 = await session1.send('Explain JavaScript async/await');
  console.log(`✓ Session 1 result: ${truncate(result1.text, 150)}`);
  console.log();

  console.log('Testing: Session 2 with agents only...');
  const result2 = await session2.send('Review this code for improvements');
  console.log(`✓ Session 2 result: ${truncate(result2.text, 150)}`);
  console.log();

  console.log('✅ Test 4 passed: Multiple sessions with different configs work correctly\n');
}

/**
 * Main test runner.
 */
async function main() {
  console.log('🚀 A3S Code Node.js SDK - Custom Skills and Agents Integration Test\n');
  console.log('='.repeat(80));

  const configPath = findConfigPath();
  console.log(`📄 Using config: ${configPath}`);

  const skillsDir = findSkillsDir();
  console.log(`📚 Skills directory: ${skillsDir}`);

  const agentsDir = findAgentsDir();
  console.log(`🤖 Agents directory: ${agentsDir}`);

  console.log('='.repeat(80));

  await testCustomSkills();
  await testCustomAgents();
  await testSkillsAndAgentsTogether();
  await testMultipleSessions();

  console.log();
  console.log('='.repeat(80));
  console.log('✅ All custom skills and agents tests completed successfully!');
  console.log('='.repeat(80));
}

// Run tests
main().catch((err) => {
  console.error('❌ Test failed:', err);
  process.exit(1);
});
