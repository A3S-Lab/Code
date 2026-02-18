# A3S Code Node.js SDK - Integration Tests

Integration tests for the Node.js SDK using real LLM configuration.

## Prerequisites

### 1. Install the SDK

```bash
# From the node SDK directory
npm install
npm run build
```

Or install from npm:

```bash
npm install @a3s/code
```

### 2. Configuration File

Tests require a valid configuration file at one of these locations:
- `~/.a3s/config.hcl` (recommended)
- `<project_root>/.a3s/config.hcl`

**Example configuration:**

```hcl
default_model = "openai/gpt-4o"

providers {
  name = "openai"

  models {
    id          = "gpt-4o"
    name        = "GPT-4o"
    family      = "gpt"
    api_key     = "your-api-key-here"
    base_url    = "https://api.openai.com/v1"
    attachment  = false
    reasoning   = false
    tool_call   = true
    temperature = true

    modalities {
      input  = ["text"]
      output = ["text"]
    }

    limit {
      context = 128000
      output  = 4096
    }
  }
}
```

## Running Tests

### Run All Tests

```bash
node examples/integration_tests.js
```

Or with npm:

```bash
npm run test:integration
```

### Expected Output

```
🚀 A3S Code Node.js SDK - Integration Tests

================================================================================
📄 Using config: /Users/you/.a3s/config.hcl
================================================================================

📦 Test 1: Basic Tool Execution
--------------------------------------------------------------------------------
Testing: List current directory...
✓ Result preview: ...

...

✅ All Node.js SDK integration tests completed successfully!
```

## Tests Included

### Test 1: Basic Tool Execution
- List directory with `ls`
- Read file with `read`

### Test 2: File Operations
- Create file
- Read file
- Clean up

### Test 3: Search Operations
- Grep search
- Glob pattern matching

### Test 4: Direct Tool Calls
- `session.readFile()`
- `session.bash()`
- `session.glob()`
- `session.grep()`

### Test 5: Streaming Execution
- Stream events
- Count text deltas and tool calls

### Test 6: Session Options
- Configure session with custom options
- Override model if needed

### Test 7: Conversation History
- Multi-turn conversation
- Access conversation history

## Troubleshooting

### Config file not found

**Error:**
```
Error: Config file not found. Please create ~/.a3s/config.hcl
```

**Solution:**
1. Create `~/.a3s/config.hcl` with your LLM configuration
2. Or copy the project's `.a3s/config.hcl` to your home directory

### API key errors

**Error:**
```
Failed to create agent: Failed to authenticate with LLM provider
```

**Solution:**
1. Check your API key in `config.hcl`
2. Ensure the API key is valid and has sufficient credits
3. Verify the `base_url` is correct

### Module not found

**Error:**
```
Error: Cannot find module '../index.js'
```

**Solution:**
Build the SDK first:
```bash
npm run build
```

## TypeScript Support

The SDK includes TypeScript definitions in `index.d.ts`. For TypeScript projects:

```typescript
import { Agent, AgentResult, SessionOptions } from '@a3s/code';

async function main() {
  const agent = await Agent.create('~/.a3s/config.hcl');
  const session = agent.session('.', { model: 'openai/gpt-4o' });
  const result: AgentResult = await session.send('Hello!');
  console.log(result.text);
}
```

## License

MIT License - See LICENSE file for details
