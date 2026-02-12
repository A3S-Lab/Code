"""
MCP (Model Context Protocol) Example

Demonstrates how to manage MCP servers for external tool integration:
- Registering MCP servers (stdio and HTTP transports)
- Connecting and disconnecting servers
- Listing servers and discovering tools
- Using MCP tools in generation
"""

import asyncio
from a3s_code import A3sClient
from a3s_code.types import McpServerConfig, McpTransport, McpStdioTransport, McpHttpTransport


async def mcp_example():
    print("=" * 60)
    print("MCP (Model Context Protocol) Example")
    print("=" * 60)
    print()

    async with A3sClient(address="localhost:4088") as client:
        # Create a session
        print("1. Creating session...")
        session = await client.create_session(
            name="mcp-demo",
            workspace="/tmp/mcp-test",
            system_prompt="You are a helpful assistant with access to external tools via MCP.",
        )
        session_id = session["session_id"]
        print(f"✓ Session created: {session_id}")
        print()

        # =====================================================================
        # Register MCP Server (stdio transport)
        # =====================================================================
        print("2. Registering MCP server (stdio transport)...")
        result = await client.register_mcp_server(
            McpServerConfig(
                name="filesystem",
                transport=McpTransport(
                    stdio=McpStdioTransport(
                        command="npx",
                        args=["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
                    )
                ),
                enabled=True,
                env={"NODE_ENV": "production"},
            )
        )
        print(f"✓ Registered: success={result['success']}, message={result['message']}")
        print()

        # =====================================================================
        # Register MCP Server (HTTP transport)
        # =====================================================================
        print("3. Registering MCP server (HTTP transport)...")
        result = await client.register_mcp_server(
            McpServerConfig(
                name="web-search",
                transport=McpTransport(
                    http=McpHttpTransport(
                        url="http://localhost:3001/mcp",
                        headers={"Authorization": "Bearer token-xxx"},
                    )
                ),
                enabled=True,
            )
        )
        print(f"✓ Registered: success={result['success']}")
        print()

        # =====================================================================
        # Connect to MCP Server
        # =====================================================================
        print("4. Connecting to MCP server...")
        connect_result = await client.connect_mcp_server("filesystem")
        print(f"✓ Connected: success={connect_result['success']}")
        if connect_result.get("tool_names"):
            print(f"  Available tools: {', '.join(connect_result['tool_names'])}")
        print()

        # =====================================================================
        # List MCP Servers
        # =====================================================================
        print("5. Listing MCP servers...")
        servers = await client.list_mcp_servers()
        print(f"✓ Found {len(servers)} servers:")
        for server in servers:
            status = "connected" if server.connected else "disconnected"
            print(f"  - {server.name}: {status}, {server.tool_count} tools")
            if server.error:
                print(f"    Error: {server.error}")
        print()

        # =====================================================================
        # Get MCP Tools
        # =====================================================================
        print("6. Getting MCP tools...")

        # Get all tools
        all_tools = await client.get_mcp_tools()
        print(f"✓ Total tools: {len(all_tools)}")
        for tool in all_tools[:5]:  # Show first 5
            print(f"  - {tool.full_name}: {tool.description}")
        print()

        # Get tools from specific server
        print("7. Getting tools from 'filesystem' server...")
        fs_tools = await client.get_mcp_tools(server_name="filesystem")
        print(f"✓ Filesystem tools: {len(fs_tools)}")
        for tool in fs_tools:
            print(f"  - {tool.tool_name}: {tool.description}")
            if tool.input_schema:
                print(f"    Schema: {tool.input_schema[:80]}...")
        print()

        # =====================================================================
        # Use MCP Tools in Generation
        # =====================================================================
        print("8. Using MCP tools in generation...")
        response = await client.generate(
            session_id=session_id,
            messages=[
                {
                    "role": "ROLE_USER",
                    "content": "List the files in /tmp using the filesystem MCP tool",
                }
            ],
        )
        if "message" in response:
            content = response["message"].get("content", "")
            print(f"✓ Response: {content[:200]}...")
        print()

        # =====================================================================
        # Disconnect MCP Server
        # =====================================================================
        print("9. Disconnecting MCP server...")
        success = await client.disconnect_mcp_server("filesystem")
        print(f"✓ Disconnected: {success}")
        print()

        # Verify disconnection
        servers = await client.list_mcp_servers()
        for server in servers:
            if server.name == "filesystem":
                print(f"  Server status: {'connected' if server.connected else 'disconnected'}")
        print()

        # Clean up
        print("10. Cleaning up...")
        await client.destroy_session(session_id)
        print("✓ Session destroyed")
        print()

        print("=" * 60)
        print("MCP example complete! ✓")
        print("=" * 60)


if __name__ == "__main__":
    asyncio.run(mcp_example())
