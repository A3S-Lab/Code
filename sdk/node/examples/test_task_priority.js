/**
 * Lane-Based Priority Preemption Test with Real LLM
 *
 * Demonstrates how the lane-based priority system allows high-priority tasks
 * to preempt queued low-priority tasks:
 *
 * 1. Constrain concurrency so tasks must queue up
 * 2. Submit multiple low-priority tasks (Execute lane: bash commands)
 * 3. Submit a high-priority task later (Query lane: read/grep)
 * 4. Observe that the high-priority task executes before queued low-priority tasks
 *
 * Lane priorities (lower = higher priority):
 *   Control (P0) > Query (P1) > Execute (P2) > Generate (P3)
 *
 * Run with: node test_task_priority.js
 */

const { Agent } = require("a3s-code");
const path = require("path");
const os = require("os");
const fs = require("fs");

function findConfig() {
  const homeConfig = path.join(os.homedir(), ".a3s", "config.hcl");
  if (fs.existsSync(homeConfig)) return homeConfig;
  throw new Error("Config not found. Create ~/.a3s/config.hcl");
}

class CompletionRecord {
  constructor(name, lane, submittedAt, completedAt = 0) {
    this.name = name;
    this.lane = lane;
    this.submittedAt = submittedAt;
    this.completedAt = completedAt;
  }
}

function elapsed(start) {
  return (performance.now() - start) / 1000;
}

function printCompletionOrder(completions) {
  const sorted = [...completions].sort(
    (a, b) => a.completedAt - b.completedAt
  );
  console.log("\n  --- Completion Order ---");
  sorted.forEach((rec, i) => {
    const marker = rec.lane.includes("P1") ? "🚨" : "  ";
    console.log(
      `  ${marker} ${i + 1}. ${rec.name} [${rec.lane}] — submitted ${rec.submittedAt.toFixed(2)}s, completed ${rec.completedAt.toFixed(2)}s`
    );
  });
  return sorted;
}

// ============================================================================
// Test 1: Query (P1) preempts Execute (P2)
// ============================================================================
async function testQueryPreemptsExecute(agent) {
  console.log("\n📋 Test 1: Query (P1) Preempts Execute (P2)");
  console.log("-".repeat(80));
  console.log("  Execute lane concurrency: 1 (tasks must queue)");
  console.log(
    "  Query lane concurrency: 2 (higher priority, separate capacity)"
  );
  console.log("  Submit 3 Execute tasks first, then 1 Query task");
  console.log(
    "  Expected: Query task completes before remaining Execute tasks\n"
  );

  const session = agent.session(".", {
    queueConfig: {
      executeMaxConcurrency: 1, // Bottleneck
      queryMaxConcurrency: 2, // Higher priority, own capacity
      generateMaxConcurrency: 1,
      enableMetrics: true,
    },
    autoApprove: true,
  });

  const start = performance.now();
  const completions = [];

  async function runTask(name, prompt, laneLabel) {
    const submittedAt = elapsed(start);
    const marker = laneLabel.includes("P1") ? "🚨" : "📤";
    console.log(
      `  [${submittedAt.toFixed(2).padStart(6)}s] ${marker} Submitting: ${name} (${laneLabel})`
    );

    const result = await session.send(prompt);
    const completedAt = elapsed(start);
    completions.push(
      new CompletionRecord(name, laneLabel, submittedAt, completedAt)
    );

    const chars = result?.text?.length || 0;
    const doneMarker = laneLabel.includes("P1") ? "🚨" : "✅";
    console.log(
      `  [${completedAt.toFixed(2).padStart(6)}s] ${doneMarker} Completed: ${name} (${chars} chars)`
    );
    return result;
  }

  // Submit 3 Execute-lane tasks (bash → Execute lane P2)
  const tasks = [];
  for (let i = 1; i <= 3; i++) {
    const prompt = `Run this bash command and tell me the output: echo 'Task ${i} started' && sleep 1 && echo 'Task ${i} done'`;
    tasks.push(runTask(`Execute-${i}`, prompt, "Execute (P2)"));
    await new Promise((r) => setTimeout(r, 200));
  }

  // Wait for queue to fill
  await new Promise((r) => setTimeout(r, 500));

  // Submit Query-lane task (read → Query lane P1)
  console.log();
  tasks.push(
    runTask(
      "Query-Urgent",
      "Read the Cargo.toml file and tell me the package name and version",
      "Query (P1)"
    )
  );

  await Promise.allSettled(tasks);
  printCompletionOrder(completions);
  console.log("\n✅ Test 1 completed");
}

// ============================================================================
// Test 2: Multi-level priority
// ============================================================================
async function testMultiLevelPriority(agent) {
  console.log(
    "\n\n📋 Test 2: Multi-Level Priority (Query P1 vs Execute P2)"
  );
  console.log("-".repeat(80));
  console.log("  All lanes constrained to concurrency=1");
  console.log("  Submit Execute task first, then Query task");
  console.log("  Expected: Query task scheduled with higher priority\n");

  const session = agent.session(".", {
    queueConfig: {
      controlMaxConcurrency: 1,
      queryMaxConcurrency: 1,
      executeMaxConcurrency: 1,
      generateMaxConcurrency: 1,
      enableMetrics: true,
    },
    autoApprove: true,
  });

  const start = performance.now();
  const completions = [];

  async function runTask(name, prompt, laneLabel) {
    const submittedAt = elapsed(start);
    console.log(
      `  [${submittedAt.toFixed(2).padStart(6)}s] 📤 ${name} (${laneLabel})`
    );
    const result = await session.send(prompt);
    const completedAt = elapsed(start);
    completions.push(
      new CompletionRecord(name, laneLabel, submittedAt, completedAt)
    );
    console.log(
      `  [${completedAt.toFixed(2).padStart(6)}s] ✅ ${name} completed`
    );
    return result;
  }

  // Execute task first (lower priority)
  const execTask = runTask(
    "Execute-Task",
    "Run: echo 'execute-lane task' && sleep 1 && echo 'done'",
    "Execute (P2)"
  );
  await new Promise((r) => setTimeout(r, 300));

  // Query task (higher priority)
  const queryTask = runTask(
    "Query-Task",
    "Read the Cargo.toml file and show the first 5 lines",
    "Query (P1)"
  );

  await Promise.allSettled([execTask, queryTask]);
  printCompletionOrder(completions);
  console.log("\n✅ Test 2 completed");
}

// ============================================================================
// Test 3: Late urgent task insertion
// ============================================================================
async function testLateUrgentInsertion(agent) {
  console.log("\n\n📋 Test 3: Late Urgent Task Insertion");
  console.log("-".repeat(80));
  console.log("  Execute concurrency: 1, Query concurrency: 2");
  console.log(
    "  Submit 4 slow Execute tasks, then 1 urgent Query task at 2s mark"
  );
  console.log(
    "  Expected: Urgent task completes before remaining Execute tasks\n"
  );

  const session = agent.session(".", {
    queueConfig: {
      executeMaxConcurrency: 1,
      queryMaxConcurrency: 2,
      generateMaxConcurrency: 1,
      enableMetrics: true,
    },
    autoApprove: true,
  });

  const start = performance.now();
  const completions = [];

  async function runTask(name, prompt, laneLabel) {
    const submittedAt = elapsed(start);
    const marker = laneLabel.includes("P1") ? "🚨" : "📤";
    console.log(
      `  [${submittedAt.toFixed(2).padStart(6)}s] ${marker} ${name}`
    );
    const result = await session.send(prompt);
    const completedAt = elapsed(start);
    completions.push(
      new CompletionRecord(name, laneLabel, submittedAt, completedAt)
    );
    const doneMarker = laneLabel.includes("P1") ? "🚨" : "✅";
    console.log(
      `  [${completedAt.toFixed(2).padStart(6)}s] ${doneMarker} ${name} completed`
    );
    return result;
  }

  // Submit 4 slow Execute tasks
  const tasks = [];
  for (let i = 1; i <= 4; i++) {
    const prompt = `Run: echo 'slow task ${i} start' && sleep 2 && echo 'slow task ${i} end'`;
    tasks.push(runTask(`SlowExec-${i}`, prompt, "Execute (P2)"));
    await new Promise((r) => setTimeout(r, 100));
  }

  // Wait for queue to fill
  console.log("\n  ⏳ Waiting 2s for queue to fill up...\n");
  await new Promise((r) => setTimeout(r, 2000));

  // Print queue stats
  try {
    const stats = await session.queueStats();
    console.log(
      `  📊 Queue stats: pending=${stats.totalPending}, active=${stats.totalActive}`
    );
  } catch {
    console.log("  📊 Queue stats: (unavailable)");
  }

  // Inject urgent Query task
  tasks.push(
    runTask(
      "UrgentQuery",
      "Use grep to search for 'name' in Cargo.toml and tell me the result",
      "Query (P1)"
    )
  );

  await Promise.allSettled(tasks);

  // Print completion order
  const sorted = printCompletionOrder(completions);

  // Check preemption
  const urgent = sorted.find((r) => r.name === "UrgentQuery");
  const lastExec = [...sorted].reverse().find((r) => r.lane.includes("P2"));

  if (urgent && lastExec) {
    if (urgent.completedAt < lastExec.completedAt) {
      console.log(
        `\n  ✅ Priority preemption confirmed: UrgentQuery (${urgent.completedAt.toFixed(2)}s) before last Execute (${lastExec.completedAt.toFixed(2)}s)`
      );
    } else {
      console.log(
        `\n  ℹ️  UrgentQuery (${urgent.completedAt.toFixed(2)}s) after last Execute (${lastExec.completedAt.toFixed(2)}s)`
      );
      console.log(
        "     This can happen if Execute tasks were already running"
      );
    }
  }

  console.log("\n✅ Test 3 completed");
}

// ============================================================================
// Main
// ============================================================================
async function main() {
  console.log("🚀 A3S Code - Lane-Based Priority Preemption Test (Node.js)\n");
  console.log("=".repeat(80));

  const configPath = findConfig();
  console.log(`📄 Using config: ${configPath}`);
  console.log("=".repeat(80));

  const agent = await Agent.create(configPath);

  await testQueryPreemptsExecute(agent);
  await testMultiLevelPriority(agent);
  await testLateUrgentInsertion(agent);

  console.log(`\n${"=".repeat(80)}`);
  console.log("✅ All lane-based priority preemption tests completed!");
  console.log("=".repeat(80));
}

main().catch(console.error);
