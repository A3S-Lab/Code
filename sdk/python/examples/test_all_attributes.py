#!/usr/bin/env python3
"""Comprehensive test for all SubAgentConfig attributes."""

from a3s_code import SubAgentConfig

print("=" * 60)
print("Testing all SubAgentConfig attributes")
print("=" * 60)

# Create config with all parameters
cfg = SubAgentConfig(
    agent_type="test-agent",
    prompt="Test prompt",
    description="Test description",
    permissive=True,
    permissive_deny=["write", "bash"],
    max_steps=50,
    timeout_ms=30000,
    parent_id="parent-123",
    workspace="/test/workspace",
    agent_dirs=["/agents/dir1", "/agents/dir2"],
    skill_dirs=["/skills/dir1", "/skills/dir2"],
)

print("\n1. Testing all getters:")
print("-" * 60)

attributes = [
    ("agent_type", "test-agent"),
    ("prompt", "Test prompt"),
    ("description", "Test description"),
    ("permissive", True),
    ("permissive_deny", ["write", "bash"]),
    ("max_steps", 50),
    ("timeout_ms", 30000),
    ("parent_id", "parent-123"),
    ("workspace", "/test/workspace"),
    ("agent_dirs", ["/agents/dir1", "/agents/dir2"]),
    ("skill_dirs", ["/skills/dir1", "/skills/dir2"]),
]

all_passed = True
for attr_name, expected_value in attributes:
    try:
        actual_value = getattr(cfg, attr_name)
        if actual_value == expected_value:
            print(f"✓ {attr_name:20s} = {actual_value}")
        else:
            print(f"✗ {attr_name:20s} = {actual_value} (expected {expected_value})")
            all_passed = False
    except AttributeError as e:
        print(f"✗ {attr_name:20s} - AttributeError: {e}")
        all_passed = False

print("\n2. Testing hasattr for all attributes:")
print("-" * 60)

for attr_name, _ in attributes:
    has_attr = hasattr(cfg, attr_name)
    status = "✓" if has_attr else "✗"
    print(f"{status} hasattr(cfg, '{attr_name}') = {has_attr}")
    if not has_attr:
        all_passed = False

print("\n3. Testing setters:")
print("-" * 60)

# Test modifying each attribute
test_cases = [
    ("agent_type", "modified-agent"),
    ("prompt", "Modified prompt"),
    ("description", "Modified description"),
    ("permissive", False),
    ("permissive_deny", ["read"]),
    ("max_steps", 100),
    ("timeout_ms", 60000),
    ("parent_id", "new-parent-456"),
    ("workspace", "/new/workspace"),
    ("agent_dirs", ["/new/agents"]),
    ("skill_dirs", ["/new/skills"]),
]

for attr_name, new_value in test_cases:
    try:
        setattr(cfg, attr_name, new_value)
        actual_value = getattr(cfg, attr_name)
        if actual_value == new_value:
            print(f"✓ {attr_name:20s} updated to {new_value}")
        else:
            print(f"✗ {attr_name:20s} = {actual_value} (expected {new_value})")
            all_passed = False
    except Exception as e:
        print(f"✗ {attr_name:20s} - Error: {e}")
        all_passed = False

print("\n4. Testing the original bug scenario:")
print("-" * 60)

# Reproduce the original bug scenario
cfg_original = SubAgentConfig(
    agent_type="my-sub-agent",
    prompt="Call Skill('scoring-video-adapter')",
    workspace="/path/to/project",
    permissive=True,
    skill_dirs=["/path/to/project/skills"],
)

print(f"Created config with skill_dirs={cfg_original.skill_dirs}")
print(f"hasattr(cfg, 'skill_dirs') = {hasattr(cfg_original, 'skill_dirs')}")

if hasattr(cfg_original, 'skill_dirs') and cfg_original.skill_dirs == ["/path/to/project/skills"]:
    print("✓ Original bug is FIXED!")
else:
    print("✗ Original bug still exists")
    all_passed = False

print("\n" + "=" * 60)
if all_passed:
    print("✅ ALL TESTS PASSED!")
    print("SubAgentConfig is now fully functional with all attributes accessible.")
else:
    print("❌ SOME TESTS FAILED")
    print("Please review the output above for details.")
print("=" * 60)
