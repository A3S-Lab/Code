#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
WORKSPACE="$ROOT/crates/code"
CONFIG_FILE="${A3S_CONFIG_FILE:-$ROOT/.a3s/config.hcl}"
MODEL_ID="${MINIMAX_MODEL_ID:-MiniMax-M2.7-highspeed}"

if [ ! -f "$CONFIG_FILE" ]; then
  echo "config file not found: $CONFIG_FILE" >&2
  exit 1
fi

eval "$(
python3 - <<'PY' "$CONFIG_FILE" "$MODEL_ID"
from pathlib import Path
import re
import shlex
import sys

text = Path(sys.argv[1]).read_text()
model_id = sys.argv[2]
pattern = (
    r'"id"\s*=\s*"' + re.escape(model_id) +
    r'".*?"apiKey"\s*=\s*"([^"]+)".*?"baseUrl"\s*=\s*"([^"]+)"'
)
match = re.search(pattern, text, re.S)
if not match:
    raise SystemExit(f"failed to find {model_id} apiKey/baseUrl in {sys.argv[1]}")

api_key, base_url = match.groups()
print(f"export MINIMAX_API_KEY={shlex.quote(api_key)}")
print(f"export MINIMAX_BASE_URL={shlex.quote(base_url.rstrip('/'))}")
print(f"export MINIMAX_MODEL={shlex.quote(model_id)}")
PY
)"

export RUST_TEST_THREADS=1

if [ "$#" -gt 0 ]; then
  TESTS=("$@")
else
  TESTS=(
    test_llm_classify_intent_plan
    test_llm_classify_intent_explore
    test_llm_classify_intent_verification
    test_agent_style_detection_plan_message
    test_agent_style_detection_explore_message
  )
fi

cd "$WORKSPACE"

for test_name in "${TESTS[@]}"; do
  echo "==> Running $test_name with model $MINIMAX_MODEL"
  cargo test -p a3s-code-core --test test_prompts_with_llm "$test_name" -- --exact --ignored --nocapture --test-threads=1
done
