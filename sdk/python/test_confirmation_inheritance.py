"""
Integration test for ConfirmationInheritance in Python SDK.

Tests that the new confirmation_inheritance field is properly exposed
and works end-to-end with real LLM calls.

Requires: A3S_CONFIG_FILE environment variable pointing to config.acl
"""

import os
import sys
import tempfile
from pathlib import Path

# Import from the built native module
try:
    from a3s_code import Agent, WorkerAgentSpec, SessionOptions, PermissionPolicy
except ImportError:
    print("ERROR: a3s_code module not found. Build with: cd sdk/python && maturin develop")
    sys.exit(1)

config_file = os.environ.get('A3S_CONFIG_FILE')
if not config_file:
    print("ERROR: A3S_CONFIG_FILE must point to the ACL config")
    sys.exit(1)

print('[python-sdk-confirmation] Starting integration test...')
print(f'[python-sdk-confirmation] Config: {config_file}')

# Read config file content
with open(config_file, 'r') as f:
    config_content = f.read()

agent = Agent.create(config_content)
workspace = tempfile.mkdtemp(prefix='a3s-python-confirmation-')
print(f'[python-sdk-confirmation] Workspace: {workspace}')

# Test 1: WorkerAgentSpec with confirmation_inheritance field
print('[python-sdk-confirmation] Test 1: Create WorkerAgentSpec with confirmation_inheritance')
worker_spec = WorkerAgentSpec('test-writer', 'Test worker with auto-approve confirmation', 'implementer')
worker_spec.confirmation_inheritance = 'auto_approve'
worker_spec.max_steps = 3

# Create SessionOptions properly
policy = PermissionPolicy(default_decision='allow')
opts = SessionOptions()
opts.permission_policy = policy
opts.worker_agents = [worker_spec]

session = agent.session(workspace, opts)

# Test 2: Verify worker was registered
print('[python-sdk-confirmation] Test 2: Verify worker registration')
tool_names = session.tool_names()
assert 'task' in tool_names, 'task tool should be registered'

# Test 3: Register another worker and check AgentDefinition
print('[python-sdk-confirmation] Test 3: Register worker and check AgentDefinition')
worker_spec2 = WorkerAgentSpec('test-reader', 'Test worker with deny-on-ask confirmation', 'read_only')
worker_spec2.confirmation_inheritance = 'deny_on_ask'
worker_spec2.max_steps = 2

agent_def = session.register_worker_agent(worker_spec2)
assert agent_def.name == 'test-reader'
assert agent_def.confirmation_inheritance == 'deny_on_ask'
print(f'[python-sdk-confirmation] AgentDefinition.confirmation_inheritance: {agent_def.confirmation_inheritance}')

# Test 4: Real LLM task delegation with confirmation_inheritance
if os.environ.get('A3S_CODE_SDK_REAL_AGENT_SMOKE') != '0':
    print('[python-sdk-confirmation] Test 4: Real LLM task delegation')

    # Create a test file for the worker to read
    test_file = Path(workspace) / 'test.txt'
    test_file.write_text('CONFIRMATION_TEST_CONTENT')

    task_result = session.task({
        'agent': 'test-reader',
        'description': 'Read test file',
        'prompt': 'Read the file test.txt and reply with its content',
        'max_steps': 2,
    })

    assert task_result['exit_code'] == 0, f"Task should succeed: {task_result['output']}"
    output = task_result['output']
    assert 'CONFIRMATION_TEST_CONTENT' in output or 'test.txt' in output, \
        'Task output should reference the test file'
    print('[python-sdk-confirmation] Task delegation successful')
else:
    print('[python-sdk-confirmation] Test 4: Skipped (set A3S_CODE_SDK_REAL_AGENT_SMOKE=1 to enable)')

print('[python-sdk-confirmation] All tests passed ✓')
