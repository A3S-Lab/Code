# AHP Safety Harness Servers

Two production-ready AHP (Agent Harness Protocol) servers for securing AI agent operations in a3s-code.

## Overview

These harness servers provide defense-in-depth security for AI agents:

1. **Pre-Action Guard** (`ahp_pre_action_guard.py`) - Intercepts dangerous operations before execution
2. **Post-Action Sanitizer** (`ahp_post_action_sanitizer.py`) - Sanitizes untrusted outputs to prevent injection attacks

## Features

### Pre-Action Guard

**Purpose**: Prevent dangerous operations before they execute

**Capabilities**:
- ✅ Command injection detection
- ✅ Dangerous command patterns (rm -rf, dd, mkfs, fork bombs, etc.)
- ✅ Sensitive path validation (blocks /etc/passwd, ~/.ssh, etc.)
- ✅ SSRF prevention (blocks localhost/internal IPs)
- ✅ Rate limiting (10 calls per 60 seconds per tool)

**Blocked Patterns**:
```bash
rm -rf /              # Recursive delete from root
dd if=.*of=/dev/      # Disk operations
mkfs.                 # Format filesystem
:(){.*}               # Fork bomb
> /dev/sd[a-z]        # Write to disk device
chmod 777             # Overly permissive permissions
curl.*| bash          # Pipe to shell
eval(                 # Eval injection
```

**Sensitive Paths**:
```
/etc/passwd, /etc/shadow
/root/.ssh, ~/.ssh/id_rsa
/proc/, /sys/
~/.aws/credentials
~/.config/gcloud
```

### Post-Action Sanitizer

**Purpose**: Sanitize tool outputs to prevent prompt injection and data leakage

**Capabilities**:
- ✅ Prompt injection detection
- ✅ PII redaction (API keys, passwords, emails, credit cards, SSNs, JWTs)
- ✅ Malicious payload detection (XSS, eval, exec, base64 payloads)
- ✅ Output size limiting (100KB max)
- ✅ Suspicious pattern detection

**Injection Patterns**:
```
ignore all previous instructions
disregard prior instructions
forget previous context
new instructions:
system: you are
<|im_start|>, <|im_end|>
[INST], [/INST]
### Instruction:
```

**PII Redaction**:
- API keys: `api_key=sk_test_...` → `[REDACTED_API_KEY]`
- Passwords: `password=secret123` → `[REDACTED_PASSWORD]`
- Emails: `user@example.com` → `[REDACTED_EMAIL]`
- Credit cards: `4111-1111-1111-1111` → `[REDACTED_CREDIT_CARD]`
- SSNs: `123-45-6789` → `[REDACTED_SSN]`
- JWTs: `eyJ...` → `[REDACTED_JWT]`

## Usage

### Standalone Testing

Test each harness server independently:

```bash
# Test pre-action guard
echo '{"jsonrpc":"2.0","id":"1","method":"ahp/handshake","params":{"protocol_version":"2.0","agent_info":{"framework":"test","version":"1.0","capabilities":[]},"session_id":"test","agent_id":"test"}}' | python3 examples/ahp_pre_action_guard.py

# Test post-action sanitizer
echo '{"jsonrpc":"2.0","id":"1","method":"ahp/handshake","params":{"protocol_version":"2.0","agent_info":{"framework":"test","version":"1.0","capabilities":[]},"session_id":"test","agent_id":"test"}}' | python3 examples/ahp_post_action_sanitizer.py
```

### Integration with a3s-code (Python SDK)

```python
from a3s_code import Agent, SessionOptions

# Create agent
agent = Agent.create("agent.hcl")

# Option 1: Pre-action guard only
opts = SessionOptions()
opts.ahp_transport = {
    "type": "stdio",
    "program": "python3",
    "args": ["examples/ahp_pre_action_guard.py"]
}
session = agent.session(".", opts)

# Option 2: Post-action sanitizer only
opts = SessionOptions()
opts.ahp_transport = {
    "type": "stdio",
    "program": "python3",
    "args": ["examples/ahp_post_action_sanitizer.py"]
}
session = agent.session(".", opts)

# Use the session - all tool calls will be supervised
result = session.send("List files in current directory")
```

### Integration with a3s-code (TypeScript SDK)

```typescript
import { Agent, SessionOptions } from '@a3s-lab/code';

// Create agent
const agent = await Agent.create('agent.hcl');

// Pre-action guard
const opts: SessionOptions = {
  ahpTransport: {
    type: 'stdio',
    program: 'python3',
    args: ['examples/ahp_pre_action_guard.py']
  }
};
const session = agent.session('.', opts);

// Use the session
const result = await session.send('List files in current directory');
```

### Integration with a3s-code (Rust Core)

```rust
use a3s_code_core::{Agent, SessionOptions};
use a3s_ahp::{Transport, AhpHookExecutor};

// Create AHP hook executor
let ahp_executor = AhpHookExecutor::new(Transport::Stdio {
    program: "python3".into(),
    args: vec!["examples/ahp_pre_action_guard.py".into()],
}).await?;

// Create session with AHP hook
let opts = SessionOptions::default()
    .with_hook_executor(Arc::new(ahp_executor));

let session = agent.session(".", opts).await?;
```

## Running Integration Tests

Run the comprehensive test suite:

```bash
cd crates/code
python3 test_ahp_safety.py
```

**Test Coverage**:

**Pre-Action Guard Tests**:
1. ✅ Safe command (ls -la) - should allow
2. ✅ Dangerous command (rm -rf /) - should block
3. ✅ Sensitive path (/etc/shadow) - should block
4. ✅ SSRF attempt (localhost) - should block
5. ✅ Rate limiting (15 rapid calls) - should throttle

**Post-Action Sanitizer Tests**:
1. ✅ Clean output - should pass through
2. ✅ Output with PII - should redact
3. ✅ Prompt injection - should block
4. ✅ XSS payload - should block
5. ✅ Oversized output (>100KB) - should truncate

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        AI Agent                              │
│                     (a3s-code)                               │
└────────────┬────────────────────────────┬───────────────────┘
             │                            │
             │ Pre-Action                 │ Post-Action
             │ (before tool exec)         │ (after tool exec)
             ▼                            ▼
┌────────────────────────┐   ┌───────────────────────────────┐
│  Pre-Action Guard      │   │  Post-Action Sanitizer        │
│  ─────────────────     │   │  ──────────────────────       │
│  • Command injection   │   │  • Prompt injection detection │
│  • Dangerous patterns  │   │  • PII redaction              │
│  • Path validation     │   │  • Malicious payload blocking │
│  • SSRF prevention     │   │  • Output size limiting       │
│  • Rate limiting       │   │  • Suspicious pattern detect  │
└────────────────────────┘   └───────────────────────────────┘
             │                            │
             │ Decision:                  │ Decision:
             │ allow/block/modify         │ allow/block/modify
             ▼                            ▼
┌─────────────────────────────────────────────────────────────┐
│                    Tool Execution                            │
│              (Bash, Read, Write, etc.)                       │
└─────────────────────────────────────────────────────────────┘
```

## Protocol Flow

### Pre-Action Flow

```
1. Agent wants to execute tool
2. AHP client sends pre_action event to harness
3. Harness evaluates security policies
4. Harness returns decision:
   - allow: proceed with execution
   - block: cancel execution, return error
   - modify: proceed with modified arguments
   - defer: retry after delay
5. Agent acts on decision
```

### Post-Action Flow

```
1. Tool execution completes
2. AHP client sends post_action event to harness
3. Harness sanitizes output
4. Harness returns decision:
   - allow: return output as-is
   - block: discard output, return error
   - modify: return sanitized output
5. Agent returns result to LLM
```

## Customization

### Adding Custom Patterns

**Pre-Action Guard** (`ahp_pre_action_guard.py`):

```python
# Add to DANGEROUS_PATTERNS
DANGEROUS_PATTERNS = [
    r"your_custom_pattern",
    # ...
]

# Add to SENSITIVE_PATHS
SENSITIVE_PATHS = [
    "/your/sensitive/path",
    # ...
]
```

**Post-Action Sanitizer** (`ahp_post_action_sanitizer.py`):

```python
# Add to INJECTION_PATTERNS
INJECTION_PATTERNS = [
    r"your_injection_pattern",
    # ...
]

# Add to PII_PATTERNS
PII_PATTERNS = {
    "custom_pii": r"your_pii_regex",
    # ...
}
```

### Adjusting Rate Limits

```python
# In ahp_pre_action_guard.py
RATE_LIMIT_WINDOW = timedelta(seconds=60)  # Time window
RATE_LIMIT_MAX_CALLS = 10                  # Max calls per window
```

### Adjusting Output Size Limit

```python
# In ahp_post_action_sanitizer.py
MAX_OUTPUT_SIZE = 100_000  # Characters
```

## Production Deployment

### HTTP Server Mode

For production, deploy harness servers as HTTP services:

```python
# Use http_server.py from examples/
python3 examples/http_server.py --port 8080 --harness pre_action_guard
```

Then configure agent:

```python
opts.ahp_transport = {
    "type": "http",
    "url": "http://localhost:8080/ahp",
    "auth": {"type": "bearer", "token": "your-token"}
}
```

### WebSocket Mode

For real-time bidirectional communication:

```python
python3 examples/websocket_server.py --port 8080 --harness post_action_sanitizer
```

### Chaining Multiple Harnesses

Deploy multiple harness servers and configure agent to use them sequentially:

```python
# Pre-action guard on port 8080
# Post-action sanitizer on port 8081
# Audit logger on port 8082

# Configure in agent.hcl
ahp_servers {
  name = "pre-guard"
  url  = "http://localhost:8080/ahp"
  events = ["pre_action"]
}

ahp_servers {
  name = "post-sanitizer"
  url  = "http://localhost:8081/ahp"
  events = ["post_action"]
}

ahp_servers {
  name = "audit"
  url  = "http://localhost:8082/ahp"
  events = ["*"]
}
```

## Monitoring & Logging

Both harness servers log to stderr:

```bash
# Run with logging
python3 examples/ahp_pre_action_guard.py 2> pre_guard.log
python3 examples/ahp_post_action_sanitizer.py 2> post_sanitizer.log

# Monitor logs
tail -f pre_guard.log post_sanitizer.log
```

**Log Format**:
```
[PRE-ACTION-GUARD] Checking tool: Bash
[PRE-ACTION-GUARD] BLOCKED: Dangerous pattern detected: rm\s+-rf\s+/
[POST-ACTION-SANITIZER] Sanitizing output from: Bash
[POST-ACTION-SANITIZER] PII REDACTED: ['api_key', 'email']
```

## Performance

**Latency**:
- Pre-action guard: ~5-10ms per tool call
- Post-action sanitizer: ~10-20ms per tool call (depends on output size)

**Throughput**:
- Stdio transport: ~100 requests/sec
- HTTP transport: ~500 requests/sec
- WebSocket transport: ~1000 requests/sec

## Security Considerations

1. **Defense in Depth**: Use both pre-action and post-action harnesses together
2. **Fail Open vs Fail Closed**: Current implementation fails open (allows on error) - adjust for your security requirements
3. **Pattern Maintenance**: Regularly update dangerous patterns and injection signatures
4. **Rate Limiting**: Adjust rate limits based on your workload
5. **Audit Logging**: Enable comprehensive logging for compliance
6. **Transport Security**: Use HTTPS/WSS in production with authentication

## License

MIT License - See LICENSE file for details

## Contributing

Contributions welcome! Please:
1. Add tests for new patterns
2. Update documentation
3. Follow existing code style
4. Test with integration suite

## Support

- GitHub Issues: https://github.com/A3S-Lab/Code/issues
- Documentation: https://github.com/A3S-Lab/AgentHarnessProtocol
