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
from a3s_code import CodeAgentClient


async def main():
    async with CodeAgentClient() as client:
        # Initialize the agent
        await client.initialize(workspace="/path/to/project")

        # =====================================================================
        # Start Language Server
        # =====================================================================
        print("Starting Rust language server...")
        result = await client.start_lsp_server(
            language="rust",
            root_uri="file:///path/to/project"
        )
        print(f"Start result: {result}")

        # List running servers
        servers = await client.list_lsp_servers()
        print(f"Running servers: {servers}")

        # =====================================================================
        # Hover Information
        # =====================================================================
        print("\n--- Hover Information ---")
        hover = await client.lsp_hover(
            file_path="/path/to/project/src/main.rs",
            line=10,  # 0-indexed
            column=5
        )
        if hover["found"]:
            print(f"Hover content:\n{hover['content']}")
        else:
            print("No hover information available")

        # =====================================================================
        # Go to Definition
        # =====================================================================
        print("\n--- Go to Definition ---")
        definitions = await client.lsp_definition(
            file_path="/path/to/project/src/main.rs",
            line=15,
            column=10
        )
        for loc in definitions:
            print(f"Definition: {loc['uri']}:{loc['range']['start']['line']+1}")

        # =====================================================================
        # Find References
        # =====================================================================
        print("\n--- Find References ---")
        references = await client.lsp_references(
            file_path="/path/to/project/src/main.rs",
            line=20,
            column=8,
            include_declaration=True
        )
        print(f"Found {len(references)} references:")
        for loc in references[:5]:  # Show first 5
            print(f"  {loc['uri']}:{loc['range']['start']['line']+1}")

        # =====================================================================
        # Search Symbols
        # =====================================================================
        print("\n--- Search Symbols ---")
        symbols = await client.lsp_symbols(query="main", limit=10)
        print(f"Found {len(symbols)} symbols matching 'main':")
        for sym in symbols:
            print(f"  {sym['name']} ({sym['kind']})")

        # =====================================================================
        # Get Diagnostics
        # =====================================================================
        print("\n--- Diagnostics ---")
        diagnostics = await client.lsp_diagnostics(
            file_path="/path/to/project/src/main.rs"
        )
        if diagnostics:
            print(f"Found {len(diagnostics)} diagnostics:")
            for diag in diagnostics:
                line = diag['range']['start']['line'] + 1 if diag['range'] else '?'
                print(f"  [{diag['severity']}] Line {line}: {diag['message']}")
        else:
            print("No diagnostics")

        # =====================================================================
        # Stop Language Server
        # =====================================================================
        print("\n--- Stopping Server ---")
        success = await client.stop_lsp_server("rust")
        print(f"Server stopped: {success}")


if __name__ == "__main__":
    asyncio.run(main())
