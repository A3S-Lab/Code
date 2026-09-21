#!/usr/bin/env bash
# F-table kernel line-coverage gate for a3s-code-core.
#
# Authoritative metric (FIRST_PRINCIPLES_TEST_CASES.md):
#   cargo llvm-cov -p a3s-code-core --tests --no-cfg-coverage
# applied to the F01–F30 kernel files below — not crate TOTAL.
#
# Usage:
#   scripts/f_table_coverage.sh              # measure + gate
#   A3S_F_TABLE_MIN_LINE_PCT=95 scripts/f_table_coverage.sh
#   A3S_F_TABLE_EVIDENCE=/tmp/a3s-llvm-cov-f95 scripts/f_table_coverage.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

MIN_PCT="${A3S_F_TABLE_MIN_LINE_PCT:-95}"
EVIDENCE="${A3S_F_TABLE_EVIDENCE:-/tmp/a3s-llvm-cov-f${MIN_PCT}}"
mkdir -p "$EVIDENCE"

# Kernel files implementing F01–F30 (integration-bearing surfaces).
# Keep soak-only / historical unregistered tools out of the gate list.
KERNELS=(
  core/src/sandbox/native.rs
  core/src/agent_api/session_sandbox.rs
  core/src/sandbox/process_host.rs
  core/src/llm/openai/streaming.rs
  core/src/mcp/transport/stdio.rs
  core/src/agent_protocol.rs
  core/src/agent_protocol_host.rs
  core/src/agent_protocol_harness.rs
  core/src/event_protocol.rs
  core/src/harness_loop.rs
  core/src/verification.rs
  core/src/tools/builtin/bash.rs
  core/src/tools/builtin/mod.rs
  core/src/effect_isolation.rs
  core/src/agent/plan_execution.rs
  core/src/tools/builtin/update_plan.rs
  core/src/permissions/interactive.rs
  core/src/loop_checkpoint.rs
  core/src/llm/structured.rs
  core/src/run_control.rs
  core/src/workspace/retrieval/runtime.rs
  core/src/workspace/retrieval/lexical.rs
  core/src/workspace/retrieval/a3s_vec.rs
  core/src/context/mod.rs
  core/src/context/assembler.rs
  core/src/tools/task.rs
  core/src/subagent/loader.rs
  core/src/durable_memory.rs
  core/src/session_checkpoint.rs
  core/src/safety_gate.rs
  core/src/budget.rs
  core/src/ask_user.rs
  core/src/hitl.rs
  core/src/program.rs
  # capability/mod.rs is declarations/re-exports only (no instrumentable lines).
  core/src/capability/value.rs
  core/src/capability/projection.rs
  core/src/content_digest.rs
  core/src/harness_evidence/digest.rs
  core/src/harness_evidence/input.rs
  core/src/harness_evidence/source.rs
  core/src/harness_evidence/tool_request.rs
  core/src/harness_evidence/usage.rs
)

for k in "${KERNELS[@]}"; do
  if [[ ! -f "$k" ]]; then
    echo "MISSING_KERNEL $k" | tee -a "$EVIDENCE/FINAL.txt"
    exit 2
  fi
done

echo "min_line_pct=$MIN_PCT" | tee "$EVIDENCE/env.txt"
echo "kernels=${#KERNELS[@]}" | tee -a "$EVIDENCE/env.txt"
printf '%s\n' "${KERNELS[@]}" > "$EVIDENCE/kernels.txt"

# Instrument with slim debuginfo so large crates link; include integration tests.
export CARGO_INCREMENTAL=0
export RUSTFLAGS="${RUSTFLAGS:-} -C debuginfo=0"

echo "[1/3] Running instrumented tests (lib + integration)…"
# --tests includes integration tests under core/tests (authoritative for this gate).
# Hermetic only: skip ignored live suites (cargo does not run #[ignore] by default).
set +e
cargo llvm-cov -p a3s-code-core --tests --no-cfg-coverage \
  --ignore-filename-regex '(/\.cargo/registry/|/rustc/)' \
  --json --output-path "$EVIDENCE/llvm-cov.json" \
  --summary-only \
  -- --test-threads=8 \
  >"$EVIDENCE/llvm-cov.summary.txt" 2>"$EVIDENCE/llvm-cov.stderr.txt"
COV_RC=$?
set -e
echo "llvm-cov_exit=$COV_RC" | tee -a "$EVIDENCE/env.txt"
if [[ "$COV_RC" -ne 0 ]]; then
  echo "ALL_F_TABLE_KERNELS_GE_${MIN_PCT}_FAIL llvm-cov_exit=$COV_RC" | tee "$EVIDENCE/FINAL.txt"
  tail -n 40 "$EVIDENCE/llvm-cov.stderr.txt" || true
  exit "$COV_RC"
fi

echo "[2/3] Scoring F-table kernels (min ≥ ${MIN_PCT}%)…"
set +e
python3 - "$EVIDENCE" "$MIN_PCT" <<'PY'
import json, sys
from pathlib import Path

evidence = Path(sys.argv[1])
min_pct = float(sys.argv[2])
kernels = [line.strip() for line in (evidence / "kernels.txt").read_text().splitlines() if line.strip()]

summary_path = evidence / "llvm-cov.summary.txt"
json_path = evidence / "llvm-cov.json"

# Prefer JSON file report; fall back to parsing summary text lines.
file_pct: dict[str, float] = {}
if json_path.exists() and json_path.stat().st_size > 0:
    data = json.loads(json_path.read_text())
    # cargo-llvm-cov JSON: data.files[].filename + summary.lines
    for entry in data.get("data", []):
        for f in entry.get("files", []):
            name = f.get("filename") or f.get("name") or ""
            summary = f.get("summary", {}).get("lines", {})
            covered = summary.get("covered", 0)
            count = summary.get("count", 0)
            if count:
                # normalize to repo-relative core/src/...
                norm = name.replace("\\", "/")
                if "core/src/" in norm:
                    norm = norm[norm.index("core/src/") :]
                file_pct[norm] = 100.0 * covered / count
else:
    # Fallback: llvm-cov text summary often looks like:
    # Filename                      Regions    Missed Regions     Cover   ...
    text = summary_path.read_text(errors="replace") if summary_path.exists() else ""
    for line in text.splitlines():
        if "core/src/" not in line:
            continue
        parts = line.split()
        if not parts:
            continue
        # Find a percent token
        pct = None
        for tok in parts:
            if tok.endswith("%"):
                try:
                    pct = float(tok[:-1])
                except ValueError:
                    pass
        name = parts[0].replace("\\", "/")
        if "core/src/" in name:
            name = name[name.index("core/src/") :]
        if pct is not None:
            file_pct[name] = pct

rows = []
missing = []
fails = []
for k in kernels:
    if k not in file_pct:
        missing.append(k)
        continue
    pct = file_pct[k]
    status = "PASS" if pct + 1e-9 >= min_pct else "FAIL"
    rows.append((status, pct, k))
    if status == "FAIL":
        fails.append((pct, k))

report = evidence / "kernel-report.txt"
with report.open("w") as fh:
    fh.write(f"min_line_pct={min_pct}\n")
    fh.write(f"scored={len(rows)} missing={len(missing)} fails={len(fails)}\n")
    for status, pct, k in sorted(rows, key=lambda r: r[1]):
        fh.write(f"{status:4} {pct:6.2f}%  {k}\n")
    for k in missing:
        fh.write(f"MISS   -----  {k}\n")

final = evidence / "FINAL.txt"
if missing:
    final.write_text(
        f"ALL_F_TABLE_KERNELS_GE_{int(min_pct)}_FAIL missing={len(missing)} "
        f"fails={len(fails)} scored={len(rows)}\n"
        f"see {report}\n"
    )
    print(final.read_text())
    sys.exit(3)

if fails:
    final.write_text(
        f"ALL_F_TABLE_KERNELS_GE_{int(min_pct)}_FAIL fails={len(fails)} scored={len(rows)}\n"
        f"worst={fails[0][1]} {fails[0][0]:.2f}%\n"
        f"see {report}\n"
    )
    print(final.read_text())
    for pct, k in sorted(fails)[:20]:
        print(f"FAIL {pct:6.2f}%  {k}")
    sys.exit(1)

final.write_text(
    f"ALL_F_TABLE_KERNELS_GE_{int(min_pct)} scored={len(rows)} min_line_pct={min_pct}\n"
    f"see {report}\n"
)
print(final.read_text())
print(f"PASS all {len(rows)} kernels ≥ {min_pct}%")
PY
SCORE_RC=$?
set -e

echo "[3/3] Done → $EVIDENCE/FINAL.txt"
exit "$SCORE_RC"
