/**
 * System Prompt Slots — Customizing agent personality without overriding core behavior
 *
 * Demonstrates the slot-based system prompt customization API:
 * 1. Custom role (persona)
 * 2. Custom guidelines (coding standards)
 * 3. Custom response style
 * 4. Extra freeform instructions
 * 5. Verify core tool behavior is preserved
 *
 * Run with: node test_prompt_slots.js
 */

const { Agent } = require("a3s-code");
const path = require("path");
const os = require("os");
const fs = require("fs");

function findConfig() {
  if (process.env.A3S_CONFIG) return process.env.A3S_CONFIG;
  const homeConfig = path.join(os.homedir(), ".a3s", "config.hcl");
  if (fs.existsSync(homeConfig)) return homeConfig;
  throw new Error("Config not found. Create ~/.a3s/config.hcl or set A3S_CONFIG");
}

function assert(condition, message) {
  if (!condition) throw new Error(`Assertion failed: ${message}`);
}

async function main() {
  const config = findConfig();
  console.log(`Config: ${config}`);

  const agent = await Agent.create(config);
  console.log("Agent created ✓\n");

  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), "a3s-slots-"));
  try {
    // --- Test 1: Custom role only ---
    console.log("═══ Test 1: Custom role ═══");
    let session = agent.session(workspace, {
      role: "You are a senior Rust developer who specializes in async programming.",
    });
    let result = await session.send("What is your area of expertise? Reply in one sentence.");
    console.log(`Response: ${result.text.trim()}\n`);
    assert(result.text, "should get a response");

    // --- Test 2: Role + guidelines + response style ---
    console.log("═══ Test 2: Role + guidelines + response style ═══");
    session = agent.session(workspace, {
      role: "You are a Python code reviewer.",
      guidelines: "Always check for type hints. Flag any use of `eval()`.",
      responseStyle: "Reply in bullet points. Be concise.",
    });

    // Write a Python file for the agent to review
    fs.writeFileSync(
      path.join(workspace, "app.py"),
      'def add(a, b):\n' +
      '    return eval(f"{a} + {b}")\n' +
      '\n' +
      'def greet(name):\n' +
      '    return "Hello " + name\n'
    );

    result = await session.send("Review the file app.py and list any issues you find.");
    console.log(`Response:\n${result.text.trim()}\n`);
    assert(result.toolCallsCount > 0, "should have used read_file tool");

    // --- Test 3: Extra instructions only ---
    console.log("═══ Test 3: Extra instructions ═══");
    session = agent.session(workspace, {
      extra: "Always end your response with '— A3S'",
    });
    result = await session.send("Say hello.");
    console.log(`Response: ${result.text.trim()}\n`);

    // --- Test 4: Verify tools still work (core behavior preserved) ---
    console.log("═══ Test 4: Core tool behavior preserved ═══");
    session = agent.session(workspace, {
      role: "You are a minimalist file manager.",
      guidelines: "Only create files when explicitly asked.",
    });
    result = await session.send(
      "Create a file called test.txt with the content 'prompt slots work'. " +
      "Then read it back."
    );
    console.log(`Response: ${result.text.trim()}\n`);
    assert(result.toolCallsCount > 0, "should have used tools");
    assert(fs.existsSync(path.join(workspace, "test.txt")), "test.txt should exist");

    console.log("═══ All prompt slots tests passed ✓ ═══");
  } finally {
    fs.rmSync(workspace, { recursive: true, force: true });
  }
}

main().catch((err) => {
  console.error("Test failed:", err.message);
  process.exit(1);
});
