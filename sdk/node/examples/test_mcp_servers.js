#!/usr/bin/env node
/**
 * A3S Code Node.js SDK - Integration Tests
 *
 * Tests all recently added/fixed features against the real kimi endpoint:
 *   1. task tool (delegate to general subagent, wait for result)
 *   2. parallel_task (fan-out to multiple subagents concurrently)
 *   3. toolNames() initial state on a fresh session
 *   4. mcpStatus error field populated on failed connect
 *   5. MCP injection: add → status → LLM use → toolNames → remove
 *   6. refreshMcpTools (smoke test)
 *
 * Run with: node examples/test_mcp_servers.js
 * Requires: ~/.a3s/config.hcl or repo .a3s/config.hcl
 */

const { Agent } = require("../index.js");
const crypto = require("crypto");
const os = require("os");
const path = require("path");
const fs = require("fs");

// Bundled minimal MCP echo server — no external dependencies required
const ECHO_SERVER = path.join(__dirname, "mcp_echo_server.js");

function resolveConfig() {
  const homeConfig = path.join(os.homedir(), ".a3s", "config.hcl");
  if (fs.existsSync(homeConfig)) return homeConfig;
  let dir = __dirname;
  for (let i = 0; i < 10; i++) {
    const candidate = path.join(dir, ".a3s", "config.hcl");
    if (fs.existsSync(candidate)) return candidate;
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  throw new Error("Config not found at .a3s/config.hcl or ~/.a3s/config.hcl");
}

function pass(label) {
  console.log(`  ✓  ${label}`);
}

// ── Test 1: task tool ─────────────────────────────────────────────────────────

async function testTaskTool(agent, tmpdir) {
  console.log("\n── Test 1: task tool (subagent delegation) ──");
  const session = agent.session(tmpdir, { permissive: true });
  const result = await session.send(
    "Use the task tool to delegate the following to the 'general' agent, " +
    "then return its exact reply verbatim: " +
    "'Reply with exactly the word: TASK_OK'"
  );
  if (!result.text.includes("TASK_OK")) {
    throw new Error(`task tool should return TASK_OK, got: ${result.text}`);
  }
  pass("task tool delegated and returned TASK_OK");
}

// ── Test 2: parallel_task ─────────────────────────────────────────────────────

async function testParallelTask(agent, tmpdir) {
  console.log("\n── Test 2: parallel_task (concurrent fan-out) ──");
  const session = agent.session(tmpdir, { permissive: true });
  const result = await session.send(
    "Use the parallel_task tool to run these three tasks concurrently " +
    "using the 'general' agent, then list their outputs:\n" +
    "1. Reply with exactly: PARALLEL_A\n" +
    "2. Reply with exactly: PARALLEL_B\n" +
    "3. Reply with exactly: PARALLEL_C"
  );
  for (const token of ["PARALLEL_A", "PARALLEL_B", "PARALLEL_C"]) {
    if (!result.text.includes(token)) {
      throw new Error(`expected ${token} in result, got: ${result.text}`);
    }
  }
  pass("parallel_task ran 3 subagents concurrently, all results returned");
}

// ── Test 3: toolNames initial state ──────────────────────────────────────────

async function testToolNames(agent, tmpdir) {
  console.log("\n── Test 3: toolNames() initial state ──");
  const session = agent.session(tmpdir, { permissive: true });

  const names = session.toolNames();
  if (names.length === 0) {
    throw new Error("expected built-in tools to be present");
  }
  pass(`toolNames() returns ${names.length} built-in tools`);

  const mcpNames = names.filter(n => n.startsWith("mcp__"));
  if (mcpNames.length > 0) {
    throw new Error(
      `expected no mcp__ tools on a fresh session (none configured globally), got: ${mcpNames}`
    );
  }
  pass("no mcp__ tools on fresh session (none configured globally)");
}

// ── Test 4: mcpStatus error field ─────────────────────────────────────────────

async function testMcpStatusError(agent, tmpdir) {
  console.log("\n── Test 4: mcpStatus error field on failed connect ──");
  const session = agent.session(tmpdir, { permissive: true });

  let err = null;
  try {
    await session.addMcpServer(
      "bad-server", "stdio", "nonexistent-mcp-binary-xyz", [], null, null, null
    );
  } catch (e) {
    err = e;
  }
  if (!err) throw new Error("addMcpServer must throw for a nonexistent binary");

  // The config is registered before connection is attempted, so the server
  // must always appear in mcpStatus — even on failure.
  const status = await session.mcpStatus();
  const badEntry = status.find(s => s.name === "bad-server");
  if (!badEntry) {
    throw new Error(
      `bad-server must appear in mcpStatus after failed connect, ` +
      `got names: ${status.map(s => s.name)}`
    );
  }
  if (badEntry.connected) {
    throw new Error("bad-server should not be connected");
  }
  if (!badEntry.error) {
    throw new Error(`error field must be populated after failed connect, got: ${JSON.stringify(badEntry)}`);
  }
  pass(`mcpStatus.error captured: ${badEntry.error}`);
}

// ── Test 5: MCP injection ─────────────────────────────────────────────────────

async function testMcpInjection(agent, tmpdir) {
  console.log("\n── Test 5: MCP injection (add → status → LLM use → toolNames → remove) ──");
  const session = agent.session(tmpdir, { permissive: true });

  // Before add: no mcp__echo__ tools
  const toolsBefore = session.toolNames();
  const echoToolsBefore = toolsBefore.filter(n => n.startsWith("mcp__echo__"));
  if (echoToolsBefore.length > 0) {
    throw new Error(`expected no mcp__echo__ tools before addMcpServer, got: ${echoToolsBefore}`);
  }
  pass("no mcp__echo__ tools before addMcpServer");

  // Generate a random secret unknown to the LLM — it can only be obtained by
  // actually calling mcp__echo__get_secret, preventing the LLM from faking it.
  const secret = crypto.randomBytes(8).toString("hex");

  // Add the bundled echo server (uses the same Node.js binary — no external deps)
  const count = await session.addMcpServer(
    "echo", "stdio",
    process.execPath, [ECHO_SERVER, secret],
    null, null, null
  );
  if (count === 0) throw new Error("echo server must expose >= 1 tool");
  pass(`addMcpServer registered ${count} tools`);

  // Verify mcpStatus: connected=true, toolCount=N, error=null
  const mcpStat = await session.mcpStatus();
  const echoStat = mcpStat.find(s => s.name === "echo");
  if (!echoStat) {
    throw new Error(`echo must appear in mcpStatus, got names: ${mcpStat.map(s => s.name)}`);
  }
  if (!echoStat.connected) throw new Error("echo server should be connected");
  if (echoStat.toolCount !== count) {
    throw new Error(`mcpStatus.toolCount should be ${count}, got ${echoStat.toolCount}`);
  }
  if (echoStat.error) throw new Error(`no error expected, got: ${echoStat.error}`);
  pass(`mcpStatus: connected=true, toolCount=${count}, error=null`);

  // Verify toolNames reflects the injected tools
  const toolsAfter = session.toolNames();
  const mcpTools = toolsAfter.filter(n => n.startsWith("mcp__echo__"));
  if (mcpTools.length !== count) {
    throw new Error(`toolNames should show ${count} mcp__echo__ tools, got ${mcpTools.length}`);
  }
  pass(`toolNames shows ${mcpTools.length} mcp__echo__ tools`);

  // LLM must actually call the MCP tool to retrieve the secret.
  // The secret value is NOT in the prompt — the LLM cannot fake this.
  const mcpResult = await session.send(
    "Use the mcp__echo__get_secret tool to retrieve the secret value, " +
    "then tell me exactly what it returned."
  );
  if (!mcpResult.text.includes(secret)) {
    throw new Error(
      `LLM should have called mcp__echo__get_secret and returned the secret, ` +
      `got: ${mcpResult.text}`
    );
  }
  pass("LLM used mcp__echo__get_secret tool and returned the correct secret");

  // Remove server: tools disappear
  await session.removeMcpServer("echo");
  const toolsFinal = session.toolNames();
  if (toolsFinal.some(n => n.startsWith("mcp__echo__"))) {
    throw new Error("mcp__echo__ tools should be gone after removeMcpServer");
  }
  pass("removeMcpServer removed all mcp__echo__ tools");
}

// ── Test 6: refreshMcpTools ───────────────────────────────────────────────────

async function testRefreshMcpTools(agent) {
  console.log("\n── Test 6: refreshMcpTools (smoke test) ──");
  await agent.refreshMcpTools();
  pass("refreshMcpTools completed without error");
}

// ── Main ──────────────────────────────────────────────────────────────────────

async function main() {
  const configPath = resolveConfig();
  console.log("=== A3S Code Node.js SDK - Integration Tests ===");
  console.log(`config: ${configPath}\n`);

  const agent = await Agent.create(configPath);
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), "a3s-test-"));

  try {
    const tests = [
      () => testTaskTool(agent, tmpdir),
      () => testParallelTask(agent, tmpdir),
      () => testToolNames(agent, tmpdir),
      () => testMcpStatusError(agent, tmpdir),
      () => testMcpInjection(agent, tmpdir),
      () => testRefreshMcpTools(agent),
    ];

    let passed = 0;
    let failed = 0;

    for (const test of tests) {
      try {
        await test();
        passed++;
      } catch (err) {
        console.error(`\n  FAILED: ${err.message}`);
        failed++;
      }
    }

    console.log("\n" + "=".repeat(48));
    if (failed === 0) {
      console.log(`=== all ${passed} tests passed ===`);
    } else {
      console.log(`=== ${passed} passed, ${failed} failed ===`);
      process.exit(1);
    }
    console.log("=".repeat(48));
  } finally {
    fs.rmSync(tmpdir, { recursive: true, force: true });
  }
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
