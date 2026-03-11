#!/usr/bin/env python3
"""
AHP Intelligent Guard - LLM-Powered Safety Harness

This is a true a3s-code based AHP server that uses LLM intelligence
to analyze and intercept dangerous operations.

Features:
- Uses a3s-code Agent with LLM for intelligent analysis
- Implements AHP protocol (JSON-RPC 2.0) over stdio
- Configurable safety skills as tools
- Context-aware dangerous operation detection
- Adaptive learning from previous decisions

Usage:
    export MOONSHOT_API_KEY=your_api_key
    python ahp_intelligent_guard.py

Architecture:
    Main Agent (AHP Server)
    ├── Safety Analysis Skill (tool)
    ├── Pattern Matching Skill (tool)
    └── Decision Making (LLM reasoning)
"""

import asyncio
import json
import sys
import os
from typing import Dict, Any, Optional
from pathlib import Path
from a3s_code import Agent

# Safety analysis prompt template
SAFETY_ANALYSIS_PROMPT = """You are a security expert analyzing tool calls for potential dangers.

Analyze this tool call and determine if it should be allowed:

Tool: {tool_name}
Arguments: {arguments}
Context: {context}

Consider:
1. Command injection risks (rm -rf, dd, mkfs, fork bombs)
2. Sensitive file access (/etc/passwd, ~/.ssh, credentials)
3. Network security (SSRF, unauthorized connections)
4. Privilege escalation attempts
5. Data exfiltration risks

Respond in JSON format:
{{
    "allowed": true/false,
    "reason": "explanation",
    "severity": "low/medium/high/critical",
    "suggestions": ["alternative approach if blocked"]
}}
"""

# Pattern matching rules (fallback when LLM is unavailable)
DANGEROUS_PATTERNS = [
    (r"rm\s+-rf\s+/", "Recursive delete from root", "critical"),
    (r"dd\s+if=.*of=/dev/", "Disk operations", "critical"),
    (r"mkfs\.", "Format filesystem", "critical"),
    (r":\(\)\{.*\}", "Fork bomb", "critical"),
    (r"chmod\s+777", "Overly permissive permissions", "high"),
    (r"curl.*\|\s*bash", "Pipe to shell", "high"),
]

SENSITIVE_PATHS = [
    "/etc/passwd", "/etc/shadow", "/root/.ssh", "~/.ssh/id_rsa",
    "~/.aws/credentials", "~/.config/gcloud",
]


class AHPIntelligentGuard:
    """AHP server powered by a3s-code Agent for intelligent safety analysis."""

    def __init__(self, config_path: Optional[str] = None):
        """Initialize the AHP server with a3s-code Agent."""
        self.config_path = config_path or self.find_config()
        self.agent = Agent.create(self.config_path)
        self.session = None
        self.analysis_history = []

    @staticmethod
    def find_config() -> str:
        """Find a3s config file."""
        if env := os.environ.get("A3S_CONFIG"):
            return env

        home_config = Path.home() / ".a3s" / "config.hcl"
        if home_config.exists():
            return str(home_config)

        raise FileNotFoundError(
            "Config not found. Create ~/.a3s/config.hcl or set A3S_CONFIG"
        )

    async def initialize(self):
        """Initialize the agent session."""
        import tempfile
        workspace = tempfile.mkdtemp(prefix="ahp_guard_")
        self.session = self.agent.session(
            workspace,
            permissive=True,
            builtin_skills=False,
        )
        self.log(f"Initialized with workspace: {workspace}")

    def log(self, message: str):
        """Log to stderr (stdout is for JSON-RPC)."""
        print(f"[AHP Guard] {message}", file=sys.stderr, flush=True)

    async def analyze_with_llm(
        self, tool_name: str, arguments: Dict[str, Any], context: Dict[str, Any]
    ) -> Dict[str, Any]:
        """Use LLM to analyze the tool call for safety."""
        prompt = SAFETY_ANALYSIS_PROMPT.format(
            tool_name=tool_name,
            arguments=json.dumps(arguments, indent=2),
            context=json.dumps(context, indent=2),
        )

        try:
            result = self.session.send(prompt)
            response_text = result.text.strip()

            # Try to extract JSON from the response
            if "{" in response_text and "}" in response_text:
                start = response_text.index("{")
                end = response_text.rindex("}") + 1
                json_str = response_text[start:end]
                analysis = json.loads(json_str)

                # Store in history for learning
                self.analysis_history.append({
                    "tool": tool_name,
                    "analysis": analysis,
                })

                return analysis
            else:
                # Fallback: parse text response
                allowed = "allow" in response_text.lower() and "not" not in response_text.lower()
                return {
                    "allowed": allowed,
                    "reason": response_text[:200],
                    "severity": "medium",
                    "suggestions": [],
                }
        except Exception as e:
            self.log(f"LLM analysis failed: {e}")
            # Fallback to pattern matching
            return self.analyze_with_patterns(tool_name, arguments)

    def analyze_with_patterns(
        self, tool_name: str, arguments: Dict[str, Any]
    ) -> Dict[str, Any]:
        """Fallback pattern-based analysis."""
        import re

        # Check command arguments
        command_str = json.dumps(arguments)

        for pattern, description, severity in DANGEROUS_PATTERNS:
            if re.search(pattern, command_str, re.IGNORECASE):
                return {
                    "allowed": False,
                    "reason": f"Dangerous pattern detected: {description}",
                    "severity": severity,
                    "suggestions": ["Review the command and use safer alternatives"],
                }

        # Check sensitive paths
        for path in SENSITIVE_PATHS:
            if path in command_str:
                return {
                    "allowed": False,
                    "reason": f"Access to sensitive path: {path}",
                    "severity": "high",
                    "suggestions": ["Avoid accessing sensitive system files"],
                }

        return {
            "allowed": True,
            "reason": "No dangerous patterns detected",
            "severity": "low",
            "suggestions": [],
        }

    async def handle_handshake(self, params: Dict[str, Any]) -> Dict[str, Any]:
        """Handle AHP handshake."""
        self.log(f"Handshake from client: {params.get('client_name', 'unknown')}")
        return {
            "server_name": "ahp-intelligent-guard",
            "server_version": "1.0.0",
            "protocol_version": "2.0",
            "capabilities": {
                "pre_action": True,
                "post_action": False,
            },
        }

    async def handle_pre_action(self, params: Dict[str, Any]) -> Dict[str, Any]:
        """Handle pre-action event (before tool execution)."""
        event_id = params.get("event_id")
        tool_name = params.get("tool_name")
        arguments = params.get("arguments", {})
        context = params.get("context", {})

        self.log(f"Pre-action: {tool_name} (event {event_id})")

        # Use LLM for intelligent analysis
        analysis = await self.analyze_with_llm(tool_name, arguments, context)

        if not analysis["allowed"]:
            self.log(f"BLOCKED: {analysis['reason']} (severity: {analysis['severity']})")
            return {
                "action": "block",
                "reason": analysis["reason"],
                "metadata": {
                    "severity": analysis["severity"],
                    "suggestions": analysis["suggestions"],
                },
            }
        else:
            self.log(f"ALLOWED: {analysis['reason']}")
            return {
                "action": "allow",
                "metadata": {
                    "analysis": analysis["reason"],
                },
            }

    async def handle_request(self, request: Dict[str, Any]) -> Dict[str, Any]:
        """Handle a JSON-RPC request."""
        method = request.get("method")
        params = request.get("params", {})
        request_id = request.get("id")

        try:
            if method == "handshake":
                result = await self.handle_handshake(params)
            elif method == "pre_action":
                result = await self.handle_pre_action(params)
            else:
                raise ValueError(f"Unknown method: {method}")

            return {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": result,
            }
        except Exception as e:
            self.log(f"Error handling {method}: {e}")
            return {
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {
                    "code": -32603,
                    "message": str(e),
                },
            }

    async def run(self):
        """Main event loop - read from stdin, write to stdout."""
        await self.initialize()
        self.log("AHP Intelligent Guard started (powered by a3s-code)")
        self.log("Waiting for requests on stdin...")

        loop = asyncio.get_event_loop()

        while True:
            try:
                # Read line from stdin
                line = await loop.run_in_executor(None, sys.stdin.readline)
                if not line:
                    break

                line = line.strip()
                if not line:
                    continue

                # Parse JSON-RPC request
                request = json.loads(line)

                # Handle request
                response = await self.handle_request(request)

                # Write response to stdout
                print(json.dumps(response), flush=True)

            except json.JSONDecodeError as e:
                self.log(f"Invalid JSON: {e}")
            except KeyboardInterrupt:
                self.log("Shutting down...")
                break
            except Exception as e:
                self.log(f"Unexpected error: {e}")


async def main():
    """Entry point."""
    guard = AHPIntelligentGuard()
    await guard.run()


if __name__ == "__main__":
    asyncio.run(main())
