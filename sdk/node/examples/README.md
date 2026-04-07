# A3S Code Node.js SDK Examples

This directory contains TypeScript examples demonstrating the A3S Code SDK capabilities.

## Directory Structure

```
examples/
├── README.md           # This file
├── basic/              # Core API usage (Agent, Session, send/stream)
├── streaming/          # Event streaming and real-time monitoring
├── teams/              # Multi-agent team collaboration
├── orchestrator/       # Sub-agent spawning and control
├── skills/            # Skill system and tool restrictions
├── mcp/                # MCP (Model Context Protocol) integration
├── context/            # Context providers, BTW questions, RAG
├── git/                # Git operations and worktree support
├── search/             # Agentic search functionality
├── configs/           # Example configuration files (.hcl)
└── docs/              # Language guides (JavaScript, etc.)
```

## Quick Start

```bash
# Install dependencies
npm install

# Set environment variables
export OPENAI_API_KEY="your-api-key"
export OPENAI_BASE_URL="http://your-api-endpoint/v1/"

# Run an example
npx tsx basic/test_runtime_nesting.ts
```

## Categories

### basic/
Core SDK usage: Agent creation, session management, send/stream operations.

### streaming/
Real-time event streaming, agent monitoring, and task priority queue.

### teams/
Multi-agent teams with Lead/Worker/Reviewer roles and task boards.

### orchestrator/
Sub-agent spawning, pause/resume/cancel, and event monitoring.

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
Example `.hcl` configuration files for the SDK.

### docs/
Language-specific guides and documentation.
