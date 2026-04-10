#!/bin/bash
# Check version alignment across all SDK files

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "=========================================="
echo "Version Alignment Check"
echo "=========================================="
echo ""

# Extract versions
CORE_VERSION=$(grep '^version = ' core/Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
NODE_CARGO_VERSION=$(grep '^version = ' sdk/node/Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
NODE_PKG_VERSION=$(grep '"version":' sdk/node/package.json | sed 's/.*"version": "\(.*\)".*/\1/')
NODE_LOCK_VERSION=$(node -p "require('./sdk/node/package-lock.json').packages[''].version")
PYTHON_CARGO_VERSION=$(grep '^version = ' sdk/python/Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
PYTHON_PYPROJECT_VERSION=$(grep '^version = ' sdk/python/pyproject.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
BOOTSTRAP_VERSION=$(grep '^version = ' sdk/python-bootstrap/pyproject.toml | head -1 | sed 's/version = "\(.*\)"/\1/')

echo "Current versions:"
echo "  core/Cargo.toml:                    ${CORE_VERSION}"
echo "  sdk/node/Cargo.toml:                ${NODE_CARGO_VERSION}"
echo "  sdk/node/package.json:              ${NODE_PKG_VERSION}"
echo "  sdk/node/package-lock:              ${NODE_LOCK_VERSION}"
echo "  sdk/python/Cargo.toml:              ${PYTHON_CARGO_VERSION}"
echo "  sdk/python/pyproject.toml:          ${PYTHON_PYPROJECT_VERSION}"
echo "  sdk/python-bootstrap/pyproject.toml: ${BOOTSTRAP_VERSION}"
echo ""

# Check alignment
if [ "$CORE_VERSION" = "$NODE_CARGO_VERSION" ] && \
   [ "$CORE_VERSION" = "$NODE_PKG_VERSION" ] && \
   [ "$CORE_VERSION" = "$NODE_LOCK_VERSION" ] && \
   [ "$CORE_VERSION" = "$PYTHON_CARGO_VERSION" ] && \
   [ "$CORE_VERSION" = "$PYTHON_PYPROJECT_VERSION" ] && \
   [ "$CORE_VERSION" = "$BOOTSTRAP_VERSION" ]; then
    echo "✅ All versions aligned: ${CORE_VERSION}"
    exit 0
else
    echo "❌ Version mismatch detected!"
    echo ""
    echo "Expected: ${CORE_VERSION}"
    echo "Mismatches:"
    [ "$CORE_VERSION" != "$NODE_CARGO_VERSION" ] && echo "  - sdk/node/Cargo.toml: ${NODE_CARGO_VERSION}"
    [ "$CORE_VERSION" != "$NODE_PKG_VERSION" ] && echo "  - sdk/node/package.json: ${NODE_PKG_VERSION}"
    [ "$CORE_VERSION" != "$NODE_LOCK_VERSION" ] && echo "  - sdk/node/package-lock.json: ${NODE_LOCK_VERSION}"
    [ "$CORE_VERSION" != "$PYTHON_CARGO_VERSION" ] && echo "  - sdk/python/Cargo.toml: ${PYTHON_CARGO_VERSION}"
    [ "$CORE_VERSION" != "$PYTHON_PYPROJECT_VERSION" ] && echo "  - sdk/python/pyproject.toml: ${PYTHON_PYPROJECT_VERSION}"
    [ "$CORE_VERSION" != "$BOOTSTRAP_VERSION" ] && echo "  - sdk/python-bootstrap/pyproject.toml: ${BOOTSTRAP_VERSION}"
    exit 1
fi
