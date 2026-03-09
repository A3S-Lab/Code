/**
 * Test /loop and related slash commands via Node.js SDK.
 * Uses the kimi model from ~/.a3s/config.hcl.
 */

import { Agent } from "@a3s-lab/code";
import { existsSync } from "fs";
import { homedir } from "os";
import { join } from "path";
import { fileURLToPath } from "url";
import { dirname } from "path";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

function findConfig(): string {
  const candidates = [
    join(homedir(), ".a3s", "config.hcl"),
    join(__dirname, "..", "..", "..", "..", "..", ".a3s", "config.hcl"),
  ];
  for (const p of candidates) {
    if (existsSync(p)) return p;
  }
  throw new Error(`config.hcl not found in: ${candidates.join(", ")}`);
}

const CONFIG_PATH = findConfig();

function sep(title: string) {
  console.log("=".repeat(60));
  console.log(title);
  console.log("=".repeat(60));
}

async function main() {
  console.log(`Config: ${CONFIG_PATH}\n`);

  const agent = await Agent.create(CONFIG_PATH);
  const session = agent.session("/tmp/test-loop-node");

  // ── 1. listCommands() ──────────────────────────────────────────
  sep("1. listCommands() — all registered slash commands");
  const commands = session.listCommands();
  for (const cmd of commands) {
    const usage = cmd.usage ? `  usage: ${cmd.usage}` : "";
    console.log(`  /${cmd.name.padEnd(15)} ${cmd.description}${usage}`);
  }
  console.log();

  // ── 2. /help via send() ────────────────────────────────────────
  sep("2. /help via send()");
  {
    const r = await session.send("/help");
    console.log(r.text);
  }
  console.log();

  // ── 3. /loop via send() ────────────────────────────────────────
  sep("3. /loop via send() — schedule a 10-second recurring prompt");
  {
    const r = await session.send("/loop 10s say 'tick' and report the time");
    console.log(r.text);
  }
  console.log();

  // ── 4. scheduleTask() — programmatic API ──────────────────────
  sep("4. scheduleTask() — programmatic API (15s interval)");
  const taskId = session.scheduleTask("print current working directory", 15);
  console.log(`Scheduled task ID: ${taskId}`);
  console.log();

  // ── 5. listScheduledTasks() + /cron-list ──────────────────────
  sep("5. listScheduledTasks() + /cron-list");
  const tasks = session.listScheduledTasks();
  console.log(`Active tasks: ${tasks.length}`);
  for (const t of tasks) {
    console.log(
      `  [${t.id}] every ${t.intervalSecs}s — fires in ${t.nextFireInSecs}s — "${t.prompt.slice(0, 50)}"`
    );
  }
  console.log();
  {
    const r = await session.send("/cron-list");
    console.log("/cron-list output:");
    console.log(r.text);
  }
  console.log();

  // ── 6. Real LLM turn (exercises cron drain path) ──────────────
  sep("6. Real LLM turn — exercises cron drain path");
  {
    const r = await session.send("Say hello in one sentence.");
    console.log(`LLM: ${r.text.slice(0, 200)}`);
  }
  console.log();

  // ── 7. /model + /cost + /history (other slash commands) ───────
  sep("7. Other slash commands: /model, /cost, /history");
  for (const cmd of ["/model", "/cost", "/history"]) {
    const r = await session.send(cmd);
    console.log(`${cmd}: ${r.text.split("\n")[0]}`);
  }
  console.log();

  // ── 8. cancelScheduledTask() + /cron-cancel ───────────────────
  sep("8. cancelScheduledTask() + /cron-cancel");
  const cancelled = session.cancelScheduledTask(taskId);
  console.log(`cancelScheduledTask(${taskId}): ${cancelled}`);

  const remaining = session.listScheduledTasks();
  if (remaining.length > 0) {
    const loopId = remaining[0].id;
    const r = await session.send(`/cron-cancel ${loopId}`);
    console.log(`/cron-cancel ${loopId}: ${r.text}`);
  }
  console.log();

  // ── 9. Verify empty ───────────────────────────────────────────
  sep("9. Verify all tasks cancelled");
  const tasksAfter = session.listScheduledTasks();
  console.log(`Remaining tasks: ${tasksAfter.length}`);
  {
    const r = await session.send("/cron-list");
    console.log(r.text);
  }
  console.log();

  sep("All tests passed!");
}

main().catch((e) => {
  console.error("Test failed:", e);
  process.exit(1);
});
