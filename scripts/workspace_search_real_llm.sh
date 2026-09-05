#!/usr/bin/env bash
set -euo pipefail

# One bounded live-model gate for the framework-owned workspace search path.
# The model is loaded from the selected .a3s/config.acl; this script never
# prints the config contents or forwards credentials on the command line.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE="$(cd "$SCRIPT_DIR/.." && pwd)"
CONFIG_ROOT="$WORKSPACE"
for candidate in "$WORKSPACE/../.." "$WORKSPACE" "$WORKSPACE/../../.."; do
  candidate="$(cd "$candidate" 2>/dev/null && pwd || true)"
  if [ -n "$candidate" ] && [ -f "$candidate/.a3s/config.acl" ]; then
    CONFIG_ROOT="$candidate"
    break
  fi
done

CONFIG_FILE="${A3S_CONFIG_FILE:-$CONFIG_ROOT/.a3s/config.acl}"
if [ ! -f "$CONFIG_FILE" ]; then
  echo "config file not found: $CONFIG_FILE" >&2
  exit 1
fi

if [ "${1:-}" = "--dry-run" ]; then
  cd "$WORKSPACE"
  A3S_CONFIG_FILE="$CONFIG_FILE" cargo test --locked -p a3s-code-core \
    --features zvec-rust-fts --test test_workspace_search_real_llm \
    --no-run
  exit 0
fi

if [ "$#" -ne 0 ]; then
  echo "usage: $0 [--dry-run]" >&2
  exit 2
fi

cd "$WORKSPACE"
export A3S_CONFIG_FILE="$CONFIG_FILE"
export RUST_TEST_THREADS=1

# Establish the local correctness and acceleration invariant before spending a
# provider request. This gate is deterministic and proves the same public
# service constructor used by the live model test selects the native index.
cargo test --locked -p a3s-code-core --features zvec-rust-fts --lib \
  tools::builtin::bm25::tests -- --nocapture --test-threads=1

cargo test --locked -p a3s-code-core --features zvec-rust-fts --lib \
  workspace::retrieval::runtime::tests \
  -- --nocapture --test-threads=1

cargo test --locked -p a3s-code-core --features zvec-rust-fts --lib \
  workspace::retrieval::persistent::tests \
  -- --nocapture --test-threads=1

cargo test --locked -p a3s-code-core \
  --features zvec-rust-fts --test test_workspace_search_real_llm \
  -- --ignored --nocapture --test-threads=1
