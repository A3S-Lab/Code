# A3S Code Node.js SDK Examples

This directory contains TypeScript examples demonstrating the A3S Code SDK capabilities.

## Directory Structure

```
examples/
├── README.md           # This file
├── basic/              # Core API usage (Agent, Session, send/stream)
├── streaming/          # Event streaming and optional queue experiments
├── skills/            # Skill system and tool restrictions
├── mcp/                # MCP (Model Context Protocol) integration
├── context/            # Context providers and RAG
├── git/                # Git operations and worktree support
├── search/             # Search configuration examples
├── configs/           # Example configuration files (.acl)
└── docs/              # Language guides (JavaScript, etc.)
```

## Quick Start

```bash
# Install dependencies
npm install

# Smoke-check examples without requiring live API credentials
npm run smoke

# Run a live-provider example
OPENAI_API_KEY="your-api-key" OPENAI_BASE_URL="http://your-endpoint/v1/" npm run basic:minimax
```

## Categories

### basic/
Core SDK usage: Agent creation, session management, send/stream operations.

### streaming/
Real-time event streaming, monitoring, HITL confirmation loops, and optional
lane-queue experiments. The default session path is queue-free.

### skills/
Custom skills, prompt slots, and tool restrictions.

### mcp/
MCP server integration and external tool access.

### context/
Context providers and RAG retrieval.

### git/
Git worktree isolation and git operations.

### search/
Search configuration examples.

### configs/
Example `.acl` configuration files for the SDK.

### docs/
Language-specific guides and documentation.
