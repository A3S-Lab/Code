//! Tool Executor Example
//!
//! Demonstrates the tool execution system:
//! - Listing available tool definitions
//! - Executing script-based tools
//! - Dynamic tool registration
//! - Error handling for unknown tools
//!
//! Note: Built-in tools (read, write, bash, etc.) delegate to the `a3s-tools`
//! binary via TOOL_ARGS env var. This example focuses on script-based tools
//! which work standalone without external binaries.
//!
//! Run with:
//!   cargo run --example tool_executor

use a3s_box_code::tools::ToolExecutor;
use serde_json::json;

#[tokio::main]
async fn main() {
    // Initialize tracing for tool execution logs
    tracing_subscriber::fmt()
        .with_env_filter("a3s_box_code=info")
        .init();

    println!("╔══════════════════════════════════════════════════╗");
    println!("║         A3S Code - Tool Executor Demo            ║");
    println!("╚══════════════════════════════════════════════════╝\n");

    // Create a temporary workspace for safe tool execution
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let workspace = temp_dir.path().to_string_lossy().to_string();
    println!("📁 Workspace: {}\n", workspace);

    let executor = ToolExecutor::new(workspace);

    // --- 1. List available tools ---
    let definitions = executor.definitions();
    println!("🔧 Available Tools ({}):", definitions.len());
    for tool in &definitions {
        println!("  • {:<12} {}", tool.name, truncate(&tool.description, 55));
    }
    println!();

    // --- 2. Register custom script tools ---
    println!("━━━ Register Custom Script Tools ━━━");
    let skill_content = r#"---
name: demo-tools
description: Demo tools for the example
tools:
  - name: greet
    description: Greet someone by name
    backend:
      type: script
      interpreter: bash
      script: echo "Hello, $TOOL_ARG_NAME! Welcome to A3S Code."
    parameters:
      type: object
      properties:
        name:
          type: string
          description: Name to greet
      required:
        - name

  - name: system-info
    description: Get basic system information
    backend:
      type: script
      interpreter: bash
      script: |
        echo "OS: $(uname -s)"
        echo "Kernel: $(uname -r)"
        echo "Arch: $(uname -m)"
        echo "User: $(whoami)"
        echo "Shell: $SHELL"
        echo "Date: $(date)"
    parameters:
      type: object
      properties: {}

  - name: word-count
    description: Count words in given text
    backend:
      type: script
      interpreter: bash
      script: |
        echo "$TOOL_ARG_TEXT" | wc -w | tr -d ' '
    parameters:
      type: object
      properties:
        text:
          type: string
          description: Text to count words in
      required:
        - text

  - name: json-format
    description: Format and validate JSON data
    backend:
      type: script
      interpreter: python3
      script: |
        import json, os, sys
        args = json.loads(os.environ.get('TOOL_ARGS', '{}'))
        data = args.get('data', '')
        try:
            parsed = json.loads(data)
            print(json.dumps(parsed, indent=2, ensure_ascii=False))
        except json.JSONDecodeError as e:
            print(f"Invalid JSON: {e}", file=sys.stderr)
            sys.exit(1)
    parameters:
      type: object
      properties:
        data:
          type: string
          description: JSON string to format
      required:
        - data

  - name: math-calc
    description: Perform basic math calculations
    backend:
      type: script
      interpreter: python3
      script: |
        import json, os, sys
        args = json.loads(os.environ.get('TOOL_ARGS', '{}'))
        numbers = args.get('numbers', [])
        op = args.get('operation', 'sum')
        try:
            nums = [float(x) for x in numbers]
            if op == 'sum': result = sum(nums)
            elif op == 'avg': result = sum(nums) / len(nums) if nums else 0
            elif op == 'max': result = max(nums) if nums else None
            elif op == 'min': result = min(nums) if nums else None
            elif op == 'product':
                result = 1
                for n in nums: result *= n
            else:
                print(f"Unknown operation: {op}", file=sys.stderr)
                sys.exit(1)
            print(json.dumps({"operation": op, "numbers": nums, "result": result}))
        except Exception as e:
            print(f"Error: {e}", file=sys.stderr)
            sys.exit(1)
    parameters:
      type: object
      properties:
        numbers:
          type: array
          items:
            type: number
          description: Array of numbers
        operation:
          type: string
          enum: ["sum", "avg", "max", "min", "product"]
          description: Math operation to perform
      required:
        - numbers
        - operation
---
Demo tools for the tool executor example.
"#;

    let registered = executor.register_skill_tools(skill_content);
    println!("  Registered {} tools: {:?}", registered.len(), registered);
    println!("  Total tools now: {}\n", executor.definitions().len());

    // --- 3. Execute greet tool ---
    println!("━━━ Execute: greet ━━━");
    let result = executor
        .execute("greet", &json!({"name": "World"}))
        .await
        .unwrap();
    print_result("greet", &result);

    // --- 4. Execute system-info tool ---
    println!("━━━ Execute: system-info ━━━");
    let result = executor.execute("system-info", &json!({})).await.unwrap();
    print_result("system-info", &result);

    // --- 5. Execute word-count tool ---
    println!("━━━ Execute: word-count ━━━");
    let result = executor
        .execute(
            "word-count",
            &json!({"text": "The quick brown fox jumps over the lazy dog"}),
        )
        .await
        .unwrap();
    print_result("word-count", &result);

    // --- 6. Execute json-format tool ---
    println!("━━━ Execute: json-format ━━━");
    let result = executor
        .execute(
            "json-format",
            &json!({
                "data": r#"{"name":"A3S Code","version":"0.1.0","features":["tools","sessions","hitl","permissions"]}"#
            }),
        )
        .await
        .unwrap();
    print_result("json-format", &result);

    // --- 7. Execute math-calc tool ---
    println!("━━━ Execute: math-calc ━━━");
    for op in &["sum", "avg", "max", "min", "product"] {
        let result = executor
            .execute(
                "math-calc",
                &json!({"numbers": [10, 20, 30, 40, 50], "operation": op}),
            )
            .await
            .unwrap();
        let status = if result.exit_code == 0 { "✅" } else { "❌" };
        println!("  {} {}: {}", status, op, result.output.trim());
    }
    println!();

    // --- 8. Error handling: unknown tool ---
    println!("━━━ Error Handling ━━━");
    let result = executor
        .execute("nonexistent_tool", &json!({"arg": "value"}))
        .await
        .unwrap();
    println!(
        "  Unknown tool → exit_code={}, output={}",
        result.exit_code,
        truncate(&result.output, 50)
    );
    println!();

    // --- 9. Dynamic unregistration ---
    println!("━━━ Dynamic Tool Management ━━━");
    println!(
        "  Before unregister: {} tools",
        executor.definitions().len()
    );
    let removed = executor.unregister_tools(&["greet".to_string(), "system-info".to_string()]);
    println!("  Removed: {:?}", removed);
    println!(
        "  After unregister:  {} tools",
        executor.definitions().len()
    );
    println!();

    println!("✅ Tool executor demo complete!");
}

fn print_result(tool: &str, result: &a3s_box_code::tools::ToolResult) {
    let status = if result.exit_code == 0 { "✅" } else { "❌" };
    println!("  {} {} (exit_code={})", status, tool, result.exit_code);
    let output = result.output.trim();
    if !output.is_empty() {
        for line in output.lines().take(10) {
            println!("    │ {}", line);
        }
        let total_lines = output.lines().count();
        if total_lines > 10 {
            println!("    │ ... ({} more lines)", total_lines - 10);
        }
    }
    println!();
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}...", &s[..max])
    } else {
        s.to_string()
    }
}
