#!/usr/bin/env bash
# Test Skill Tool with Kimi Model
# This script creates a test environment and runs the Skill tool test

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE=$(mktemp -d)

echo "================================================================================"
echo "Skill Tool Test with Kimi Model"
echo "================================================================================"
echo ""
echo "📁 Workspace: $WORKSPACE"

# Create test files
cat > "$WORKSPACE/test.txt" << 'EOF'
Hello from Skill Tool test!
This is line 2.
This is line 3.
EOF

cat > "$WORKSPACE/README.md" << 'EOF'
# Test Project

This is a test project for Skill tool.
EOF

# Create test skill
mkdir -p "$WORKSPACE/skills"
cat > "$WORKSPACE/skills/file-reader.md" << 'EOF'
---
name: file-reader
description: Read and analyze files
allowed-tools: read(*), grep(*)
---

# File Reader Skill

You are a file reading specialist. You can:
- Read files using the read tool
- Search for patterns using grep
- Analyze and summarize file contents

You CANNOT:
- Write files
- Execute bash commands
- Edit files
EOF

echo "📄 Test files created:"
echo "   - test.txt"
echo "   - README.md"
echo "   - skills/file-reader.md"
echo ""

# Create test Rust program
cat > "$WORKSPACE/test_skill.rs" << 'RUST_CODE'
use a3s_code_core::{Agent, SessionOptions};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Set up Kimi API credentials from environment
    let api_key = std::env::var("KIMI_API_KEY")
        .expect("KIMI_API_KEY environment variable not set");
    let base_url = std::env::var("KIMI_BASE_URL")
        .expect("KIMI_BASE_URL environment variable not set");
    std::env::set_var("KIMI_API_KEY", &api_key);
    std::env::set_var("KIMI_BASE_URL", &base_url);

    println!("🤖 Creating agent with Kimi model...");
    let agent = Agent::from_config_file("examples/agent_kimi.hcl").await?;

    println!("📝 Creating session with file-reader skill...");
    let workspace = std::env::current_dir()?;
    let opts = SessionOptions::new()
        .with_skills_from_dir(workspace.join("skills"));

    let session = agent.session(workspace.to_str().unwrap(), opts).await?;

    println!("\n================================================================================");
    println!("Test 1: Invoke skill to read a file");
    println!("================================================================================\n");

    println!("💬 Prompt: Use the file-reader skill to read test.txt");
    match session.send("Use the file-reader skill to read test.txt and tell me what it contains").await {
        Ok(result) => {
            println!("\n✅ Response:\n{}", result.text);
            println!("\n📊 Tool calls: {}", result.tool_calls_count);
            println!("📊 Tokens: {} prompt + {} completion", result.usage.prompt_tokens, result.usage.completion_tokens);
        }
        Err(e) => {
            println!("\n❌ Error: {}", e);
        }
    }

    println!("\n================================================================================");
    println!("Test 2: Invoke skill to search in files");
    println!("================================================================================\n");

    println!("💬 Prompt: Use the file-reader skill to search for 'test' in all files");
    match session.send("Use the file-reader skill to search for the word 'test' in all files").await {
        Ok(result) => {
            println!("\n✅ Response:\n{}", result.text);
            println!("\n📊 Tool calls: {}", result.tool_calls_count);
            println!("📊 Tokens: {} prompt + {} completion", result.usage.prompt_tokens, result.usage.completion_tokens);
        }
        Err(e) => {
            println!("\n❌ Error: {}", e);
        }
    }

    println!("\n================================================================================");
    println!("Test Complete!");
    println!("================================================================================");

    Ok(())
}
RUST_CODE

echo "🔨 Building test program..."
cd "$WORKSPACE"
cargo init --name test_skill_tool --bin > /dev/null 2>&1

# Add dependencies
cat > Cargo.toml << 'EOF'
[package]
name = "test_skill_tool"
version = "0.1.0"
edition = "2021"

[dependencies]
a3s-code-core = { path = "../../../core" }
tokio = { version = "1", features = ["full"] }
anyhow = "1"
EOF

# Copy the test code
mv test_skill.rs src/main.rs

echo "🚀 Running test..."
echo ""

cargo run --release

# Cleanup
cd /
rm -rf "$WORKSPACE"

echo ""
echo "✅ Test completed and workspace cleaned up"
