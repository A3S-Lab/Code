#!/usr/bin/env bash
# Explicit real-provider gate for the v9.1.3 original-Run SDK contract.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE="$(cd "$SCRIPT_DIR/.." && pwd)"
CONFIG_FILE="${A3S_CONFIG_FILE:?Set A3S_CONFIG_FILE to a credentialed ACL file}"
PYTHON_BIN="${A3S_CODE_PYTHON:-python3}"
BRIDGE_BINARY="${A3S_CODE_GO_BRIDGE_TEST_BINARY:?Build and select the current Go bridge}"

if [ "${A3S_CODE_ACL_ENV_PREPARED:-0}" != "1" ]; then
  exec bash "$SCRIPT_DIR/with_acl_env.sh" run "$CONFIG_FILE" bash "$SCRIPT_DIR/sdk_release_real_llm.sh"
fi
export RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}"
cd "$WORKSPACE"
status=0
node sdk/node/test_release_real_llm.mjs || status=1
"$PYTHON_BIN" sdk/python/tests/release_real_llm.py || status=1
(
  cd sdk/go
  A3S_CODE_GO_BRIDGE_TEST_BINARY="$BRIDGE_BINARY" \
    go test -race -tags=release_real_llm -run '^TestReleaseRealLLM$' -count=1 -v
) || status=1
exit "$status"
