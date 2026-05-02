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

if [ ! -f "$CONFIG_FILE" ]; then
  echo "config file not found: $CONFIG_FILE" >&2
  exit 1
fi

if [ "$#" -gt 0 ]; then
  echo "minimax_prompt_smoke.sh no longer accepts per-test filters; 2.0 uses the ACL env integration suite." >&2
  exit 2
fi

if [ -z "${A3S_OPENAI_API_KEY:-}" ] && [ -n "${MINIMAX_API_KEY:-}" ]; then
  export A3S_OPENAI_API_KEY="$MINIMAX_API_KEY"
fi

if [ -z "${A3S_OPENAI_BASE_URL:-}" ] && [ -n "${MINIMAX_BASE_URL:-}" ]; then
  export A3S_OPENAI_BASE_URL="$MINIMAX_BASE_URL"
fi

echo "Running real MiniMax/OpenAI-compatible smoke through .a3s/config.acl"
echo "Config: $CONFIG_FILE"
A3S_CONFIG_FILE="$CONFIG_FILE" "$WORKSPACE/scripts/real_config_env_integration.sh"
