/**
 * Git Worktree Tool Test with Real LLM
 *
 * Demonstrates the git_worktree builtin tool via the Node.js SDK:
 * 1. Initialize a git repo in a temp directory
 * 2. Direct tool calls: status, create, list, remove
 * 3. LLM-driven: ask the agent to use git_worktree
 *
 * Run with: node test_git_worktree.js
 */

const { Agent } = require("a3s-code");
const path = require("path");
const os = require("os");
const fs = require("fs");
const { execSync } = require("child_process");

function findConfig() {
  if (process.env.A3S_CONFIG) return process.env.A3S_CONFIG;
  const homeConfig = path.join(os.homedir(), ".a3s", "config.hcl");
  if (fs.existsSync(homeConfig)) return homeConfig;
  throw new Error("Config not found. Create ~/.a3s/config.hcl or set A3S_CONFIG");
}

function initGitRepo(dir) {
  execSync("git init", { cwd: dir, stdio: "pipe" });
  execSync('git config user.email "test@example.com"', { cwd: dir, stdio: "pipe" });
  execSync('git config user.name "Test User"', { cwd: dir, stdio: "pipe" });
  fs.writeFileSync(path.join(dir, "README.md"), "# Test Repo\n");
  execSync("git add .", { cwd: dir, stdio: "pipe" });
  execSync('git commit -m "Initial commit"', { cwd: dir, stdio: "pipe" });
}

function assert(condition, message) {
  if (!condition) throw new Error(`Assertion failed: ${message}`);
}

async function main() {
  const config = findConfig();
  console.log(`Config: ${config}`);

  const agent = await Agent.create(config);
  console.log("Agent created ✓");

  // Create temp workspace
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), "a3s-wt-"));
  try {
    initGitRepo(workspace);
    console.log(`Git repo initialized at: ${workspace}\n`);

    const session = agent.session(workspace);

    // --- Test 1: Direct tool call — status ---
    console.log("═══ Test 1: git_worktree status ═══");
    let result = await session.tool("git_worktree", { command: "status" });
    console.log(result.output);
    assert(result.exitCode === 0, "status should succeed");
    console.log();

    // --- Test 2: Direct tool call — create worktree ---
    console.log("═══ Test 2: git_worktree create ═══");
    const wtPath = path.join(workspace, "wt-feature-auth");
    result = await session.tool("git_worktree", {
      command: "create",
      branch: "feature-auth",
      path: wtPath,
    });
    console.log(result.output);
    assert(result.exitCode === 0, `create failed: ${result.output}`);
    assert(fs.existsSync(wtPath), "worktree directory should exist");
    console.log();

    // --- Test 3: Direct tool call — list ---
    console.log("═══ Test 3: git_worktree list ═══");
    result = await session.tool("git_worktree", { command: "list" });
    console.log(result.output);
    assert(result.exitCode === 0, "list should succeed");
    assert(result.output.includes("feature-auth"), "list should contain the new branch");
    console.log();

    // --- Test 4: LLM-driven query ---
    console.log("═══ Test 4: LLM-driven worktree query ═══");
    const llmResult = await session.send(
      "Use the git_worktree tool with command 'list' to show me all worktrees. " +
      "Just show the tool output, nothing else."
    );
    console.log(`LLM response:\n${llmResult.text}`);
    assert(llmResult.toolCallsCount > 0, "LLM should have called git_worktree");
    console.log();

    // --- Test 5: Direct tool call — remove ---
    console.log("═══ Test 5: git_worktree remove ═══");
    result = await session.tool("git_worktree", {
      command: "remove",
      path: wtPath,
    });
    console.log(result.output);
    assert(result.exitCode === 0, `remove failed: ${result.output}`);
    assert(!fs.existsSync(wtPath), "worktree directory should be gone");
    console.log();

    // --- Test 6: Verify cleanup ---
    console.log("═══ Test 6: Verify cleanup ═══");
    result = await session.tool("git_worktree", { command: "list" });
    console.log(result.output);
    assert(!result.output.includes("feature-auth"), "feature-auth should be removed");
    console.log();

    console.log("═══ All git_worktree tests passed ✓ ═══");
  } finally {
    // Cleanup
    fs.rmSync(workspace, { recursive: true, force: true });
  }
}

main().catch((err) => {
  console.error("Test failed:", err.message);
  process.exit(1);
});
