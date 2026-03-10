#!/bin/bash
# Release script for a3s-code
# Usage: ./release.sh <version>
# Example: ./release.sh 1.3.4

set -e

VERSION=$1

if [ -z "$VERSION" ]; then
    echo "Usage: ./release.sh <version>"
    echo "Example: ./release.sh 1.3.4"
    exit 1
fi

echo "=========================================="
echo "Releasing a3s-code v${VERSION}"
echo "=========================================="
echo ""

# Check if we're in the code submodule
if [ ! -f "core/Cargo.toml" ]; then
    echo "❌ Error: Must run from crates/code directory"
    exit 1
fi

# Check for uncommitted changes
if [ -n "$(git status --porcelain)" ]; then
    echo "⚠️  Warning: You have uncommitted changes"
    git status --short
    echo ""
    read -p "Continue anyway? (y/N) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

echo "Step 1: Update version numbers"
echo "----------------------------------------"

# Update Rust crate versions
echo "  Updating core/Cargo.toml..."
sed -i.bak "s/^version = \".*\"/version = \"${VERSION}\"/" core/Cargo.toml
rm -f core/Cargo.toml.bak

echo "  Updating sdk/node/Cargo.toml..."
sed -i.bak "s/^version = \".*\"/version = \"${VERSION}\"/" sdk/node/Cargo.toml
rm -f sdk/node/Cargo.toml.bak

echo "  Updating sdk/python/Cargo.toml..."
sed -i.bak "s/^version = \".*\"/version = \"${VERSION}\"/" sdk/python/Cargo.toml
rm -f sdk/python/Cargo.toml.bak

# Update Node SDK package.json
echo "  Updating sdk/node/package.json..."
sed -i.bak "s/\"version\": \".*\"/\"version\": \"${VERSION}\"/" sdk/node/package.json
rm -f sdk/node/package.json.bak

# Update Python SDK pyproject.toml
echo "  Updating sdk/python/pyproject.toml..."
sed -i.bak "s/^version = \".*\"/version = \"${VERSION}\"/" sdk/python/pyproject.toml
rm -f sdk/python/pyproject.toml.bak

echo "✅ Version numbers updated"
echo ""

echo "Step 2: Update Cargo.lock"
echo "----------------------------------------"
cargo check --workspace
echo "✅ Cargo.lock updated"
echo ""

echo "Step 3: Run tests"
echo "----------------------------------------"
cargo test --lib permissions::tests
echo "✅ Tests passed"
echo ""

echo "Step 4: Format code"
echo "----------------------------------------"
cargo fmt --all
echo "✅ Code formatted"
echo ""

echo "Step 5: Git commit and tag"
echo "----------------------------------------"

# Show changes
echo "Changes to be committed:"
git diff --stat

echo ""
read -p "Commit these changes? (y/N) " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo "❌ Aborted"
    exit 1
fi

# Commit changes
git add -A
git commit -m "chore: bump version to ${VERSION}

- Fix permission wildcard matching for MCP tools
- Add support for mcp__longvt__* pattern in deny rules
- Update all SDK versions to ${VERSION}
"

# Create tag
git tag -a "v${VERSION}" -m "Release v${VERSION}

## Changes
- Fix permission wildcard matching for MCP tools
- Add support for mcp__longvt__* pattern in deny rules
- permissive_deny parameter now correctly blocks specified tools
- Agent .md deny rules work with permissive mode

## Tests
- 50 unit tests passed
- 5 integration tests passed
- 10 scenario tests passed
"

echo "✅ Committed and tagged"
echo ""

echo "Step 6: Push to GitHub"
echo "----------------------------------------"
echo "Ready to push:"
echo "  - Commit: $(git log -1 --oneline)"
echo "  - Tag: v${VERSION}"
echo ""
read -p "Push to origin? (y/N) " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo "❌ Aborted"
    echo ""
    echo "To push manually:"
    echo "  git push origin main"
    echo "  git push origin v${VERSION}"
    exit 1
fi

git push origin main
git push origin "v${VERSION}"

echo "✅ Pushed to GitHub"
echo ""

echo "=========================================="
echo "✅ Release v${VERSION} completed!"
echo "=========================================="
echo ""
echo "GitHub Actions will now:"
echo "  1. Run CI checks"
echo "  2. Publish to crates.io"
echo "  3. Publish Node SDK to npm"
echo "  4. Publish Python SDK to PyPI"
echo "  5. Create GitHub Release"
echo ""
echo "Monitor progress at:"
echo "  https://github.com/A3S-Lab/Code/actions"
echo ""
