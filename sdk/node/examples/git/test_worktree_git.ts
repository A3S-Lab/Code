/**
 * Git Worktree Tool Test with Real LLM
 *
 * Demonstrates the git builtin tool's worktree support via the Node.js SDK:
 * 1. Initialize a git repo in a temp directory
 * 2. Direct tool calls: status, create, list, remove
 * 3. LLM-driven: ask the agent to use git worktree
 *
 * Run with: npx ts-node examples/git/test_worktree_git.ts
 */

import { Agent, Session, AgentResult, ToolResult } from "../../index.js";
import * as path from "path";
import * as os from "os";
import * as fs from "fs";
import { execSync } from "child_process";

class GitWorktreeTest {
  private readonly agent: Agent;
  private readonly configPath: string;

  constructor(agent: Agent, configPath: string) {
    this.agent = agent;
    this.configPath = configPath;
  }

  static findConfig(): string {
    if (process.env.A3S_CONFIG) return process.env.A3S_CONFIG;
    const homeConfig: string = path.join(os.homedir(), ".a3s", "config.acl");
    if (fs.existsSync(homeConfig)) return homeConfig;
    throw new Error("Config not found. Create ~/.a3s/config.acl or set A3S_CONFIG");
  }

  static initGitRepo(dir: string): void {
    execSync("git init", { cwd: dir, stdio: "pipe" });
    execSync('git config user.email "test@example.com"', { cwd: dir, stdio: "pipe" });
    execSync('git config user.name "Test User"', { cwd: dir, stdio: "pipe" });
    fs.writeFileSync(path.join(dir, "README.md"), "# Test Repo\n");
    execSync("git add .", { cwd: dir, stdio: "pipe" });
    execSync('git commit -m "Initial commit"', { cwd: dir, stdio: "pipe" });
  }

  static assert(condition: boolean, message: string): void {
    if (!condition) throw new Error(`Assertion failed: ${message}`);
  }

  async runAll(): Promise<void> {
    console.log(`Config: ${this.configPath}`);
    console.log("Agent created");

    // Create temp workspace
    const workspace: string = fs.mkdtempSync(path.join(os.tmpdir(), "a3s-wt-"));
    try {
      GitWorktreeTest.initGitRepo(workspace);
      console.log(`Git repo initialized at: ${workspace}\n`);

      const session: Session = this.agent.session(workspace);

      // --- Test 1: Direct tool call -- status ---
      console.log("=== Test 1: git status ===");
      let result: ToolResult = await session.git({ command: "status" });
      console.log(result.output);
      GitWorktreeTest.assert(result.exitCode === 0, "status should succeed");
      console.log();

      // --- Test 2: Direct tool call -- create worktree ---
      console.log("=== Test 2: git worktree create ===");
      const wtPath: string = path.join(workspace, "wt-feature-auth");
      result = await session.git({
        command: "worktree",
        subcommand: "create",
        name: "feature-auth",
        path: wtPath,
      });
      console.log(result.output);
      GitWorktreeTest.assert(result.exitCode === 0, `create failed: ${result.output}`);
      GitWorktreeTest.assert(fs.existsSync(wtPath), "worktree directory should exist");
      console.log();

      // --- Test 3: Direct tool call -- list ---
      console.log("=== Test 3: git worktree list ===");
      result = await session.git({ command: "worktree", subcommand: "list" });
      console.log(result.output);
      GitWorktreeTest.assert(result.exitCode === 0, "list should succeed");
      GitWorktreeTest.assert(result.output.includes("feature-auth"), "list should contain the new branch");
      console.log();

      // --- Test 4: LLM-driven query ---
      console.log("=== Test 4: LLM-driven worktree query ===");
      const llmResult: AgentResult = await session.send(
        "Use the git tool with command 'worktree' and subcommand 'list' to show me all worktrees. " +
        "Just show the tool output, nothing else."
      );
      console.log(`LLM response:\n${llmResult.text}`);
      GitWorktreeTest.assert(llmResult.toolCallsCount > 0, "LLM should have called git");
      console.log();

      // --- Test 5: Direct tool call -- remove ---
      console.log("=== Test 5: git worktree remove ===");
      result = await session.git({
        command: "worktree",
        subcommand: "remove",
        path: wtPath,
      });
      console.log(result.output);
      GitWorktreeTest.assert(result.exitCode === 0, `remove failed: ${result.output}`);
      GitWorktreeTest.assert(!fs.existsSync(wtPath), "worktree directory should be gone");
      console.log();

      // --- Test 6: Verify cleanup ---
      console.log("=== Test 6: Verify cleanup ===");
      result = await session.git({ command: "worktree", subcommand: "list" });
      console.log(result.output);
      GitWorktreeTest.assert(!result.output.includes("feature-auth"), "feature-auth should be removed");
      console.log();

      console.log("=== All git worktree tests passed ===");
    } finally {
      // Cleanup
      fs.rmSync(workspace, { recursive: true, force: true });
    }
  }
}

async function main(): Promise<void> {
  const configPath: string = GitWorktreeTest.findConfig();
  const agent: Agent = await Agent.create(configPath);
  const test = new GitWorktreeTest(agent, configPath);
  await test.runAll();
}

main().catch((err: unknown) => {
  console.error("Test failed:", (err as Error).message);
  process.exit(1);
});
