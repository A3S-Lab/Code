#!/usr/bin/env python3
"""
AHP Pre-Action Guard - Dangerous Operation Interceptor

This AHP harness server uses a3s-code to implement intelligent dangerous
operation detection and interception.

Features:
- Command injection detection using pattern matching
- Dangerous command patterns (rm -rf, dd, mkfs, etc.)
- File path validation (prevent access to sensitive paths)
- Network operation restrictions (SSRF prevention)
- Rate limiting for expensive operations
- Intelligent context-aware analysis
"""

import json
import sys
import re
from typing import Dict, Any, List, Optional
from datetime import datetime, timedelta
from collections import defaultdict

# Dangerous command patterns
DANGEROUS_PATTERNS = [
    (r"rm\s+-rf\s+/", "Recursive delete from root"),
    (r"dd\s+if=.*of=/dev/", "Disk operations"),
    (r"mkfs\.", "Format filesystem"),
    (r":\(\)\{.*\}", "Fork bomb"),
    (r">\s*/dev/sd[a-z]", "Write to disk device"),
    (r"chmod\s+777", "Overly permissive permissions"),
    (r"curl.*\|\s*bash", "Pipe to shell"),
    (r"wget.*\|\s*sh", "Pipe to shell"),
    (r"eval\s*\(", "Eval injection"),
    (r"exec\s*\(", "Exec injection"),
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

# Rate limiting configuration
RATE_LIMIT_WINDOW = timedelta(seconds=60)
RATE_LIMIT_MAX_CALLS = 10

class RateLimiter:
    """Rate limiter for tool calls"""

    def __init__(self):
        self.tool_usage: Dict[str, List[datetime]] = defaultdict(list)

    def check_rate_limit(self, tool_name: str) -> tuple[bool, Optional[str]]:
        """Check if tool usage exceeds rate limit"""
        now = datetime.now()

        # Remove old entries outside the window
        self.tool_usage[tool_name] = [
            ts for ts in self.tool_usage[tool_name]
            if now - ts < RATE_LIMIT_WINDOW
        ]

        # Check if limit exceeded
        if len(self.tool_usage[tool_name]) >= RATE_LIMIT_MAX_CALLS:
            return True, f"Rate limit exceeded: {RATE_LIMIT_MAX_CALLS} calls per {RATE_LIMIT_WINDOW.seconds}s"

        # Record this call
        self.tool_usage[tool_name].append(now)
        return False, None

class PreActionGuard:
    """Pre-Action Guard - Intelligent dangerous operation detector"""

    def __init__(self):
        self.rate_limiter = RateLimiter()
        self.compiled_patterns = [
            (re.compile(pattern, re.IGNORECASE), desc)
            for pattern, desc in DANGEROUS_PATTERNS
        ]

    def check_dangerous_command(self, command: str) -> Optional[str]:
        """Check if command contains dangerous patterns"""
        for pattern, description in self.compiled_patterns:
            if pattern.search(command):
                return description
        return None

    def check_sensitive_path(self, path: str) -> Optional[str]:
        """Check if path accesses sensitive locations"""
        for sensitive in SENSITIVE_PATHS:
            if sensitive in path:
                return f"Access to sensitive path: {sensitive}"
        return None

    def check_ssrf(self, url: str) -> Optional[str]:
        """Check for SSRF attempts"""
        url_lower = url.lower()
        ssrf_patterns = ["localhost", "127.0.0.1", "0.0.0.0", "169.254"]

        for pattern in ssrf_patterns:
            if pattern in url_lower:
                return "SSRF attempt: Cannot fetch from internal/localhost URLs"
        return None

    def handle_handshake(self, params: Dict[str, Any]) -> Dict[str, Any]:
        """Handle handshake request"""
        return {
            "protocol_version": "2.0",
            "harness_info": {
                "name": "pre-action-guard",
                "version": "1.0.0",
                "capabilities": ["pre_action", "query"],
                "description": "Intelligent dangerous operation interceptor"
            },
            "config": {
                "timeout_ms": 5000,
                "batch_size": 50
            }
        }

    def handle_pre_action(self, event: Dict[str, Any]) -> Dict[str, Any]:
        """Handle pre-action event - intercept dangerous operations"""
        payload = event.get("payload", {})
        tool_name = payload.get("tool", "")
        arguments = payload.get("arguments", {})

        log(f"Checking tool: {tool_name}")

        # Check rate limiting
        is_limited, limit_reason = self.rate_limiter.check_rate_limit(tool_name)
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
            danger_reason = self.check_dangerous_command(command)
            if danger_reason:
                log(f"BLOCKED: {danger_reason}")
                return {
                    "decision": "block",
                    "reason": f"Dangerous command blocked: {danger_reason}",
                    "metadata": {
                        "blocked_by": "command_pattern_matcher",
                        "command": command[:100],  # Truncate for logging
                        "danger_type": danger_reason
                    }
                }

        # Check Read/Write tools for sensitive paths
        if tool_name.lower() in ["read", "write", "edit"]:
            file_path = arguments.get("file_path", "") or arguments.get("path", "")
            path_reason = self.check_sensitive_path(file_path)
            if path_reason:
                log(f"BLOCKED: {path_reason}")
                return {
                    "decision": "block",
                    "reason": f"Sensitive path access blocked: {path_reason}",
                    "metadata": {
                        "blocked_by": "path_validator",
                        "path": file_path
                    }
                }

        # Check web_fetch for SSRF
        if tool_name.lower() == "web_fetch":
            url = arguments.get("url", "")
            ssrf_reason = self.check_ssrf(url)
            if ssrf_reason:
                log(f"BLOCKED: {ssrf_reason}")
                return {
                    "decision": "block",
                    "reason": ssrf_reason,
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

    def handle_query(self, query: Dict[str, Any]) -> Dict[str, Any]:
        """Handle query request"""
        query_type = query.get("query_type", "")
        payload = query.get("payload", {})

        if query_type == "is_safe_command":
            command = payload.get("command", "")
            danger_reason = self.check_dangerous_command(command)
            return {
                "answer": danger_reason is None,
                "reason": danger_reason if danger_reason else "Command appears safe",
                "alternatives": ["Review command manually", "Run in sandbox"] if danger_reason else None
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
    log("Pre-Action Guard started (Python implementation)")

    guard = PreActionGuard()

    for line in sys.stdin:
        try:
            msg = json.loads(line.strip())
            req_id = msg.get("id")
            method = msg.get("method", "")
            params = msg.get("params", {})

            # Handle request (blocking)
            if req_id:
                if method == "ahp/handshake":
                    result = guard.handle_handshake(params)
                elif method == "ahp/event":
                    event_type = params.get("event_type")
                    if event_type == "pre_action":
                        result = guard.handle_pre_action(params)
                    else:
                        result = {"decision": "allow"}
                elif method == "ahp/query":
                    result = guard.handle_query(params)
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
