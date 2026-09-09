#!/usr/bin/env bash
# Stage only the Flow step identity digest-fold publish surface.
# Does not commit. From crates/code: ./scripts/stage-digest-fold.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

files=(
  CHANGELOG.md
  ROADMAP.md
  README.md
  README.zh-CN.md
  core/Cargo.toml
  core/src/dynamic_workflow.rs
  core/src/dynamic_workflow/tests.rs
  Cargo.lock
  sdk/node/Cargo.toml
  sdk/node/Cargo.lock
  sdk/node/package.json
  sdk/python/Cargo.toml
  sdk/python/Cargo.lock
  sdk/python/CHANGELOG.md
  sdk/python/pyproject.toml
  sdk/python-bootstrap/pyproject.toml
  sdk/python-bootstrap/src/a3s_code/_bootstrap.py
  sdk/go/bridge/Cargo.toml
  scripts/stage-digest-fold.sh
  scripts/check_release_versions.sh
)

for f in "${files[@]}"; do
  if [[ ! -f "$f" ]]; then
    echo "error: missing $f" >&2
    exit 1
  fi
done

echo "=== digest-fold unit tests ==="
cargo test -p a3s-code-core --features dynamic-workflow --lib \
  dynamic_flow_step_identity_ --quiet

echo "=== release version consistency ==="
./scripts/check_release_versions.sh 8.5.2

git add -- "${files[@]}"
git status --short -- "${files[@]}"
echo
echo "Staged digest-fold 8.5.2 surface. Review, then commit when authorized."
echo "Suggested subject:"
echo "  release: a3s-code-core 8.5.2 digest-fold for DeepResearch multi-source"
echo
echo "After push + crates.io publish, in crates/cli:"
echo "  # bump Cargo.toml version pin to =8.5.2 then:"
echo "  ./scripts/bump-core-digest-fold-pin.sh <published-rev>"
echo "  ./scripts/verify-capability-regression.sh --require-published"
echo "  # then commit CLI pin when authorized"
