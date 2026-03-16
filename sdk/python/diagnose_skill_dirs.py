#!/usr/bin/env python3
"""
Diagnostic script for SubAgentConfig skill_dirs issue.

This script helps diagnose why skills are not being found by sub-agents.
"""

import os
import sys
from pathlib import Path

print("=" * 70)
print("SubAgentConfig skill_dirs Diagnostic Tool")
print("=" * 70)

# Check 1: Verify skill file format
print("\n1. Checking skill file format...")
print("-" * 70)

skill_file = input("Enter the full path to your skill file: ").strip()

if not os.path.exists(skill_file):
    print(f"❌ File not found: {skill_file}")
    sys.exit(1)

print(f"✓ File exists: {skill_file}")

with open(skill_file, 'r') as f:
    content = f.read()

print("\nFile content (first 500 chars):")
print("-" * 70)
print(content[:500])
print("-" * 70)

# Check frontmatter
if not content.startswith("---"):
    print("❌ ERROR: File must start with '---' (YAML frontmatter)")
    print("\nExpected format:")
    print("""---
name: skill-name
description: What the skill does
allowed-tools: "read(*), grep(*)"
kind: instruction
---
# Skill Instructions

Your skill content here...
""")
    sys.exit(1)

parts = content.split("---")
if len(parts) < 3:
    print("❌ ERROR: Invalid frontmatter format")
    print("   Frontmatter must be enclosed between two '---' markers")
    sys.exit(1)

frontmatter = parts[1].strip()
body = parts[2].strip()

print("\n✓ Frontmatter structure is valid")
print("\nFrontmatter content:")
print("-" * 70)
print(frontmatter)
print("-" * 70)

# Check for required fields
import re

name_match = re.search(r'^name:\s*(.+)$', frontmatter, re.MULTILINE)
if not name_match:
    print("❌ ERROR: 'name' field is required in frontmatter")
    sys.exit(1)

skill_name = name_match.group(1).strip()
print(f"\n✓ Skill name found: '{skill_name}'")

# Check for common formatting issues
issues = []

# Check for unclosed quotes
if frontmatter.count('"') % 2 != 0:
    issues.append("Unclosed double quotes in frontmatter")

if frontmatter.count("'") % 2 != 0:
    issues.append("Unclosed single quotes in frontmatter")

# Check for invalid fields
valid_fields = ['name', 'description', 'allowed-tools', 'disable-model-invocation',
                'kind', 'tags', 'version']
for line in frontmatter.split('\n'):
    line = line.strip()
    if ':' in line and not line.startswith('#'):
        field = line.split(':')[0].strip()
        if field and field not in valid_fields:
            issues.append(f"Unknown field '{field}' (valid: {', '.join(valid_fields)})")

if issues:
    print("\n⚠️  Potential issues found:")
    for issue in issues:
        print(f"   - {issue}")
else:
    print("\n✓ No obvious formatting issues detected")

# Check 2: Test with a3s_code
print("\n2. Testing with a3s_code SDK...")
print("-" * 70)

try:
    from a3s_code import SubAgentConfig

    skill_dir = str(Path(skill_file).parent)
    print(f"Skill directory: {skill_dir}")

    cfg = SubAgentConfig(
        agent_type="test",
        prompt=f"Call Skill('{skill_name}')",
        workspace=".",
        skill_dirs=[skill_dir]
    )

    print(f"\n✓ SubAgentConfig created successfully")
    print(f"  - skill_dirs: {cfg.skill_dirs}")
    print(f"  - hasattr(cfg, 'skill_dirs'): {hasattr(cfg, 'skill_dirs')}")

except ImportError as e:
    print(f"❌ Cannot import a3s_code: {e}")
    print("   Make sure the SDK is installed: pip install a3s-code")
    sys.exit(1)
except Exception as e:
    print(f"❌ Error creating SubAgentConfig: {e}")
    sys.exit(1)

# Check 3: Verify path is absolute
print("\n3. Checking path format...")
print("-" * 70)

if not os.path.isabs(skill_dir):
    print(f"⚠️  WARNING: Path is relative: {skill_dir}")
    print(f"   Absolute path: {os.path.abspath(skill_dir)}")
    print("   Recommendation: Use absolute paths for skill_dirs")
else:
    print(f"✓ Path is absolute: {skill_dir}")

# Check 4: Verify file extension
print("\n4. Checking file extension...")
print("-" * 70)

if not skill_file.endswith('.md'):
    print(f"❌ ERROR: Skill file must have .md extension")
    print(f"   Current: {skill_file}")
    sys.exit(1)
else:
    print(f"✓ File has .md extension")

# Summary
print("\n" + "=" * 70)
print("DIAGNOSTIC SUMMARY")
print("=" * 70)

print(f"""
Skill Information:
  - Name: {skill_name}
  - File: {skill_file}
  - Directory: {skill_dir}
  - File size: {len(content)} bytes

Configuration to use:
  SubAgentConfig(
      agent_type="your-agent-type",
      prompt="Call Skill('{skill_name}')",
      workspace="/your/workspace",
      skill_dirs=["{skill_dir}"],
      permissive=True
  )

Next Steps:
  1. If issues were found above, fix them in your skill file
  2. Make sure you're using the absolute path for skill_dirs
  3. Enable debug logging to see skill loading messages:

     import logging
     logging.basicConfig(level=logging.DEBUG)

  4. Check for warning messages like:
     - "Failed to load session skill dir"
     - "Skill validation failed"
""")

print("=" * 70)
