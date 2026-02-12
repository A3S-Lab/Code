"""
LSP (Language Server Protocol) Example

Demonstrates how to use LSP features for code intelligence:
- Starting/stopping language servers
- Getting hover information
- Go to definition
- Find references
- Search symbols
- Get diagnostics
"""

import asyncio
from a3s_code import A3sClient


async def lsp_example():
    print("=" * 60)
    print("LSP (Language Server Protocol) Example")
    print("=" * 60)
    print()

    async with A3sClient(address="localhost:4088") as client:
        # Create a session with a real project workspace
        print("1. Creating session...")
        session = await client.create_session(
            name="lsp-demo",
            workspace="/path/to/project",
            system_prompt="You are a helpful coding assistant with LSP support.",
        )
        session_id = session["session_id"]
        print(f"✓ Session created: {session_id}")
        print()

        # =====================================================================
        # Start Language Server
        # =====================================================================
        print("2. Starting Rust language server...")
        result = await client.start_lsp_server(
            language="rust",
            root_uri="file:///path/to/project",
        )
        print(f"✓ Server started: {result}")
        print()

        # List running servers
        print("3. Listing running servers...")
        servers = await client.list_lsp_servers()
        print(f"✓ Running servers: {len(servers)}")
        for server in servers:
            print(f"  - {server.get('language')}: {server.get('status', 'unknown')}")
        print()

        # =====================================================================
        # Hover Information
        # =====================================================================
        print("4. Getting hover information...")
        hover = await client.lsp_hover(
            file_path="/path/to/project/src/main.rs",
            line=10,  # 0-indexed
            column=5,
        )
        if hover.get("found"):
            print(f"✓ Hover content:")
            print(f"  {hover.get('content', '')[:200]}")
        else:
            print("  No hover information available at this position")
        print()

        # =====================================================================
        # Go to Definition
        # =====================================================================
        print("5. Going to definition...")
        definitions = await client.lsp_definition(
            file_path="/path/to/project/src/main.rs",
            line=15,
            column=10,
        )
        if definitions:
            print(f"✓ Found {len(definitions)} definition(s):")
            for loc in definitions:
                uri = loc.get("uri", "")
                start = loc.get("range", {}).get("start", {})
                line = start.get("line", 0) + 1
                print(f"  {uri}:{line}")
        else:
            print("  No definition found")
        print()

        # =====================================================================
        # Find References
        # =====================================================================
        print("6. Finding references...")
        references = await client.lsp_references(
            file_path="/path/to/project/src/main.rs",
            line=20,
            column=8,
            include_declaration=True,
        )
        print(f"✓ Found {len(references)} references:")
        for loc in references[:5]:  # Show first 5
            uri = loc.get("uri", "")
            start = loc.get("range", {}).get("start", {})
            line = start.get("line", 0) + 1
            print(f"  {uri}:{line}")
        if len(references) > 5:
            print(f"  ... and {len(references) - 5} more")
        print()

        # =====================================================================
        # Search Symbols
        # =====================================================================
        print("7. Searching symbols...")
        symbols = await client.lsp_symbols(query="main", limit=10)
        print(f"✓ Found {len(symbols)} symbols matching 'main':")
        for sym in symbols:
            print(f"  - {sym.get('name')} ({sym.get('kind')})")
        print()

        # =====================================================================
        # Get Diagnostics
        # =====================================================================
        print("8. Getting diagnostics...")

        # For a specific file
        diagnostics = await client.lsp_diagnostics(
            file_path="/path/to/project/src/main.rs"
        )
        if diagnostics:
            print(f"✓ Found {len(diagnostics)} diagnostics:")
            for diag in diagnostics:
                start = diag.get("range", {}).get("start", {})
                line = start.get("line", 0) + 1 if start else "?"
                severity = diag.get("severity", "unknown")
                print(f"  [{severity}] Line {line}: {diag.get('message', '')}")
        else:
            print("  No diagnostics (clean code!)")
        print()

        # All diagnostics (no file filter)
        print("9. Getting all project diagnostics...")
        all_diags = await client.lsp_diagnostics()
        print(f"✓ Total diagnostics across project: {len(all_diags)}")
        print()

        # =====================================================================
        # Stop Language Server
        # =====================================================================
        print("10. Stopping language server...")
        success = await client.stop_lsp_server("rust")
        print(f"✓ Server stopped: {success}")
        print()

        # Clean up
        print("11. Cleaning up...")
        await client.destroy_session(session_id)
        print("✓ Session destroyed")
        print()

        print("=" * 60)
        print("LSP example complete! ✓")
        print("=" * 60)


if __name__ == "__main__":
    asyncio.run(lsp_example())
