#!/usr/bin/env bash
set -euo pipefail

WORKSPACE="$(cd "$(dirname "$0")/.." && pwd)"
cd "$WORKSPACE"
build=(rustup run 1.97.1 cargo build --locked --quiet -p a3s-code-core --example acl_env_config)
target="${TARGET_DIR:-${CARGO_TARGET_DIR:-$WORKSPACE/target}}"
if [[ "$target" != /* ]]; then target="$WORKSPACE/$target"; fi
build+=(--target-dir "$target")
"${build[@]}"
exec "$target/debug/examples/acl_env_config" "$@"
