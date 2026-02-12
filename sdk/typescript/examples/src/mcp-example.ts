/**
 * MCP (Model Context Protocol) Example
 *
 * Demonstrates how to manage MCP servers for external tool integration:
 * - Registering MCP servers (stdio and HTTP transports)
 * - Connecting and disconnecting servers
 * - Listing servers and discovering tools
 * - Using MCP tools in generation
 */

import { A3sClient } from '@a3s-lab/code';

async function mcpExample(): Promise<void> {
  console.log('='.repeat(60));
  console.log('MCP (Model Context Protocol) Example');
  console.log('='.repeat(60));
  console.log();

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
  });

  try {
    // Create a session
    console.log('1. Creating session...');
    const session = await client.createSession({
      name: 'mcp-demo',
      workspace: '/tmp/mcp-test',
      systemPrompt: 'You are a helpful assistant with access to external tools via MCP.',
    });
    const sessionId = session.sessionId;
    console.log(`✓ Session created: ${sessionId}`);
    console.log();

    // Register MCP server (stdio transport)
    console.log('2. Registering MCP server (stdio transport)...');
    const regResult = await client.registerMcpServer({
      name: 'filesystem',
      transport: {
        stdio: {
          command: 'npx',
          args: ['-y', '@modelcontextprotocol/server-filesystem', '/tmp'],
        },
      },
      enabled: true,
      env: { NODE_ENV: 'production' },
    });
    console.log(`✓ Registered: success=${regResult.success}, message=${regResult.message}`);
    console.log();

    // Register MCP server (HTTP transport)
    console.log('3. Registering MCP server (HTTP transport)...');
    const httpResult = await client.registerMcpServer({
      name: 'web-search',
      transport: {
        http: {
          url: 'http://localhost:3001/mcp',
          headers: { Authorization: 'Bearer token-xxx' },
        },
      },
      enabled: true,
    });
    console.log(`✓ Registered: success=${httpResult.success}`);
    console.log();

    // Connect to MCP server
    console.log('4. Connecting to MCP server...');
    const connectResult = await client.connectMcpServer('filesystem');
    console.log(`✓ Connected: success=${connectResult.success}`);
    if (connectResult.toolNames?.length) {
      console.log(`  Available tools: ${connectResult.toolNames.join(', ')}`);
    }
    console.log();

    // List MCP servers
    console.log('5. Listing MCP servers...');
    const servers = await client.listMcpServers();
    console.log(`✓ Found ${servers.servers?.length || 0} servers:`);
    for (const server of servers.servers || []) {
      const status = server.connected ? 'connected' : 'disconnected';
      console.log(`  - ${server.name}: ${status}, ${server.toolCount} tools`);
      if (server.error) {
        console.log(`    Error: ${server.error}`);
      }
    }
    console.log();

    // Get MCP tools
    console.log('6. Getting all MCP tools...');
    const allTools = await client.getMcpTools();
    console.log(`✓ Total tools: ${allTools.tools?.length || 0}`);
    for (const tool of (allTools.tools || []).slice(0, 5)) {
      console.log(`  - ${tool.fullName}: ${tool.description}`);
    }
    console.log();

    // Get tools from specific server
    console.log('7. Getting tools from "filesystem" server...');
    const fsTools = await client.getMcpTools('filesystem');
    console.log(`✓ Filesystem tools: ${fsTools.tools?.length || 0}`);
    for (const tool of fsTools.tools || []) {
      console.log(`  - ${tool.toolName}: ${tool.description}`);
    }
    console.log();

    // Use MCP tools in generation
    console.log('8. Using MCP tools in generation...');
    const response = await client.generate(sessionId, [
      { role: 'ROLE_USER', content: 'List the files in /tmp using the filesystem MCP tool' },
    ]);
    if (response.message?.content) {
      console.log(`✓ Response: ${response.message.content.substring(0, 200)}...`);
    }
    console.log();

    // Disconnect MCP server
    console.log('9. Disconnecting MCP server...');
    const disconnectResult = await client.disconnectMcpServer('filesystem');
    console.log(`✓ Disconnected: ${disconnectResult.success}`);
    console.log();

    // Clean up
    console.log('10. Cleaning up...');
    await client.destroySession(sessionId);
    console.log('✓ Session destroyed');
    console.log();

    console.log('='.repeat(60));
    console.log('MCP example complete! ✓');
    console.log('='.repeat(60));
  } catch (error) {
    console.error('Error:', error);
  }
}

mcpExample();
