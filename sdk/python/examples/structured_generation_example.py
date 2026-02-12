"""
Structured Generation Example

Demonstrates how to generate structured output with JSON Schema:
- Unary structured generation
- Streaming structured generation
- Using schemas for type-safe LLM output
"""

import asyncio
import json
from a3s_code import A3sClient


async def structured_generation_example():
    print("=" * 60)
    print("Structured Generation Example")
    print("=" * 60)
    print()

    async with A3sClient(address="localhost:4088") as client:
        # Create a session
        print("1. Creating session...")
        session = await client.create_session(
            name="structured-demo",
            workspace="/tmp/structured-test",
            system_prompt="You are a helpful assistant that returns structured data.",
            llm={
                "provider": "anthropic",
                "model": "claude-sonnet-4-20250514",
            },
        )
        session_id = session["session_id"]
        print(f"✓ Session created: {session_id}")
        print()

        # =====================================================================
        # Example 1: Extract structured data from text
        # =====================================================================
        print("2. Extracting structured data from text...")

        person_schema = json.dumps({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Full name"},
                "age": {"type": "integer", "description": "Age in years"},
                "email": {"type": "string", "format": "email"},
                "skills": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "List of technical skills",
                },
            },
            "required": ["name", "age", "email", "skills"],
        })

        response = await client.generate_structured(
            session_id=session_id,
            messages=[
                {
                    "role": "ROLE_USER",
                    "content": (
                        "Extract the person's info: John Smith is a 32-year-old "
                        "developer at john@example.com who knows Python, Rust, and TypeScript."
                    ),
                }
            ],
            schema=person_schema,
        )

        print("✓ Structured response:")
        if response.get("data"):
            data = json.loads(response["data"])
            print(f"  Name: {data.get('name')}")
            print(f"  Age: {data.get('age')}")
            print(f"  Email: {data.get('email')}")
            print(f"  Skills: {', '.join(data.get('skills', []))}")
        print()

        # =====================================================================
        # Example 2: Generate a list of items
        # =====================================================================
        print("3. Generating a structured list...")

        task_schema = json.dumps({
            "type": "object",
            "properties": {
                "tasks": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": {"type": "string"},
                            "priority": {
                                "type": "string",
                                "enum": ["high", "medium", "low"],
                            },
                            "estimated_hours": {"type": "number"},
                        },
                        "required": ["title", "priority", "estimated_hours"],
                    },
                },
                "total_hours": {"type": "number"},
            },
            "required": ["tasks", "total_hours"],
        })

        response = await client.generate_structured(
            session_id=session_id,
            messages=[
                {
                    "role": "ROLE_USER",
                    "content": "Create a task list for building a REST API with authentication.",
                }
            ],
            schema=task_schema,
        )

        print("✓ Task list:")
        if response.get("data"):
            data = json.loads(response["data"])
            for task in data.get("tasks", []):
                print(f"  [{task['priority'].upper()}] {task['title']} ({task['estimated_hours']}h)")
            print(f"  Total: {data.get('total_hours')}h")
        print()

        # =====================================================================
        # Example 3: Streaming structured generation
        # =====================================================================
        print("4. Streaming structured generation...")

        review_schema = json.dumps({
            "type": "object",
            "properties": {
                "summary": {"type": "string"},
                "issues": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "severity": {
                                "type": "string",
                                "enum": ["critical", "warning", "info"],
                            },
                            "description": {"type": "string"},
                            "line": {"type": "integer"},
                        },
                        "required": ["severity", "description"],
                    },
                },
                "score": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 100,
                },
            },
            "required": ["summary", "issues", "score"],
        })

        print("   Streaming: ", end="", flush=True)
        final_data = ""
        async for chunk in client.stream_generate_structured(
            session_id=session_id,
            messages=[
                {
                    "role": "ROLE_USER",
                    "content": (
                        "Review this code:\n"
                        "```python\n"
                        "def divide(a, b):\n"
                        "    return a / b\n"
                        "\n"
                        "result = divide(10, 0)\n"
                        "print(result)\n"
                        "```"
                    ),
                }
            ],
            schema=review_schema,
        ):
            if chunk.get("data"):
                final_data = chunk["data"]
                print(".", end="", flush=True)
            if chunk.get("done"):
                print(" done!")
                break

        if final_data:
            data = json.loads(final_data)
            print(f"✓ Code review:")
            print(f"  Summary: {data.get('summary')}")
            print(f"  Score: {data.get('score')}/100")
            for issue in data.get("issues", []):
                line_info = f" (line {issue['line']})" if issue.get("line") else ""
                print(f"  [{issue['severity'].upper()}]{line_info} {issue['description']}")
        print()

        # Clean up
        print("5. Cleaning up...")
        await client.destroy_session(session_id)
        print("✓ Session destroyed")
        print()

        print("=" * 60)
        print("Structured generation example complete! ✓")
        print("=" * 60)


if __name__ == "__main__":
    asyncio.run(structured_generation_example())
