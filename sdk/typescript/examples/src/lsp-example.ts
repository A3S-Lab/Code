/**
 * LSP (Language Server Protocol) Example
 *
 * Demonstrates how to use LSP features for code intelligence:
 * - Starting/stopping language servers
 * - Getting hover information
 * - Go to definition
 * - Find references
 * - Search symbols
 * - Get diagnostics
 */

import { A3sClient } from '@a3s-lab/code';

async function lspExample(): Promise<void> {
  console.log('='.repeat(60));
  console.log('LSP (Language Server Protocol) Example');
  console.log('='.repeat(60));
  console.log();

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
  });

  try {
    // Create a session
    console.log('1. Creating session...');
    const session = await client.createSession({
      name: 'lsp-demo',
      workspace: '/path/to/project',
      systemPrompt: 'You are a helpful coding assistant with LSP support.',
    });
    const sessionId = session.sessionId;
    console.log(`✓ Session created: ${sessionId}`);
    console.log();

    // Start language server
    console.log('2. Starting Rust language server...');
    const startResult = await client.startLspServer('rust', 'file:///path/to/project');
    console.log(`✓ Server started: ${JSON.stringify(startResult)}`);
    console.log();

    // List running servers
    console.log('3. Listing running servers...');
    const servers = await client.listLspServers();
    console.log(`✓ Running servers: ${servers.servers?.length || 0}`);
    for (const server of servers.servers || []) {
      console.log(`  - ${server.language}: ${server.status || 'unknown'}`);
    }
    console.log();

    // Hover information
    console.log('4. Getting hover information...');
    const hover = await client.lspHover(
      '/path/to/project/src/main.rs',
      10,
      5,
    );
    if (hover.found) {
      console.log('✓ Hover content:');
      console.log(`  ${(hover.content || '').substring(0, 200)}`);
    } else {
      console.log('  No hover information available at this position');
    }
    console.log();

    // Go to definition
    console.log('5. Going to definition...');
    const definitions = await client.lspDefinition(
      '/path/to/project/src/main.rs',
      15,
      10,
    );
    const defLocations = definitions.locations || [];
    if (defLocations.length > 0) {
      console.log(`✓ Found ${defLocations.length} definition(s):`);
      for (const loc of defLocations) {
        const line = (loc.range?.start?.line || 0) + 1;
        console.log(`  ${loc.uri}:${line}`);
      }
    } else {
      console.log('  No definition found');
    }
    console.log();

    // Find references
    console.log('6. Finding references...');
    const references = await client.lspReferences(
      '/path/to/project/src/main.rs',
      20,
      8,
      true,
    );
    const refLocations = references.locations || [];
    console.log(`✓ Found ${refLocations.length} references:`);
    for (const loc of refLocations.slice(0, 5)) {
      const line = (loc.range?.start?.line || 0) + 1;
      console.log(`  ${loc.uri}:${line}`);
    }
    if (refLocations.length > 5) {
      console.log(`  ... and ${refLocations.length - 5} more`);
    }
    console.log();

    // Search symbols
    console.log('7. Searching symbols...');
    const symbols = await client.lspSymbols('main', 10);
    const symbolList = symbols.symbols || [];
    console.log(`✓ Found ${symbolList.length} symbols matching 'main':`);
    for (const sym of symbolList) {
      console.log(`  - ${sym.name} (${sym.kind})`);
    }
    console.log();

    // Get diagnostics
    console.log('8. Getting diagnostics...');
    const diagnostics = await client.lspDiagnostics('/path/to/project/src/main.rs');
    const diagList = diagnostics.diagnostics || [];
    if (diagList.length > 0) {
      console.log(`✓ Found ${diagList.length} diagnostics:`);
      for (const diag of diagList) {
        const line = (diag.range?.start?.line || 0) + 1;
        console.log(`  [${diag.severity}] Line ${line}: ${diag.message}`);
      }
    } else {
      console.log('  No diagnostics (clean code!)');
    }
    console.log();

    // All project diagnostics
    console.log('9. Getting all project diagnostics...');
    const allDiags = await client.lspDiagnostics();
    console.log(`✓ Total diagnostics: ${allDiags.diagnostics?.length || 0}`);
    console.log();

    // Stop language server
    console.log('10. Stopping language server...');
    const stopResult = await client.stopLspServer('rust');
    console.log(`✓ Server stopped: ${stopResult.success}`);
    console.log();

    // Clean up
    console.log('11. Cleaning up...');
    await client.destroySession(sessionId);
    console.log('✓ Session destroyed');
    console.log();

    console.log('='.repeat(60));
    console.log('LSP example complete! ✓');
    console.log('='.repeat(60));
  } catch (error) {
    console.error('Error:', error);
  }
}

lspExample();
