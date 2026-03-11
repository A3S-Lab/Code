#!/usr/bin/env python3
"""
AHP Pre-Action Guard - Dangerous Operation Interceptor

This harness server intercepts tool calls before execution and blocks
dangerous operations based on security policies.

Features:
- Command injection detection
- Dangerous command patterns (rm -rf, dd, mkfs, etc.)
- File path validation (prevent access to sensitive paths)
- Network operation restrictions
- Rate limiting for expensive operations
"""

import json
import sys
import re
from typing import Dict, Any, List
from datetime import datetime, timedelta

# Dangerous command patterns
DANGEROUS_PATTERNS = [
    r"rm\s+-rf\s+/",           # Recursive delete from root
    r"dd\s+if=.*of=/dev/",     # Disk operations
    r"mkfs\.",                 # Format filesystem
    r":\(\)\{.*\}",            # Fork bomb
    r">\s*/dev/sd[a-z]",       # Write to disk device
    r"chmod\s+777",            # Overly permissive permissions
    r"curl.*\|\s*bash",        # Pipe to shell
    r"wget.*\|\s*sh",          # Pipe to shell
    r"eval\s*\(",              # Eval injection
    r"exec\s*\(",              # Exec injection
]

# Sensitive paths that should not be accessed
SENSITIVE_PATHS = [
    "/etc/passwd",
    "/etc/shadow",
    "/root/.ssh",
    "~/.ssh/id_rsa",
    "/proc/",
    "/sys/",
    "~/.aws/credentials",
    "~/.config/gcloud",
]

# Rate limiting: track tool usage
tool_usage_tracker: Dict[str, List[datetime]] = {}
RATE_LIMIT_WINDOW = timedelta(seconds=60)
RATE_LIMIT_MAX_CALLS = 10

def check_dangerous_command(command: str) -> tuple[bool, str]:
    """Check if command contains dangerous patterns"""
    for pattern in DANGEROUS_PATTERNS:
        if re.search(pattern, command, re.IGNORECASE):
            return True, f"Dangerous pattern detected: {pattern}"
    return False, ""

def check_sensitive_path(path: str) -> tuple[bool, str]:
    """Check if path accesses sensitive locations"""
    for sensitive in SENSITIVE_PATHS:
        if sensitive in path:
            return True, f"Access to sensitive path: {sensitive}"
    return False, ""

def check_rate_limit(tool_name: str) -> tuple[bool, str]:
    """Check if tool usage exceeds rate limit"""
    now = datetime.now()

    # Initialize tracker for this tool
    if tool_name not in tool_usage_tracker:
        tool_usage_tracker[tool_name] = []

    # Remove old entries outside the window
    tool_usage_tracker[tool_name] = [
        ts for ts in tool_usage_tracker[tool_name]
        if now - ts < RATE_LIMIT_WINDOW
    ]

    # Check if limit exceeded
    if len(tool_usage_tracker[tool_name]) >= RATE_LIMIT_MAX_CALLS:
        return True, f"Rate limit exceeded: {RATE_LIMIT_MAX_CALLS} calls per {RATE_LIMIT_WINDOW.seconds}s"

    # Record this call
    tool_usage_tracker[tool_name].append(now)
    return False, ""

def handle_handshake(params: Dict[str, Any]) -> Dict[str, Any]:
    """Handle handshake request"""
    return {
        "protocol_version": "2.0",
        "harness_info": {
            "name": "pre-action-guard",
            "version": "1.0.0",
            "capabilities": ["pre_action", "query"],
            "description": "Dangerous operation interceptor for tool calls"
        },
        "config": {
            "timeout_ms": 5000,
            "batch_size": 50
        }
    }

def handle_pre_action(event: Dict[str, Any]) -> Dict[str, Any]:
    """Handle pre-action event - intercept dangerous operations"""
    payload = event.get("payload", {})
    tool_name = payload.get("tool", "")
    arguments = payload.get("arguments", {})

    log(f"Checking tool: {tool_name}")

    # Check rate limiting
    is_limited, limit_reason = check_rate_limit(tool_name)
    if is_limited:
        log(f"BLOCKED: {limit_reason}")
        return {
            "decision": "block",
            "reason": limit_reason,
            "metadata": {
                "blocked_by": "rate_limiter",
                "tool": tool_name
            }
        }

    # Check Bash tool for dangerous commands
    if tool_name.lower() == "bash":
        command = arguments.get("command", "")
        is_dangerous, danger_reason = check_dangerous_command(command)
        if is_dangerous:
            log(f"BLOCKED: {danger_reason}")
            return {
                "decision": "block",
                "reason": f"Dangerous command blocked: {danger_reason}",
                "metadata": {
                    "blocked_by": "command_pattern_matcher",
                    "command": command[:100]  # Truncate for logging
                }
            }

    # Check Read/Write tools for sensitive paths
    if tool_name.lower() in ["read", "write", "edit"]:
        file_path = arguments.get("file_path", "") or arguments.get("path", "")
        is_sensitive, path_reason = check_sensitive_path(file_path)
        if is_sensitive:
            log(f"BLOCKED: {path_reason}")
            return {
                "decision": "block",
                "reason": f"Sensitive path access blocked: {path_reason}",
                "metadata": {
                    "blocked_by": "path_validator",
                    "path": file_path
                }
            }

    # Check web_fetch for suspicious URLs
    if tool_name.lower() == "web_fetch":
        url = arguments.get("url", "")
        # Block localhost/internal IPs (SSRF prevention)
        if any(pattern in url.lower() for pattern in ["localhost", "127.0.0.1", "0.0.0.0", "169.254"]):
            log(f"BLOCKED: SSRF attempt detected")
            return {
                "decision": "block",
                "reason": "SSRF attempt: Cannot fetch from internal/localhost URLs",
                "metadata": {
                    "blocked_by": "ssrf_detector",
                    "url": url
                }
            }

    # All checks passed
    log(f"ALLOWED: {tool_name}")
    return {
        "decision": "allow",
        "metadata": {
            "checked_by": "pre_action_guard",
            "timestamp": datetime.now().isoformat()
        }
    }

def handle_query(query: Dict[str, Any]) -> Dict[str, Any]:
    """Handle query request"""
    query_type = query.get("query_type", "")
    payload = query.get("payload", {})

    if query_type == "is_safe_command":
        command = payload.get("command", "")
        is_dangerous, reason = check_dangerous_command(command)
        return {
            "answer": not is_dangerous,
            "reason": reason if is_dangerous else "Command appears safe",
            "alternatives": ["Review command manually", "Run in sandbox"] if is_dangerous else None
        }

    return {
        "answer": True,
        "reason": "No policy applies to this query"
    }

def log(message: str):
    """Log to stderr"""
    sys.stderr.write(f"[PRE-ACTION-GUARD] {message}\n")
    sys.stderr.flush()

def main():
    """Main event loop"""
    log("Pre-Action Guard started")

    for line in sys.stdin:
        try:
            msg = json.loads(line.strip())
            req_id = msg.get("id")
            method = msg.get("method", "")
            params = msg.get("params", {})

            # Handle request (blocking)
            if req_id:
                if method == "ahp/handshake":
                    result = handle_handshake(params)
                elif method == "ahp/event":
                    event_type = params.get("event_type")
                    if event_type == "pre_action":
                        result = handle_pre_action(params)
                    else:
                        result = {"decision": "allow"}
                elif method == "ahp/query":
                    result = handle_query(params)
                else:
                    result = {"error": f"Unknown method: {method}"}

                response = {
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "result": result
                }
                print(json.dumps(response), flush=True)

            # Handle notification (fire-and-forget)
            else:
                event_type = params.get("event_type")
                log(f"Notification: {event_type}")

        except Exception as e:
            log(f"ERROR: {e}")
            if req_id:
                error_response = {
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "error": {
                        "code": -32603,
                        "message": str(e)
                    }
                }
                print(json.dumps(error_response), flush=True)

if __name__ == "__main__":
    main()
