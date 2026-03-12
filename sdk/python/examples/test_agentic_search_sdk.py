"""Test that agentic_search tool is available in Python SDK
and that DocumentParserRegistry can be configured via SessionOptions.
"""

import os
import tempfile
import shutil
from pathlib import Path

try:
    from a3s_code import Agent, SessionOptions, DocumentParserRegistry
except ImportError:
    print("⚠️  a3s_code module not installed, skipping test")
    print("   Run: pip install -e . (from sdk/python directory)")
    exit(0)


def test_agentic_search_available():
    """Test that agentic_search is available in the SDK."""
    tmpdir = tempfile.mkdtemp(prefix='agentic-search-test-')

    try:
        auth_file = Path(tmpdir) / 'auth.py'
        auth_file.write_text('''
def authenticate(token: str):
    """JWT token validation"""
    return validate_jwt(token)
''')

        config_path = Path(tmpdir) / 'agent.hcl'
        config_path.write_text('''
default_model = "anthropic/claude-sonnet-4-20250514"
providers {
  name    = "anthropic"
  api_key = env("ANTHROPIC_API_KEY")
}
''')

        agent = Agent(str(config_path))

        # Test 1: default session (no document parser registry)
        session1 = agent.session(tmpdir)
        tools1 = session1.tool_names()
        if 'agentic_search' not in tools1:
            raise AssertionError('agentic_search not found in default session')
        print('✅ agentic_search available in default session')

        # Test 2: session with DocumentParserRegistry
        opts = SessionOptions()
        opts.document_parser_registry = DocumentParserRegistry()
        session2 = agent.session(tmpdir, opts)
        tools2 = session2.tool_names()
        if 'agentic_search' not in tools2:
            raise AssertionError('agentic_search not found in session with DocumentParserRegistry')
        print('✅ agentic_search available with DocumentParserRegistry')

    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)


if __name__ == '__main__':
    try:
        test_agentic_search_available()
        print('\n✅ All tests passed')
    except Exception as e:
        print(f'\n❌ Test failed: {e}')
        exit(1)
