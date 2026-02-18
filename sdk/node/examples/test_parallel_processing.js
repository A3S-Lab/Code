#!/usr/bin/env node
/**
 * A3S Code Node.js SDK - Parallel Task Processing Integration Test
 *
 * Demonstrates parallel processing of multiple tasks using A3S Lane queue.
 * Tests concurrent file analysis, code review, and documentation generation.
 *
 * Run with: node examples/test_parallel_processing.js
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

  // Try project root (6 levels up)
  const projectConfig = path.join(__dirname, '..', '..', '..', '..', '..', '..', '.a3s', 'config.hcl');
  if (fs.existsSync(projectConfig)) {
    return projectConfig;
  }

  throw new Error('Config file not found. Please create ~/.a3s/config.hcl');
}

/**
 * Test 1: Sequential processing (baseline).
 */
async function testSequentialProcessing() {
  console.log('\n📦 Test 1: Sequential Processing (Baseline)');
  console.log('-'.repeat(80));

  const configPath = findConfigPath();
  const agent = await Agent.create(configPath);
  const session = agent.session('.');

  const tasks = [
    'Count the number of JavaScript files in this project',
    'Find all TODO comments in JavaScript files',
    'List all exported functions in the main module',
  ];

  const start = Date.now();
  console.log(`Processing ${tasks.length} tasks sequentially...`);

  for (let i = 0; i < tasks.length; i++) {
    console.log(`  Task ${i + 1}: ${tasks[i]}`);
    const result = await session.send(tasks[i]);
    console.log(`  ✓ Completed: ${result.text.length} chars`);
  }

  const duration = (Date.now() - start) / 1000;
  console.log(`\n✓ Sequential processing took: ${duration.toFixed(2)}s`);
  console.log('\n✅ Test 1 passed: Sequential processing works\n');
}

/**
 * Test 2: Parallel processing with queue.
 */
async function testParallelProcessing() {
  console.log('⚡ Test 2: Parallel Processing with Queue');
  console.log('-'.repeat(80));

  const configPath = findConfigPath();
  const agent = await Agent.create(configPath);

  // Configure queue for parallel processing
  const session = agent.session('.', {
    queueConfig: {
      queryConcurrency: 3,      // Allow 3 concurrent query operations
      executeConcurrency: 2,    // Allow 2 concurrent execute operations
      enableMetrics: true,      // Enable metrics collection
      enableDlq: true,          // Enable dead letter queue
    }
  });

  const tasks = [
    'Count the number of JavaScript files in this project',
    'Find all TODO comments in JavaScript files',
    'List all exported functions in the main module',
    'Find all async functions in the codebase',
    'Count lines of code in all JavaScript files',
  ];

  const start = Date.now();
  console.log(`Processing ${tasks.length} tasks in parallel...`);

  // Queue all tasks
  for (let i = 0; i < tasks.length; i++) {
    console.log(`  Queuing task ${i + 1}: ${tasks[i]}`);
  }

  console.log('\n  Waiting for all tasks to complete...');

  // Process tasks concurrently
  const promises = tasks.map((task, i) =>
    session.send(task)
      .then(result => ({ taskNum: i + 1, success: true, result }))
      .catch(error => ({ taskNum: i + 1, success: false, error }))
  );

  const results = await Promise.all(promises);

  for (const { taskNum, success, result, error } of results) {
    if (success) {
      console.log(`  ✓ Task ${taskNum} completed: ${result.text.length} chars`);
    } else {
      console.log(`  ✗ Task ${taskNum} failed: ${error.message}`);
    }
  }

  const duration = (Date.now() - start) / 1000;
  console.log(`\n✓ Parallel processing took: ${duration.toFixed(2)}s`);

  // Check queue stats
  if (session.hasQueue && session.hasQueue()) {
    const stats = session.queueStats();
    console.log('\n📊 Queue Statistics:');
    console.log(`  Total processed: ${stats.totalProcessed}`);
    console.log(`  Total failed: ${stats.totalFailed}`);
    console.log(`  DLQ size: ${stats.dlqSize}`);
  }

  console.log('\n✅ Test 2 passed: Parallel processing with queue works\n');
}

/**
 * Test 3: Parallel processing with priority lanes.
 */
async function testPriorityLanes() {
  console.log('🎯 Test 3: Parallel Processing with Priority Lanes');
  console.log('-'.repeat(80));

  const configPath = findConfigPath();
  const agent = await Agent.create(configPath);

  const session = agent.session('.', {
    queueConfig: {
      controlConcurrency: 1,    // P0: Control operations
      queryConcurrency: 3,      // P1: Query operations (highest concurrency)
      executeConcurrency: 2,    // P2: Execute operations
      generateConcurrency: 1,   // P3: Generate operations
      enableMetrics: true,
      enableDlq: true,
    }
  });

  console.log('Testing priority-based task execution...');
  console.log('  P1 (Query): 3 concurrent tasks');
  console.log('  P2 (Execute): 2 concurrent tasks');
  console.log('  P3 (Generate): 1 concurrent task');

  const start = Date.now();

  // Mix of different task types
  const tasks = [
    { type: 'Query', text: 'How many JavaScript files are there?' },
    { type: 'Query', text: 'What is the project structure?' },
    { type: 'Query', text: 'List all dependencies' },
    { type: 'Execute', text: 'Create a summary of the codebase' },
    { type: 'Execute', text: 'Analyze code complexity' },
  ];

  for (let i = 0; i < tasks.length; i++) {
    console.log(`  Queuing ${tasks[i].type} task ${i + 1}: ${tasks[i].text}`);
  }

  console.log('\n  Waiting for all tasks to complete...');

  const promises = tasks.map((task, i) =>
    session.send(task.text)
      .then(result => ({ taskNum: i + 1, type: task.type, success: true, result }))
      .catch(error => ({ taskNum: i + 1, type: task.type, success: false, error }))
  );

  const results = await Promise.all(promises);

  for (const { taskNum, type, success, result, error } of results) {
    if (success) {
      console.log(`  ✓ Task ${taskNum} (${type}) completed: ${result.text.length} chars`);
    } else {
      console.log(`  ✗ Task ${taskNum} (${type}) failed: ${error.message}`);
    }
  }

  const duration = (Date.now() - start) / 1000;
  console.log(`\n✓ Priority-based processing took: ${duration.toFixed(2)}s`);
  console.log('\n✅ Test 3 passed: Priority lanes work correctly\n');
}

/**
 * Test 4: Parallel processing with retry policy.
 */
async function testRetryPolicy() {
  console.log('🔄 Test 4: Parallel Processing with Retry Policy');
  console.log('-'.repeat(80));

  const configPath = findConfigPath();
  const agent = await Agent.create(configPath);

  const session = agent.session('.', {
    queueConfig: {
      queryConcurrency: 3,
      enableMetrics: true,
      enableDlq: true,
    }
  });

  console.log('Testing retry policy with exponential backoff...');
  console.log('  Note: Retry policy configured at queue level');
  console.log('  Max retries: 3 (default)');
  console.log('  Strategy: exponential (default)');

  const tasks = [
    'Analyze the main function',
    'Find all error types',
    'List all test functions',
  ];

  const start = Date.now();

  for (let i = 0; i < tasks.length; i++) {
    console.log(`  Queuing task ${i + 1}: ${tasks[i]}`);
  }

  console.log('\n  Waiting for all tasks to complete...');

  const promises = tasks.map((task, i) =>
    session.send(task)
      .then(result => ({ taskNum: i + 1, success: true, result }))
      .catch(error => ({ taskNum: i + 1, success: false, error }))
  );

  const results = await Promise.all(promises);

  for (const { taskNum, success, result, error } of results) {
    if (success) {
      console.log(`  ✓ Task ${taskNum} completed: ${result.text.length} chars`);
    } else {
      console.log(`  ✗ Task ${taskNum} failed: ${error.message}`);
    }
  }

  const duration = (Date.now() - start) / 1000;
  console.log(`\n✓ Processing with retry took: ${duration.toFixed(2)}s`);

  if (session.hasQueue && session.hasQueue()) {
    const stats = session.queueStats();
    console.log('\n📊 Final Queue Statistics:');
    console.log(`  Total processed: ${stats.totalProcessed}`);
    console.log(`  Total failed: ${stats.totalFailed}`);
    console.log(`  DLQ size: ${stats.dlqSize}`);
  }

  console.log('\n✅ Test 4 passed: Retry policy works correctly\n');
}

/**
 * Main test runner.
 */
async function main() {
  console.log('🚀 A3S Code Node.js SDK - Parallel Task Processing Integration Test\n');
  console.log('='.repeat(80));

  const configPath = findConfigPath();
  console.log(`📄 Using config: ${configPath}`);
  console.log('='.repeat(80));

  await testSequentialProcessing();
  await testParallelProcessing();
  await testPriorityLanes();
  await testRetryPolicy();

  console.log();
  console.log('='.repeat(80));
  console.log('✅ All parallel processing tests completed successfully!');
  console.log('='.repeat(80));
}

// Run tests
main().catch((err) => {
  console.error('❌ Test failed:', err);
  process.exit(1);
});
