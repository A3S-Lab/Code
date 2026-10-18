#!/bin/bash
# Build and qualify the actual debug pager used by ACP end-to-end tests.
set -euo pipefail
script_dir="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
pager="$repo_root/vendor/a3s-code-ui"
bash "$script_dir/prepare-shipped-tui.sh"
cd "$pager"

# The end-to-end tests only launch this binary.
export CARGO_PROFILE_DEV_DEBUG=0
export RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}"
# The pager is excluded from Code's workspace tests. Exercise the actual
# shipped entry point and its command boundary here, not the thin scaffold.
rustup run 1.97.1 cargo test --locked -p a3s-code-pager --lib app::dispatch::tests::prompt:: -- --test-threads=1
rustup run 1.97.1 cargo test --locked -p a3s-code-pager-bin --bin a3s-code-tui startup::tests:: -- --test-threads=1
rustup run 1.97.1 cargo build --locked -p a3s-code-pager-bin --bin a3s-code-tui
test -x target/debug/a3s-code-tui
expected_version="$(rustup run 1.97.1 cargo metadata --locked --no-deps --format-version 1 | python3 -c 'import json, sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "a3s-code-pager-bin"))')"
revision_args=()
# Standalone Code CI must stamp the checked-out Code revision. A nested OS
# development build uses CODE_REVISION and cannot attest to a clean Code tag.
if [[ "$(git rev-parse --show-toplevel)" == "$(cd "$repo_root" && pwd -P)" ]]; then
  revision_args=(--expected-revision "$(git rev-parse HEAD)")
fi
python3 "$repo_root/scripts/check_shipped_tui.py" \
  --binary "$pager/target/debug/a3s-code-tui" \
  --expected-version "$expected_version" \
  "${revision_args[@]}"
