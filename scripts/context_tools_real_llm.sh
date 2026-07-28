#!/usr/bin/env bash
set -euo pipefail

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
TEST_FILTER="${CONTEXT_TOOLS_REAL_LLM_FILTER:-}"

if [ ! -f "$CONFIG_FILE" ] && [ "${A3S_CONTEXT_TOOLS_USE_CODEX_LOGIN:-0}" != "1" ]; then
  echo "config file not found: $CONFIG_FILE" >&2
  exit 1
fi

cd "$WORKSPACE"
if [ -f "$CONFIG_FILE" ]; then
  export A3S_CONFIG_FILE="$CONFIG_FILE"
fi
export RUST_TEST_THREADS=1

command=(cargo test --locked -p a3s-code-core --test test_context_tools_real_llm)
if [ -n "${TARGET_DIR:-}" ]; then
  command+=(--target-dir "$TARGET_DIR")
fi
if [ -n "$TEST_FILTER" ]; then
  command+=("$TEST_FILTER")
fi
command+=(-- --ignored --nocapture --test-threads=1)

"${command[@]}"
