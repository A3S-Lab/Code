#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
CONFIG_FILE="${A3S_CONFIG_FILE:-$ROOT/.a3s/config.acl}"

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
A3S_CONFIG_FILE="$CONFIG_FILE" "$ROOT/crates/code/scripts/real_config_env_integration.sh"
