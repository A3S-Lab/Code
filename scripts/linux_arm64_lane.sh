#!/usr/bin/env bash
# Spec 018 T038: run Core proofs on native arm64 Linux (no macOS native refusal).
#
# Builds aarch64-unknown-linux-musl test binaries with cargo-zigbuild, then runs
# them in an arm64 Alpine+node container with the repository mounted read-only
# at its host path (compile-time manifest paths stay valid).
#
#   apps/code/scripts/linux_arm64_lane.sh [lib|tests|t028|t029|all] [log-dir]
#
# tests runs every Core integration test binary (non-ignored), one log each.
#
# Real-model suites read keys from scripts/harbor/.env through
# scripts/harbor/native-tb.acl (env("BOYUE_API_KEY")); values are never printed.
set -euo pipefail

MODE="${1:-all}"
REPO="$(cd "$(dirname "$0")/../../.." && pwd)"
CODE="$REPO/apps/code"
LOG_DIR="${2:-/tmp/a3s-linux-lane/$(date +%Y%m%d-%H%M%S)}"
IMAGE="a3s-code-linux-lane:node22"
TARGET="aarch64-unknown-linux-musl"
mkdir -p "$LOG_DIR"

case "$MODE" in
  lib) TEST_TARGETS=(--lib) ;;
  tests|all) TEST_TARGETS=(--tests) ;;
  t028) TEST_TARGETS=(--test test_engineering_discipline_real_llm) ;;
  t029) TEST_TARGETS=(--test test_durable_goal_real_llm) ;;
  *) echo "mode must be lib, tests, t028, t029 or all" >&2; exit 2 ;;
esac
[[ "$(docker info --format '{{.Architecture}}')" == "aarch64" ]] || { echo "needs an arm64 Docker host" >&2; exit 2; }

docker build -q -t "$IMAGE" - >/dev/null <<'EOF'
FROM node:22-alpine
RUN apk add --no-cache bash bubblewrap curl git coreutils findutils grep diffutils python3 rust
EOF

(cd "$CODE" && rustup run 1.97.1 cargo-zigbuild test --locked -p a3s-code-core --target "$TARGET" \
  "${TEST_TARGETS[@]}" --no-run --message-format=json-render-diagnostics >"$LOG_DIR/build.json" 2>"$LOG_DIR/build.log")

binary() {
  python3 - "$LOG_DIR/build.json" "$1" <<'EOF'
import json, sys
path = None
for line in open(sys.argv[1]):
    try:
        m = json.loads(line)
    except ValueError:
        continue
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] == sys.argv[2]:
        path = m["executable"]
if not path:
    sys.exit(f"no test binary for {sys.argv[2]}")
print(path)
EOF
}

integration_binaries() {
  python3 - "$LOG_DIR/build.json" <<'EOF'
import json, sys
for line in open(sys.argv[1]):
    try:
        m = json.loads(line)
    except ValueError:
        continue
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["kind"] == ["test"]:
        print(m["target"]["name"], m["executable"])
EOF
}

ENV_FILE="$(mktemp)"
chmod 600 "$ENV_FILE"
trap 'rm -f "$ENV_FILE"' EXIT
if [[ -f "$REPO/scripts/harbor/.env" ]]; then
  sed -n 's/^[[:space:]]*\(export[[:space:]]\{1,\}\)\{0,1\}\([A-Za-z_][A-Za-z0-9_]*=.*\)$/\2/p' \
    "$REPO/scripts/harbor/.env" | sed "s/=[\"']\(.*\)[\"']$/=\1/" >"$ENV_FILE"
fi

lane_result=0
run() { # run <log> <binary> <args...>
  local log="$1"; shift
  set +e
  # bubblewrap (the Linux native sandbox) needs user namespaces and a mountable
  # /proc; a non-root user keeps permission-denied proofs meaningful.
  docker run --rm --env-file "$ENV_FILE" --user node \
    --security-opt seccomp=unconfined --security-opt apparmor=unconfined --security-opt systempaths=unconfined \
    -e A3S_CONFIG_FILE="$REPO/scripts/harbor/native-tb.acl" -e RUST_BACKTRACE=1 \
    -v "$REPO:$REPO:ro" -w "$CODE/core" "$IMAGE" "$@" >"$LOG_DIR/$log" 2>&1
  local run_status=$?
  set -e
  echo "$log exit=$run_status" | tee -a "$LOG_DIR/summary.txt"
  if [[ "$run_status" != 0 ]]; then
    lane_result=1
  fi
}

real_suite() { # real_suite <test-target> <prefix>
  local bin
  bin="$(binary "$1")"
  docker run --rm -v "$REPO:$REPO:ro" "$IMAGE" "$bin" --ignored --list --format terse \
    | sed -n 's/: test$//p' >"$LOG_DIR/$2-list.txt"
  while read -r name; do
    run "$2-${name//::/__}.log" "$bin" --ignored --exact "$name" --test-threads=1 --nocapture
  done <"$LOG_DIR/$2-list.txt"
}

if [[ "$MODE" == lib || "$MODE" == all ]]; then
  run lib.log "$(binary a3s_code_core)"
fi
if [[ "$MODE" == tests || "$MODE" == all ]]; then
  integration_binaries >"$LOG_DIR/tests-list.txt"
  while read -r name bin; do
    run "tests-$name.log" "$bin"
  done <"$LOG_DIR/tests-list.txt"
fi
if [[ "$MODE" == t028 || "$MODE" == all ]]; then
  real_suite test_engineering_discipline_real_llm t028
fi
if [[ "$MODE" == t029 || "$MODE" == all ]]; then
  real_suite test_durable_goal_real_llm t029
fi
echo "logs: $LOG_DIR"
exit "$lane_result"
