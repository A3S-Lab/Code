//! Test Skill Tool with Kimi Model
//!
//! This example demonstrates:
//! 1. Creating a session with a custom skill
//! 2. Invoking the skill through the Skill tool
//! 3. Verifying the skill can use its allowed tools (read, grep)

use a3s_code_core::{Agent, SessionOptions};

#[tokio::main]
async fn main() {
    // Set up Kimi API credentials from config
    std::env::set_var("KIMI_API_KEY", "sk-ZaH1YnkiGmcBt8qxKWfsBV5w9aInp4QuDUeq1HEIOAzEg5cT");
    std::env::set_var("KIMI_BASE_URL", "http://35.220.164.252:3888/v1");

    println!("================================================================================");
    println!("Skill Tool Test with Kimi Model");
    println!("================================================================================\n");

    println!("🤖 Creating agent with Kimi model...");
    let agent = match Agent::create("examples/agent_kimi.hcl").await {
        Ok(a) => a,
        Err(e) => { eprintln!("❌ Failed to create agent: {}", e); return; }
    };

    println!("📝 Creating session with file-reader skill (permissive mode)...");
    let workspace = std::path::PathBuf::from("examples");
    let opts = SessionOptions::new()
        .with_skills_from_dir(workspace.join("skills"))
        .with_permissive_policy();  // Allow all tools without confirmation

    let session = match agent.session(workspace.to_str().unwrap(), Some(opts)) {
        Ok(s) => s,
        Err(e) => { eprintln!("❌ Failed to create session: {}", e); return; }
    };

    println!("\n================================================================================");
    println!("Test 1: Invoke skill to read a file");
    println!("================================================================================\n");

    println!("💬 Prompt: Use the file-reader skill to read test_data.txt");
    match session.send("Use the file-reader skill to read test_data.txt and tell me what it contains", None).await {
        Ok(result) => {
            println!("\n✅ Response:\n{}", result.text);
            println!("\n📊 Tool calls: {}", result.tool_calls_count);
            println!("📊 Tokens: {} prompt + {} completion",
                result.usage.prompt_tokens, result.usage.completion_tokens);
        }
        Err(e) => eprintln!("\n❌ Error: {}", e),
    }

    println!("\n================================================================================");
    println!("Test 2: Invoke skill to search in files");
    println!("================================================================================\n");

    println!("💬 Prompt: Use the file-reader skill to search for 'Skill' in test_data.txt");
    match session.send("Use the file-reader skill to search for the word 'Skill' in test_data.txt", None).await {
        Ok(result) => {
            println!("\n✅ Response:\n{}", result.text);
            println!("\n📊 Tool calls: {}", result.tool_calls_count);
            println!("📊 Tokens: {} prompt + {} completion",
                result.usage.prompt_tokens, result.usage.completion_tokens);
        }
        Err(e) => eprintln!("\n❌ Error: {}", e),
    }

    println!("\n================================================================================");
    println!("Test Complete!");
    println!("================================================================================");
}
