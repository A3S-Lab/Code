# A3S Code Node.js SDK Examples

This directory contains TypeScript examples demonstrating the A3S Code SDK capabilities.

## Directory Structure

```
examples/
├── README.md           # This file
├── basic/              # Core API usage (Agent, Session, send/stream)
├── streaming/          # Event streaming and optional queue experiments
├── orchestrator/       # Advanced SubAgent lifecycle control
├── skills/            # Skill system and tool restrictions
├── mcp/                # MCP (Model Context Protocol) integration
├── context/            # Context providers, BTW questions, RAG
├── git/                # Git operations and worktree support
├── search/             # Agentic search functionality
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
Real-time event streaming, monitoring, and optional lane-queue experiments.
The default session path is queue-free.

### orchestrator/
Advanced SubAgent spawning, pause/resume/cancel, and event monitoring.
This is a control plane, not the default multi-agent composition path.

### skills/
Custom skills, prompt slots, and tool restrictions.

### mcp/
MCP server integration and external tool access.

### context/
Context providers, ephemeral BTW questions, and RAG retrieval.

### git/
Git worktree isolation and git operations.

### search/
Agentic search with locators and sampled lines.

### configs/
Example `.acl` configuration files for the SDK.

### docs/
Language-specific guides and documentation.
