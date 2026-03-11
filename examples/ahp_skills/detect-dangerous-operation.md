---
name: detect-dangerous-operation
description: Analyze tool calls for dangerous operations and security risks. Identifies command injection, sensitive path access, SSRF, privilege escalation, and other threats. Returns structured risk assessment with recommendations.
version: 1.0.0
category: security
---

# Detect Dangerous Operation

Analyzes tool calls before execution to identify dangerous operations and security risks.

## Purpose

This skill helps AHP Server agents evaluate the safety of tool calls from business agents. It provides structured analysis of potential threats including:

- Dangerous command patterns (rm -rf, dd, mkfs, fork bombs)
- Sensitive path access (/etc/passwd, ~/.ssh, credentials)
- SSRF risks (private IP access, localhost)
- Command injection attempts
- Path traversal attacks
- Privilege escalation attempts

## When to Use

Use this skill when:
- Analyzing pre_action events in AHP protocol
- Evaluating tool calls before execution
- Assessing security risks of shell commands
- Validating file path access
- Checking network requests for SSRF

## Input Format

Provide the tool call details in this format:

```json
{
  "tool_name": "bash",
  "arguments": {
    "command": "rm -rf /tmp/cache"
  },
  "context": {
    "user": "admin",
    "workspace": "/home/user/project"
  }
}
```

## Analysis Process

Follow these steps to analyze the operation:

### 1. Identify Dangerous Command Patterns

Check for these critical patterns:

**Critical Risk:**
- `rm -rf /` - Recursive delete from root
- `dd if=... of=/dev/...` - Direct disk operations
- `mkfs.` - Format filesystem
- `:(){ :|:& };:` - Fork bomb
- `> /dev/sd*` - Write to disk device

**High Risk:**
- `chmod 777` - Overly permissive permissions
- `chmod +s` - SUID bit setting
- `curl ... | bash` - Pipe to shell
- `eval(...)` - Eval injection
- `sudo` - Privilege escalation
- `nc -l` - Netcat listener

**Medium Risk:**
- `/bin/bash -i` - Interactive shell
- `su -` - User switching

### 2. Check Sensitive Path Access

Identify access to sensitive locations:

**Critical:**
- `/etc/shadow` - Shadow password file
- `~/.ssh/id_rsa` - SSH private keys
- `~/.aws/credentials` - AWS credentials
- `.env` files - Environment variables
- `credentials.json` - Credential files

**High:**
- `/etc/passwd` - Password file
- `/etc/sudoers` - Sudo configuration
- `/root/` - Root directory
- `~/.config/gcloud/` - GCloud credentials

**Medium:**
- `/proc/` - Process information
- `/sys/` - System information

### 3. Detect SSRF Risks

For network operations (web_fetch, http_request, curl, wget):

Check if the URL targets:
- Private IP ranges (10.x, 172.16-31.x, 192.168.x)
- Localhost (127.0.0.1, ::1, localhost)
- Link-local addresses (169.254.x)
- file:// protocol

### 4. Identify Command Injection

Look for injection patterns:
- `; command` - Command separator
- `| command` - Pipe injection
- `&& command` - Logical AND injection
- `` `command` `` - Backtick substitution
- `$(command)` - Command substitution

### 5. Check Path Traversal

Detect path traversal attempts:
- `../` - Parent directory traversal
- `..\` - Windows path traversal

## Output Format

Return a structured JSON response:

```json
{
  "is_dangerous": true,
  "risk_level": "critical",
  "threats": [
    {
      "type": "dangerous_command",
      "description": "Recursive delete from root directory",
      "pattern": "rm -rf /",
      "risk_level": "critical",
      "location": "command argument"
    }
  ],
  "recommendations": [
    "Avoid using dangerous system commands",
    "Use explicit paths and parameters",
    "Consider safer alternatives"
  ],
  "metadata": {
    "tool_name": "bash",
    "threat_count": 1,
    "analysis_timestamp": "2024-03-11T10:30:00Z"
  }
}
```

### Risk Levels

- **critical**: Immediate system damage or data loss possible
- **high**: Significant security risk or privilege escalation
- **medium**: Potential security issue requiring review
- **low**: Minor concern, proceed with caution

## Decision Guidelines

Based on the analysis:

**If risk_level is "critical":**
- **Action**: BLOCK immediately
- **Reason**: Operation poses immediate threat to system integrity
- **No exceptions**: Critical threats should always be blocked

**If risk_level is "high":**
- **Action**: BLOCK by default
- **Exception**: Allow only with explicit user confirmation
- **Reason**: Significant security risk requires human review

**If risk_level is "medium":**
- **Action**: ALLOW with warning
- **Reason**: Potential issue but may be legitimate
- **Log**: Record for audit trail

**If risk_level is "low" or no threats:**
- **Action**: ALLOW
- **Reason**: Operation appears safe

## Examples

### Example 1: Critical Threat

**Input:**
```json
{
  "tool_name": "bash",
  "arguments": {"command": "rm -rf /"},
  "context": {}
}
```

**Analysis:**
- Pattern detected: `rm -rf /`
- Type: dangerous_command
- Risk: critical

**Output:**
```json
{
  "is_dangerous": true,
  "risk_level": "critical",
  "threats": [{
    "type": "dangerous_command",
    "description": "Recursive delete from root directory",
    "pattern": "rm -rf /",
    "risk_level": "critical"
  }],
  "recommendations": [
    "Use specific directory paths instead of root",
    "Add --interactive flag for confirmation"
  ]
}
```

**Decision:** BLOCK

### Example 2: Sensitive Path Access

**Input:**
```json
{
  "tool_name": "read",
  "arguments": {"file_path": "/etc/shadow"},
  "context": {}
}
```

**Analysis:**
- Path detected: `/etc/shadow`
- Type: sensitive_path_access
- Risk: critical

**Output:**
```json
{
  "is_dangerous": true,
  "risk_level": "critical",
  "threats": [{
    "type": "sensitive_path_access",
    "description": "Access to shadow password file",
    "pattern": "/etc/shadow",
    "risk_level": "critical"
  }],
  "recommendations": [
    "Avoid accessing system password files",
    "Use appropriate APIs for user management"
  ]
}
```

**Decision:** BLOCK

### Example 3: SSRF Risk

**Input:**
```json
{
  "tool_name": "web_fetch",
  "arguments": {"url": "http://127.0.0.1:8080/admin"},
  "context": {}
}
```

**Analysis:**
- URL targets: localhost
- Type: ssrf_risk
- Risk: high

**Output:**
```json
{
  "is_dangerous": true,
  "risk_level": "high",
  "threats": [{
    "type": "ssrf_risk",
    "description": "Attempt to access local service",
    "pattern": "127.0.0.1",
    "risk_level": "high"
  }],
  "recommendations": [
    "Validate and whitelist allowed domains",
    "Restrict access to internal services"
  ]
}
```

**Decision:** BLOCK (or require confirmation)

### Example 4: Safe Operation

**Input:**
```json
{
  "tool_name": "bash",
  "arguments": {"command": "ls -la /tmp/myapp"},
  "context": {"workspace": "/tmp/myapp"}
}
```

**Analysis:**
- No dangerous patterns detected
- Path is within workspace
- Risk: low

**Output:**
```json
{
  "is_dangerous": false,
  "risk_level": "low",
  "threats": [],
  "recommendations": ["Operation appears safe"],
  "metadata": {"tool_name": "bash", "threat_count": 0}
}
```

**Decision:** ALLOW

## Integration with AHP Server

When using this skill in an AHP Server agent:

1. **Receive pre_action event** from business agent
2. **Apply this skill** to analyze the tool call
3. **If critical threat detected**: Return immediate BLOCK decision
4. **Otherwise**: Combine skill analysis with LLM reasoning
5. **Make final decision** based on both analyses
6. **Return decision** to business agent

## Best Practices

- **Fast path for critical threats**: Block immediately without LLM analysis
- **Combine with LLM**: Use skill for detection, LLM for context understanding
- **Log all decisions**: Maintain audit trail for security review
- **Update patterns regularly**: Add new threat patterns as discovered
- **Context matters**: Consider workspace and user context in decisions
- **False positives**: Review and refine patterns to reduce false alarms

## Limitations

- Pattern-based detection may miss novel attacks
- Cannot understand complex intent without LLM analysis
- May produce false positives on legitimate operations
- Requires regular updates to threat pattern database

## See Also

- `sanitize-untrusted-output` - For post-action output sanitization
- AHP Protocol documentation
- Security best practices guide
