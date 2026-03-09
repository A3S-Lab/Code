"""
Example: AGENTS.md Auto-Loading

Demonstrates automatic loading of AGENTS.md from workspace root.
Similar to Claude Code's CLAUDE.md mechanism.
"""

from a3s_code import Agent
import tempfile
import os

def main():
    print("🚀 AGENTS.md Auto-Loading Example\n")

    # Create a temporary workspace with AGENTS.md
    with tempfile.TemporaryDirectory(prefix="agents-md-test-") as tmpdir:
        print(f"📁 Workspace: {tmpdir}\n")

        # Write AGENTS.md with project-specific instructions
        agents_md_content = """# Project Instructions

This is a Python project using FastAPI and SQLAlchemy.

## Code Style
- Use type hints everywhere
- Follow PEP 8
- Use Black for formatting
- Use mypy for type checking

## Architecture
- Follow Clean Architecture
- Use dependency injection
- Keep routes thin
- Business logic in services

## Testing
- Use pytest for all tests
- Unit tests for all services
- Integration tests for API endpoints
- Minimum 80% code coverage

## Security
- Validate all user input with Pydantic
- Use parameterized queries
- Never log sensitive data
- Follow OWASP Top 10 guidelines
"""

        agents_md_path = os.path.join(tmpdir, "AGENTS.md")
        with open(agents_md_path, "w") as f:
            f.write(agents_md_content)

        print("✅ Created AGENTS.md in workspace\n")

        # Create agent and session
        agent = Agent.create("agent.hcl")
        session = agent.session(tmpdir, builtin_skills=True)

        print("📝 Sending prompt to agent...\n")

        # Send a prompt - the agent should follow AGENTS.md instructions
        result = session.send(
            "Create a new user registration endpoint with validation and tests"
        )

        print("✅ Agent response:\n")
        print(result.text)
        print(f"\n📊 Stats: {result.tool_calls_count} tools, {result.total_tokens} tokens")

        print("\n🧹 Workspace will be cleaned up automatically")

if __name__ == "__main__":
    main()
