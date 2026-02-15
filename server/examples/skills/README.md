# Skills System Tests

This directory contains tests for the skills system with different backend types.

## Running Tests

### Binary Tool Tests

```bash
# Install required tools
brew install jq  # macOS
# or
apt-get install jq  # Linux

# Test binary tool loading
cargo test --lib test_register_skill_tools -- --nocapture
```

### HTTP Tool Tests

```bash
# HTTP tools are tested with mock servers
cargo test --lib test_http_tool -- --nocapture
```

### Script Tool Tests

```bash
# Ensure interpreters are installed
which python3
which node
which bash

# Test script tool execution
cargo test --lib test_script_tool -- --nocapture
```

## Example Usage

### Loading a Custom Skill

```rust
use a3s_code::tools::ToolExecutor;

let executor = ToolExecutor::new("/tmp/workspace".to_string());

// Load skill from file
let skill_content = std::fs::read_to_string("examples/skills/binary-tool-example.md")?;
executor.register_skill(&skill_content)?;

// Tool is now available
let definitions = executor.definitions();
assert!(definitions.iter().any(|t| t.name == "jq"));
```

### Via gRPC API

```bash
# Register skill via gRPC
grpcurl -d @ localhost:4088 a3s.code.v1.CodeService/RegisterSkill <<EOF
{
  "skill_content": "$(cat examples/skills/http-tool-example.md)"
}
EOF
```

## Skill Definition Format

All skills follow this format:

```markdown
---
name: skill-name
description: Skill description
version: 1.0.0
tools:
  - name: tool-name
    description: Tool description
    backend:
      type: binary|http|script
      # Backend-specific fields...
    parameters:
      type: object
      properties:
        # JSON Schema for parameters
---

# Skill Documentation

Additional documentation in Markdown format.
```

## Backend-Specific Configuration

### Binary Backend

```yaml
backend:
  type: binary
  path: binary-name          # Required: binary name or path
  url: https://...           # Optional: download URL
  args_template: "${arg}"    # Optional: argument template
```

### HTTP Backend

```yaml
backend:
  type: http
  url: https://api.example.com    # Required: API endpoint
  method: POST                     # Optional: HTTP method (default: POST)
  headers:                         # Optional: request headers
    Authorization: Bearer ${token}
  body_template: |                 # Optional: request body template
    {"key": "${value}"}
  timeout_ms: 30000               # Optional: timeout (default: 30000)
```

### Script Backend

```yaml
backend:
  type: script
  interpreter: python3             # Required: interpreter command
  interpreter_args: ["-u"]         # Optional: interpreter flags
  script: |                        # Required: script content
    import json, os
    args = json.loads(os.environ['TOOL_ARGS'])
    print(json.dumps({"result": args}))
```

## Testing Your Skills

1. **Validate YAML**: Ensure frontmatter is valid YAML
2. **Test parameters**: Verify JSON Schema is correct
3. **Test execution**: Run the tool with sample inputs
4. **Check output**: Ensure output format is correct

Example test:

```rust
#[tokio::test]
async fn test_my_skill() {
    let executor = ToolExecutor::new("/tmp".to_string());

    // Load skill
    let skill = include_str!("../examples/skills/my-skill.md");
    executor.register_skill(skill).unwrap();

    // Execute tool
    let args = serde_json::json!({
        "param1": "value1"
    });
    let ctx = ToolContext::new("/tmp".into());
    let output = executor.execute("my-tool", &args, &ctx).await.unwrap();

    assert!(output.success);
    assert!(output.content.contains("expected"));
}
```
