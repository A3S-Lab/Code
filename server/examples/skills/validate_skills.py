#!/usr/bin/env python3
"""
Test script for validating skill examples.
This script loads and validates all skill example files.
"""

import json
import yaml
import sys
from pathlib import Path

def extract_frontmatter(content: str) -> dict:
    """Extract YAML frontmatter from markdown file."""
    if not content.startswith('---'):
        raise ValueError("No frontmatter found")

    parts = content.split('---', 2)
    if len(parts) < 3:
        raise ValueError("Invalid frontmatter format")

    return yaml.safe_load(parts[1])

def validate_skill(skill_path: Path) -> bool:
    """Validate a skill definition file."""
    print(f"\n{'='*60}")
    print(f"Validating: {skill_path.name}")
    print(f"{'='*60}")

    try:
        content = skill_path.read_text()
        frontmatter = extract_frontmatter(content)

        # Validate required fields
        assert 'name' in frontmatter, "Missing 'name' field"
        assert 'tools' in frontmatter, "Missing 'tools' field"
        assert isinstance(frontmatter['tools'], list), "'tools' must be a list"

        print(f"✓ Skill name: {frontmatter['name']}")
        print(f"✓ Description: {frontmatter.get('description', 'N/A')}")
        print(f"✓ Version: {frontmatter.get('version', 'N/A')}")
        print(f"✓ Tools count: {len(frontmatter['tools'])}")

        # Validate each tool
        for i, tool in enumerate(frontmatter['tools'], 1):
            print(f"\n  Tool {i}: {tool['name']}")

            # Validate tool fields
            assert 'name' in tool, f"Tool {i}: Missing 'name'"
            assert 'description' in tool, f"Tool {i}: Missing 'description'"
            assert 'backend' in tool, f"Tool {i}: Missing 'backend'"
            assert 'parameters' in tool, f"Tool {i}: Missing 'parameters'"

            # Validate backend
            backend = tool['backend']
            assert 'type' in backend, f"Tool {i}: Missing backend 'type'"
            backend_type = backend['type']
            print(f"    Backend: {backend_type}")

            if backend_type == 'binary':
                # Binary backend validation
                has_path_or_url = 'path' in backend or 'url' in backend
                assert has_path_or_url, f"Tool {i}: Binary backend needs 'path' or 'url'"
                if 'path' in backend:
                    print(f"    Path: {backend['path']}")
                if 'url' in backend:
                    print(f"    URL: {backend['url']}")
                if 'args_template' in backend:
                    print(f"    Args template: {backend['args_template']}")

            elif backend_type == 'http':
                # HTTP backend validation
                assert 'url' in backend, f"Tool {i}: HTTP backend needs 'url'"
                print(f"    URL: {backend['url']}")
                print(f"    Method: {backend.get('method', 'POST')}")
                print(f"    Timeout: {backend.get('timeout_ms', 30000)}ms")
                if 'headers' in backend:
                    print(f"    Headers: {len(backend['headers'])} defined")

            elif backend_type == 'script':
                # Script backend validation
                assert 'interpreter' in backend, f"Tool {i}: Script backend needs 'interpreter'"
                assert 'script' in backend, f"Tool {i}: Script backend needs 'script'"
                print(f"    Interpreter: {backend['interpreter']}")
                script_lines = backend['script'].strip().split('\n')
                print(f"    Script lines: {len(script_lines)}")
                if 'interpreter_args' in backend:
                    print(f"    Interpreter args: {backend['interpreter_args']}")
            else:
                raise ValueError(f"Tool {i}: Unknown backend type '{backend_type}'")

            # Validate parameters (JSON Schema)
            params = tool['parameters']
            assert 'type' in params, f"Tool {i}: Parameters missing 'type'"
            assert params['type'] == 'object', f"Tool {i}: Parameters type must be 'object'"

            if 'properties' in params:
                print(f"    Parameters: {len(params['properties'])} defined")
                for param_name in params['properties']:
                    param = params['properties'][param_name]
                    param_type = param.get('type', 'unknown')
                    print(f"      - {param_name}: {param_type}")

            if 'required' in params:
                print(f"    Required: {', '.join(params['required']) if params['required'] else 'none'}")

        print(f"\n✅ {skill_path.name} is valid!")
        return True

    except Exception as e:
        print(f"\n❌ {skill_path.name} validation failed: {e}")
        return False

def main():
    """Main validation function."""
    skills_dir = Path(__file__).parent
    skill_files = list(skills_dir.glob('*-example.md'))

    if not skill_files:
        print("No skill example files found!")
        return 1

    print(f"Found {len(skill_files)} skill example files")

    results = []
    for skill_file in sorted(skill_files):
        results.append(validate_skill(skill_file))

    print(f"\n{'='*60}")
    print("SUMMARY")
    print(f"{'='*60}")
    print(f"Total: {len(results)}")
    print(f"Passed: {sum(results)}")
    print(f"Failed: {len(results) - sum(results)}")

    return 0 if all(results) else 1

if __name__ == '__main__':
    try:
        import yaml
    except ImportError:
        print("Error: PyYAML is required. Install with: pip install pyyaml")
        sys.exit(1)

    sys.exit(main())
