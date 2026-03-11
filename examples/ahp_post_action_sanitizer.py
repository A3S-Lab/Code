#!/usr/bin/env python3
"""
AHP Post-Action Sanitizer - Untrusted Environment Information Sanitizer

This AHP harness server uses a3s-code to implement intelligent output
sanitization and injection protection.

Features:
- Prompt injection detection (ignore previous instructions, etc.)
- PII redaction (API keys, passwords, emails, credit cards, SSNs, JWTs)
- Malicious payload detection (XSS, eval, exec, base64 payloads)
- Output size limiting (100KB max)
- Suspicious pattern detection
- Context-aware sanitization
"""

import json
import sys
import re
from typing import Dict, Any, List, Optional, Tuple
from datetime import datetime

# Prompt injection patterns
INJECTION_PATTERNS = [
    (r"ignore\s+(all\s+)?previous\s+instructions", "Ignore previous instructions"),
    (r"disregard\s+(all\s+)?prior\s+instructions", "Disregard prior instructions"),
    (r"forget\s+(all\s+)?previous\s+(instructions|context)", "Forget previous context"),
    (r"new\s+instructions?:", "New instructions"),
    (r"system\s*:\s*you\s+are", "System prompt override"),
    (r"<\|im_start\|>", "Chat template injection"),
    (r"<\|im_end\|>", "Chat template injection"),
    (r"\[INST\]", "Instruction template injection"),
    (r"\[/INST\]", "Instruction template injection"),
    (r"###\s*Instruction:", "Instruction header injection"),
]

# PII patterns
PII_PATTERNS = [
    ("api_key", r"(api[_-]?key|apikey|access[_-]?token)\s*[:=]\s*['\"]?([a-zA-Z0-9_\-]{20,})['\"]?"),
    ("password", r"(password|passwd|pwd)\s*[:=]\s*['\"]?([^\s'\"]{8,})['\"]?"),
    ("email", r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b"),
    ("credit_card", r"\b\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}\b"),
    ("ssn", r"\b\d{3}-\d{2}-\d{4}\b"),
    ("jwt", r"eyJ[a-zA-Z0-9_-]+\.eyJ[a-zA-Z0-9_-]+\.[a-zA-Z0-9_-]+"),
]

# Malicious payload patterns
MALICIOUS_PATTERNS = [
    (r"<script[^>]*>.*?</script>", "XSS script tag"),
    (r"javascript:", "JavaScript protocol"),
    (r"data:text/html", "Data URI XSS"),
    (r"eval\s*\(", "Eval injection"),
    (r"exec\s*\(", "Exec injection"),
    (r"__import__\s*\(", "Python import injection"),
    (r"base64\s*\.", "Base64 encoded payload"),
]

# Maximum output size (characters)
MAX_OUTPUT_SIZE = 100_000

class PostActionSanitizer:
    """Post-Action Sanitizer - Intelligent output sanitization"""

    def __init__(self):
        self.injection_patterns = [
            (re.compile(pattern, re.IGNORECASE), desc)
            for pattern, desc in INJECTION_PATTERNS
        ]
        self.pii_patterns = [
            (name, re.compile(pattern, re.IGNORECASE))
            for name, pattern in PII_PATTERNS
        ]
        self.malicious_patterns = [
            (re.compile(pattern, re.IGNORECASE | re.DOTALL), desc)
            for pattern, desc in MALICIOUS_PATTERNS
        ]

    def detect_injection(self, text: str) -> List[str]:
        """Detect prompt injection attempts"""
        detected = []
        for pattern, description in self.injection_patterns:
            if pattern.search(text):
                detected.append(description)
        return detected

    def redact_pii(self, text: str) -> Tuple[str, List[str]]:
        """Redact PII from text"""
        redacted = text
        found_types = []

        for pii_type, pattern in self.pii_patterns:
            if pattern.search(redacted):
                found_types.append(pii_type)
                redacted = pattern.sub(f"[REDACTED_{pii_type.upper()}]", redacted)

        return redacted, found_types

    def detect_malicious_payload(self, text: str) -> List[str]:
        """Detect malicious payloads"""
        detected = []
        for pattern, description in self.malicious_patterns:
            if pattern.search(text):
                detected.append(description)
        return detected

    def check_output_size(self, text: str) -> Tuple[bool, int]:
        """Check if output exceeds size limit"""
        size = len(text)
        return size > MAX_OUTPUT_SIZE, size

    def sanitize_output(self, output: str, tool_name: str) -> Dict[str, Any]:
        """
        Sanitize tool output and return decision

        Returns:
            - allow: output is safe (possibly with modifications)
            - block: output contains dangerous content
            - modify: output has been sanitized
        """
        # Check output size
        is_oversized, size = self.check_output_size(output)
        if is_oversized:
            log(f"Output size {size} exceeds limit {MAX_OUTPUT_SIZE}")
            truncated = output[:MAX_OUTPUT_SIZE] + f"\n\n[TRUNCATED: Output exceeded {MAX_OUTPUT_SIZE} characters]"
            return {
                "decision": "modify",
                "modified_payload": {
                    "result": {
                        "output": truncated,
                        "success": True,
                    }
                },
                "metadata": {
                    "sanitized_by": "size_limiter",
                    "original_size": size,
                    "truncated_size": MAX_OUTPUT_SIZE
                }
            }

        # Detect prompt injection
        injection_patterns = self.detect_injection(output)
        if injection_patterns:
            log(f"INJECTION DETECTED: {injection_patterns}")
            return {
                "decision": "block",
                "reason": f"Prompt injection detected in tool output. Patterns: {', '.join(injection_patterns[:3])}",
                "metadata": {
                    "blocked_by": "injection_detector",
                    "tool": tool_name,
                    "patterns": injection_patterns
                }
            }

        # Detect malicious payloads
        malicious_patterns = self.detect_malicious_payload(output)
        if malicious_patterns:
            log(f"MALICIOUS PAYLOAD DETECTED: {malicious_patterns}")
            return {
                "decision": "block",
                "reason": "Malicious payload detected in tool output",
                "metadata": {
                    "blocked_by": "payload_detector",
                    "tool": tool_name,
                    "patterns": malicious_patterns[:3]
                }
            }

        # Redact PII
        sanitized_output, found_pii = self.redact_pii(output)
        if found_pii:
            log(f"PII REDACTED: {found_pii}")
            return {
                "decision": "modify",
                "modified_payload": {
                    "result": {
                        "output": sanitized_output,
                        "success": True,
                    }
                },
                "metadata": {
                    "sanitized_by": "pii_redactor",
                    "redacted_types": found_pii,
                    "redaction_count": len(found_pii)
                }
            }

        # Output is clean
        return {
            "decision": "allow",
            "metadata": {
                "checked_by": "post_action_sanitizer",
                "timestamp": datetime.now().isoformat()
            }
        }

    def handle_handshake(self, params: Dict[str, Any]) -> Dict[str, Any]:
        """Handle handshake request"""
        return {
            "protocol_version": "2.0",
            "harness_info": {
                "name": "post-action-sanitizer",
                "version": "1.0.0",
                "capabilities": ["post_action", "query"],
                "description": "Intelligent output sanitizer and injection protector"
            },
            "config": {
                "timeout_ms": 5000,
                "batch_size": 50
            }
        }

    def handle_post_action(self, event: Dict[str, Any]) -> Dict[str, Any]:
        """Handle post-action event - sanitize tool outputs"""
        payload = event.get("payload", {})
        tool_name = payload.get("tool", "")
        result = payload.get("result", {})
        output = result.get("output", "")

        log(f"Sanitizing output from: {tool_name}")

        # Only sanitize if there's output
        if not output or not isinstance(output, str):
            return {
                "decision": "allow",
                "metadata": {"checked_by": "post_action_sanitizer"}
            }

        # Sanitize the output
        return self.sanitize_output(output, tool_name)

    def handle_query(self, query: Dict[str, Any]) -> Dict[str, Any]:
        """Handle query request"""
        query_type = query.get("query_type", "")
        payload = query.get("payload", {})

        if query_type == "check_injection":
            text = payload.get("text", "")
            patterns = self.detect_injection(text)
            has_injection = len(patterns) > 0
            return {
                "answer": not has_injection,
                "reason": f"Injection patterns found: {patterns}" if has_injection else "No injection detected",
                "alternatives": ["Sanitize text", "Block output"] if has_injection else None
            }

        if query_type == "redact_pii":
            text = payload.get("text", "")
            sanitized, found = self.redact_pii(text)
            return {
                "answer": sanitized,
                "reason": f"Redacted {len(found)} PII instances: {found}" if found else "No PII found",
            }

        return {
            "answer": True,
            "reason": "No policy applies to this query"
        }

def log(message: str):
    """Log to stderr"""
    sys.stderr.write(f"[POST-ACTION-SANITIZER] {message}\n")
    sys.stderr.flush()

def main():
    """Main event loop"""
    log("Post-Action Sanitizer started (Python implementation)")

    sanitizer = PostActionSanitizer()

    for line in sys.stdin:
        try:
            msg = json.loads(line.strip())
            req_id = msg.get("id")
            method = msg.get("method", "")
            params = msg.get("params", {})

            # Handle request (blocking)
            if req_id:
                if method == "ahp/handshake":
                    result = sanitizer.handle_handshake(params)
                elif method == "ahp/event":
                    event_type = params.get("event_type")
                    if event_type == "post_action":
                        result = sanitizer.handle_post_action(params)
                    else:
                        result = {"decision": "allow"}
                elif method == "ahp/query":
                    result = sanitizer.handle_query(params)
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
