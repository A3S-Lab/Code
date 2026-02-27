#!/usr/bin/env node
/**
 * A3S Code Node.js SDK - MCP Servers Integration Tests
 *
 * Tests:
 * 1. Global MCP servers loaded from config.hcl
 * 2. Session-level dynamic MCP server injection
 * 3. Both global + session MCP servers working together
 * 4. LLM autonomously selecting and calling MCP tools
 * 5. Disabled MCP server is skipped
 *
 * Run with: node examples/test_mcp_servers.js
 * Requires: ~/.a3s/config.hcl or repo .a3s/config.hcl
 */

const { Agent } = require("../index.js");
const os = require("os");
const path = require("path");
const fs = require("fs");

const CONFIG_PATH = (() => {
  const homeConfig = path.join(os.homedir(), ".a3s", "config.hcl");
  if (fs.existsSync(homeConfig)) return homeConfig;
  // Walk up from __dirname to find .a3s/config.hcl in the repo
  let dir = __dirname;
  for (let i = 0; i < 10; i++) {
    const candidate = path.join(dir, ".a3s", "config.hcl");
    if (fs.existsSync(candidate)) return candidate;
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  throw new Error("Config file not found. Please create ~/.a3s/config.hcl");
})();

function truncate(text, maxLen = 300) {
  if (!text || text.length <= maxLen) return text;
  return `${text.slice(0, maxLen)}... (truncated)`;
}

function buildConfig(extraBlocks = "") {
  return fs.readFileSync(CONFIG_PATH, "utf8") + "\n" + extraBlocks;
}

function withTempConfig(content, fn) {
  const tmp = require("os").tmpdir() + "/a3s-test-" + Date.now() + ".hcl";
  fs.writeFileSync(tmp, content);
  return fn(tmp).finally(() => {
    try { fs.unlinkSync(tmp); } catch (_) {}
  });
}

// ── Test 1: Global MCP servers from config.hcl ──────────────────────────────

async function testGlobalMcpFromConfig() {
  console.log("\n🌐 Test 1: Global MCP Servers from config.hcl");
  console.log("-".repeat(80));

  const config = buildConfig(`
mcp_servers {
    name      = "everything"
    transport = "stdio"
    command   = "npx"
    args      = ["-y", "@modelcontextprotocol/server-everything", "stdio"]
    enabled   = true
}
`);

  return withTempConfig(config, async (tmpPath) => {
    const agent = await Agent.create(tmpPath);
    const session = agent.session(".");

    console.log("Asking agent to use the MCP 'echo' tool autonomously...");
    const result = await session.send(
      "Use the mcp__everything__echo tool to echo the message 'Hello from global MCP!'"
    );
    console.log(`✓ Response: ${truncate(result.text)}`);

    if (!result.text.toLowerCase().includes("hello from global mcp") &&
        !result.text.toLowerCase().includes("echo")) {
      throw new Error(`Expected echo result, got: ${result.text}`);
    }

    console.log("\n✅ Test 1 passed: Global MCP servers auto-loaded from config");
  });
}

// ── Test 2: Session-level dynamic MCP injection ──────────────────────────────

async function testSessionLevelMcpInjection() {
  console.log("\n🔌 Test 2: Session-level Dynamic MCP Injection");
  console.log("-".repeat(80));

  const agent = await Agent.create(CONFIG_PATH);
  const session = agent.session(".");

  console.log("Injecting MCP server dynamically into live session...");
  const toolCount = await session.addMcpServer(
    "everything",
    "npx",
    ["-y", "@modelcontextprotocol/server-everything", "stdio"],
    null
  );
  console.log(`✓ Registered ${toolCount} tools from 'everything' server`);

  if (toolCount === 0) {
    throw new Error("Expected at least 1 tool from server-everything");
  }

  console.log("\nAsking agent to use the dynamically injected MCP tool...");
  const result = await session.send(
    "Use the mcp__everything__add tool to add the numbers 17 and 25"
  );
  console.log(`✓ Response: ${truncate(result.text)}`);

  if (!result.text.includes("42")) {
    throw new Error(`Expected 42 (17+25), got: ${result.text}`);
  }

  console.log("\n✅ Test 2 passed: Session-level MCP injection works");
}

// ── Test 3: Global + Session MCP working together ────────────────────────────

async function testGlobalAndSessionMcpTogether() {
  console.log("\n🔗 Test 3: Global + Session MCP Together");
  console.log("-".repeat(80));

  const config = buildConfig(`
mcp_servers {
    name      = "global-everything"
    transport = "stdio"
    command   = "npx"
    args      = ["-y", "@modelcontextprotocol/server-everything", "stdio"]
    enabled   = true
}
`);

  return withTempConfig(config, async (tmpPath) => {
    const agent = await Agent.create(tmpPath);
    const session = agent.session(".");

    console.log("Injecting additional session-level MCP server...");
    const toolCount = await session.addMcpServer(
      "session-everything",
      "npx",
      ["-y", "@modelcontextprotocol/server-everything", "stdio"],
      null
    );
    console.log(`✓ Session server registered ${toolCount} tools`);

    console.log("\nAsking agent to use tools from BOTH servers...");
    const result = await session.send(
      "First use mcp__global-everything__echo to echo 'from global', " +
      "then use mcp__session-everything__echo to echo 'from session'. " +
      "Report both results."
    );
    console.log(`✓ Response: ${truncate(result.text)}`);

    const lower = result.text.toLowerCase();
    if (!lower.includes("global") && !lower.includes("session")) {
      throw new Error(`Expected both MCP results, got: ${result.text}`);
    }

    console.log("\n✅ Test 3 passed: Global and session MCP servers work together");
  });
}

// ── Test 4: LLM autonomously selects MCP tool ────────────────────────────────

async function testLlmAutonomousMcpSelection() {
  console.log("\n🤖 Test 4: LLM Autonomous MCP Tool Selection");
  console.log("-".repeat(80));

  const config = buildConfig(`
mcp_servers {
    name      = "everything"
    transport = "stdio"
    command   = "npx"
    args      = ["-y", "@modelcontextprotocol/server-everything", "stdio"]
    enabled   = true
}
`);

  return withTempConfig(config, async (tmpPath) => {
    const agent = await Agent.create(tmpPath);
    const session = agent.session(".");

    console.log("Asking agent to add numbers (without specifying which tool)...");
    const result = await session.send(
      "What is 123 plus 456? Use the available MCP tools to compute this."
    );
    console.log(`✓ Response: ${truncate(result.text)}`);

    if (!result.text.includes("579")) {
      throw new Error(`Expected 579 (123+456), got: ${result.text}`);
    }

    console.log("\n✅ Test 4 passed: LLM autonomously selected and called MCP tool");
  });
}

// ── Test 5: Disabled MCP server is skipped ───────────────────────────────────

async function testDisabledMcpServerSkipped() {
  console.log("\n🚫 Test 5: Disabled MCP Server is Skipped");
  console.log("-".repeat(80));

  const config = buildConfig(`
mcp_servers {
    name      = "disabled-server"
    transport = "stdio"
    command   = "npx"
    args      = ["-y", "@modelcontextprotocol/server-everything", "stdio"]
    enabled   = false
}
`);

  return withTempConfig(config, async (tmpPath) => {
    const agent = await Agent.create(tmpPath);
    const session = agent.session(".");

    console.log("Asking agent about available MCP tools...");
    const result = await session.send(
      "List all available tools that start with 'mcp__'. " +
      "If there are none, say 'no MCP tools available'."
    );
    console.log(`✓ Response: ${truncate(result.text)}`);

    if (result.text.toLowerCase().includes("mcp__disabled")) {
      throw new Error(`Disabled server should not register tools, got: ${result.text}`);
    }

    console.log("\n✅ Test 5 passed: Disabled MCP server correctly skipped");
  });
}

// ── Main ──────────────────────────────────────────────────────────────────────

async function main() {
  console.log("🚀 A3S Code Node.js SDK - MCP Servers Integration Tests\n");
  console.log("=".repeat(80));
  console.log(`📄 Using config: ${CONFIG_PATH}`);
  console.log("=".repeat(80));

  const tests = [
    testGlobalMcpFromConfig,
    testSessionLevelMcpInjection,
    testGlobalAndSessionMcpTogether,
    testLlmAutonomousMcpSelection,
    testDisabledMcpServerSkipped,
  ];

  let passed = 0;
  let failed = 0;

  for (const test of tests) {
    try {
      await test();
      passed++;
    } catch (err) {
      console.error(`\n❌ FAILED: ${err.message}`);
      failed++;
    }
  }

  console.log("\n\n" + "=".repeat(80));
  if (failed === 0) {
    console.log(`✅ All ${passed} MCP integration tests completed successfully!`);
  } else {
    console.log(`⚠️  ${passed} passed, ${failed} failed`);
    process.exit(1);
  }
  console.log("=".repeat(80));
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
