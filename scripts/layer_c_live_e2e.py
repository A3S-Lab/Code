#!/usr/bin/env python3
"""Run the existing serial Layer C campaign and reconcile every ACL receipt.

This is suite orchestration only. Agent execution remains in the existing
product tests; this runner adds no harness, model loop, or configuration parser.
"""

import argparse
import datetime
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import subprocess
import sys
import time


CODE_ROOT = Path(__file__).resolve().parent.parent
REPO_ROOT = CODE_ROOT.parent.parent
READER_MANIFEST = CODE_ROOT / "scripts/layer_c_manifest/Cargo.toml"
SUMMARY = re.compile(
    r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;"
    r" (\d+) measured; (\d+) filtered out;.*$",
    re.MULTILINE,
)
LIST_TEST = re.compile(r"^(.+): test$", re.MULTILINE)


def write_text(path, value):
    temporary = path.with_suffix(path.suffix + f".tmp.{os.getpid()}")
    temporary.write_text(value)
    temporary.replace(path)


def write_json(path, value):
    write_text(path, json.dumps(value, indent=2, sort_keys=True) + "\n")


def command(args, log, env):
    """Keep exact argv and raw output for each independent phase/attempt."""
    start = time.monotonic()
    with log.open("w") as output:
        output.write("COMMAND " + shlex.join(str(arg) for arg in args) + "\n")
        output.flush()
        try:
            process = subprocess.Popen(
                args, cwd=CODE_ROOT, env=env, stdout=output, stderr=subprocess.STDOUT,
                start_new_session=os.name != "nt",
            )
            try:
                code = process.wait()
            except KeyboardInterrupt:
                # Terminating the runner must not leave a model campaign alive.
                try:
                    if os.name != "nt":
                        os.killpg(process.pid, signal.SIGTERM)
                    else:
                        process.terminate()
                except ProcessLookupError:
                    pass
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    if os.name != "nt":
                        os.killpg(process.pid, signal.SIGKILL)
                    else:
                        process.kill()
                    process.wait()
                raise
        except OSError as error:
            output.write(f"COMMAND_ERROR {error}\n")
            code = 127
    return code, round(time.monotonic() - start, 3), log.read_text(errors="replace")


def manifest_reader(explicit, evidence, env):
    if explicit:
        return str(Path(explicit).resolve())
    target = CODE_ROOT / "target/layer-c-manifest"
    args = [
        "rustup", "run", "1.97.1", "cargo", "build", "--locked", "--manifest-path",
        str(READER_MANIFEST), "--target-dir", str(target),
    ]
    code, _, _ = command(args, evidence / "manifest-reader-build.log", env)
    if code != 0:
        raise RuntimeError("a3s-acl manifest reader failed to build; see manifest-reader-build.log")
    suffix = ".exe" if os.name == "nt" else ""
    return str(target / "debug" / ("a3s-code-layer-c-manifest" + suffix))


def read_manifest(reader, manifest, evidence, env):
    # ACL is parsed only by the a3s-acl reader. This TSV is its validated process
    # protocol, not another configuration language.
    result = subprocess.run([reader, str(manifest)], env=env, capture_output=True, text=True)
    (evidence / "manifest-reader.log").write_text(result.stderr)
    if result.returncode != 0:
        raise RuntimeError("ACL manifest refused; see manifest-reader.log")
    suites = []
    for row in result.stdout.splitlines():
        name, package, track, features, minimum = row.split("\t")
        suites.append({
            "suite": name, "package": package, "track": track,
            "features": features.split(",") if features else [],
            "minimum_tests": int(minimum),
        })
    if not suites or len({suite["suite"] for suite in suites}) != len(suites):
        raise RuntimeError("manifest reader did not emit a complete unique suite contract")
    return suites


def environment(evidence):
    env = os.environ.copy()
    config = env.get("A3S_CONFIG_FILE") or str(REPO_ROOT / ".a3s/config.acl")
    env["A3S_CONFIG_FILE"] = str(Path(config).expanduser().resolve())
    env.setdefault("A3S_TEST_MODEL", "boyue/deepseek-v4-flash")
    # Preserve the existing Linux sandbox's explicit Node toolchain exposure.
    nvm = Path(env.get("NVM_DIR", str(Path.home() / ".nvm")))
    if nvm.is_dir():
        env.setdefault("NVM_DIR", str(nvm))
        bins = list((nvm / "versions/node").glob("*/bin"))
        if bins:
            def version(path):
                return tuple(int(part) if part.isdigit() else 0 for part in path.parent.name.lstrip("v").split("."))
            env["PATH"] = str(max(bins, key=version)) + os.pathsep + env.get("PATH", "")
    # Paths and model identity only; never echo provider configuration/credentials.
    write_json(evidence / "env.json", {
        key: env.get(key, "") for key in ("A3S_CONFIG_FILE", "A3S_TEST_MODEL", "NVM_DIR")
    })
    return env


def metadata(cargo, evidence, env):
    args = [cargo, "metadata", "--no-deps", "--format-version=1"]
    start = time.monotonic()
    try:
        result = subprocess.run(args, cwd=CODE_ROOT, env=env, capture_output=True, text=True)
    except OSError as error:
        (evidence / "metadata.log").write_text(f"COMMAND {shlex.join(args)}\n{error}\n")
        raise RuntimeError("cargo metadata could not start") from error
    (evidence / "metadata.log").write_text(
        "COMMAND " + shlex.join(args) + "\n" + result.stderr + result.stdout
    )
    if result.returncode != 0:
        raise RuntimeError("cargo metadata failed")
    data = json.loads(result.stdout)
    targets = {}
    for package in data["packages"]:
        targets[package["name"]] = {
            target["name"]: target for target in package["targets"] if "test" in target["kind"]
        }
    return targets, round(time.monotonic() - start, 3)


def initial_receipt(suite):
    return {
        **suite, "status": "not_run", "preflight": None, "attempts": [],
        "first_result": None, "final_result": "not_run", "discovered_tests": [],
    }


def failure(receipt, status, detail):
    receipt.update(status=status, first_result=status, final_result=status, detail=detail)
    return receipt


def run_suite(suite, targets, cargo, evidence, env, receipt):
    name = suite["suite"]
    target = targets.get(suite["package"], {}).get(name)
    if target is None:
        return failure(receipt, "target_missing", "cargo metadata has no named integration-test target")
    missing_features = set(target.get("required-features", [])) - set(suite["features"])
    if missing_features:
        return failure(receipt, "feature_missing", "manifest omits target required-features: " + ",".join(sorted(missing_features)))
    base = [cargo, "test", "-p", suite["package"]]
    if suite["features"]:
        base += ["--features", ",".join(suite["features"])]
    base += ["--test", name]
    preflight_log = evidence / f"{name}.preflight.log"
    code, duration, output = command(base + ["--", "--ignored", "--list"], preflight_log, env)
    receipt["preflight"] = {"exit_code": code, "seconds": duration, "log": preflight_log.name}
    if code != 0:
        return failure(receipt, "compile_failed", "compilation/discovery phase failed")
    discovered = LIST_TEST.findall(output)
    receipt["discovered_tests"] = discovered
    if len(discovered) < suite["minimum_tests"] or len(set(discovered)) != len(discovered):
        return failure(receipt, "discovery_failed", f"discovered {len(discovered)} unique live tests; minimum {suite['minimum_tests']}")
    for number in (1, 2):
        log = evidence / f"{name}.attempt-{number}.log"
        code, duration, output = command(
            base + ["--", "--ignored", "--test-threads=1", "--nocapture"], log, env
        )
        summaries = SUMMARY.findall(output)
        # One integration-test target must produce exactly one libtest summary.
        # Exit zero alone is insufficient: process::exit(0), zero selected tests,
        # ignored tests and a truncated transcript all leave failed evidence.
        counts = None
        status = "execution_failed"
        if len(summaries) == 1:
            verdict, passed, failed, ignored, measured, filtered = summaries[0]
            counts = dict(zip(
                ("passed", "failed", "ignored", "measured", "filtered"),
                map(int, (passed, failed, ignored, measured, filtered)),
            ))
            if code == 0 and verdict == "ok" and counts["passed"] == len(discovered) and counts["failed"] == counts["ignored"] == counts["measured"] == 0:
                status = "passed"
            elif code == 0:
                status = "execution_count_mismatch"
        elif code == 0:
            status = "execution_summary_missing"
        attempt = {"number": number, "status": status, "exit_code": code,
                   "seconds": duration, "counts": counts, "log": log.name}
        receipt["attempts"].append(attempt)
        if number == 1:
            receipt["first_result"] = status
        receipt["final_result"] = status
        if status == "passed":
            receipt["status"] = "passed" if number == 1 else "passed_after_retry"
            return receipt
        print(f"ATTEMPT_FAILED {name} attempt={number} status={status}", flush=True)
    receipt["status"] = receipt["final_result"]
    return receipt


def reconcile(suites, evidence):
    expected = {(suite["package"], suite["suite"]): suite for suite in suites}
    receipts = []
    errors = []
    for path in sorted((evidence / "receipts").glob("*.json")):
        try:
            receipt = json.loads(path.read_text())
            if not isinstance(receipt, dict) or not {"package", "suite", "track", "status", "first_result", "final_result", "attempts"}.issubset(receipt):
                raise ValueError("incomplete receipt schema")
            receipts.append(receipt)
        except (OSError, ValueError) as error:
            errors.append(f"{path.name}: {error}")
    actual = [(receipt["package"], receipt["suite"]) for receipt in receipts]
    integrity = not errors and set(actual) == set(expected) and len(actual) == len(expected)
    # A receipt cannot promote a measurement or alter its declared feature floor.
    for receipt in receipts:
        contract = expected.get((receipt["package"], receipt["suite"]))
        if contract is None or any(receipt.get(key) != value for key, value in contract.items()):
            integrity = False
    counts = {}
    for track in ("mandatory", "measurement"):
        subset = [receipt for receipt in receipts if receipt["track"] == track]
        counts[track] = {
            "expected": sum(suite["track"] == track for suite in suites),
            "receipts": len(subset),
            "first_pass": sum(receipt["first_result"] == "passed" for receipt in subset),
            "first_failed": sum(receipt["first_result"] != "passed" for receipt in subset),
            "passed_after_retry": sum(receipt["status"] == "passed_after_retry" for receipt in subset),
            "retry_attempted": sum(len(receipt["attempts"]) > 1 for receipt in subset),
            "retry_failed": sum(len(receipt["attempts"]) > 1 and receipt["final_result"] != "passed" for receipt in subset),
            "failed": sum(receipt["final_result"] != "passed" for receipt in subset),
            "executed_tests_first_attempt": sum((receipt["attempts"][0]["counts"] or {}).get("passed", 0) + (receipt["attempts"][0]["counts"] or {}).get("failed", 0) for receipt in subset if receipt["attempts"]),
            "executed_tests_retry": sum((attempt["counts"] or {}).get("passed", 0) + (attempt["counts"] or {}).get("failed", 0) for receipt in subset for attempt in receipt["attempts"][1:]),
        }
    return {"receipt_integrity": integrity, "receipt_errors": errors, **counts}, receipts


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=CODE_ROOT / "scripts/layer_c_suites.acl")
    parser.add_argument("--manifest-reader", help="prebuilt a3s-acl reader (used by the hermetic fault matrix)")
    args = parser.parse_args()
    base = Path(os.environ.get("A3S_LAYER_C_EVIDENCE", "/tmp/a3s-layer-c-live")).expanduser().resolve()
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
    evidence = base / f"run-{stamp}-{os.getpid()}"
    evidence.mkdir(parents=True)
    (evidence / "receipts").mkdir()
    # Existing automation reads base/FINAL.txt. Invalidate any previous green
    # immediately; the retained per-run directory remains authoritative.
    write_text(base / "FINAL.txt", f"LAYER_C_IN_PROGRESS evidence={evidence}\n")
    print(f"LAYER_C_EVIDENCE={evidence}", flush=True)
    env = environment(evidence)
    suites = []
    recorded = set()
    campaign_error = None
    active_receipt = None
    cargo = env.get("A3S_LAYER_C_CARGO", "cargo")
    try:
        manifest = args.manifest.expanduser().absolute()
        reader = manifest_reader(args.manifest_reader, evidence, env)
        suites = read_manifest(reader, manifest, evidence, env)
        # Retain the exact source contract and validated expected list.
        shutil.copyfile(manifest, evidence / "suites.acl")
        write_json(evidence / "expected.json", suites)
        targets, _ = metadata(cargo, evidence, env)
        for suite in suites:
            active_receipt = initial_receipt(suite)
            receipt = run_suite(suite, targets, cargo, evidence, env, active_receipt)
            write_json(evidence / "receipts" / (suite["suite"] + ".json"), receipt)
            recorded.add(suite["suite"])
            label = "MANDATORY" if suite["track"] == "mandatory" else "MEASUREMENT"
            print(f"{label} {suite['suite']} status={receipt['status']}", flush=True)
    except (OSError, ValueError, KeyError, RuntimeError, KeyboardInterrupt) as error:
        campaign_error = str(error) or "interrupted"
        print(f"CAMPAIGN_ERROR {campaign_error}", file=sys.stderr, flush=True)
    finally:
        # A startup failure/interruption still leaves a terminal receipt for every
        # validated suite, rather than presenting a smaller successful campaign.
        for suite in suites:
            if suite["suite"] not in recorded:
                receipt = initial_receipt(suite)
                if active_receipt is not None and active_receipt["suite"] == suite["suite"]:
                    receipt = active_receipt
                    number = len(receipt["attempts"]) + 1
                    log = evidence / f"{suite['suite']}.attempt-{number}.log"
                    if log.exists():
                        receipt["attempts"].append({
                            "number": number, "status": "interrupted", "exit_code": None,
                            "seconds": None, "counts": None, "log": log.name,
                        })
                    receipt.update(status="interrupted", final_result="interrupted",
                                   first_result=receipt["first_result"] or "interrupted",
                                   detail=campaign_error or "missing suite receipt")
                else:
                    receipt = failure(receipt, "not_run", campaign_error or "missing suite receipt")
                write_json(evidence / "receipts" / (suite["suite"] + ".json"), receipt)
    summary, receipts = reconcile(suites, evidence)
    success = bool(suites) and campaign_error is None and summary["receipt_integrity"] and summary["mandatory"]["failed"] == 0
    summary.update(result="LAYER_C_PASS" if success else "LAYER_C_FAIL",
                   model=env["A3S_TEST_MODEL"], campaign_error=campaign_error)
    write_json(evidence / "summary.json", summary)
    lines = [f"{receipt['track'].upper()} {receipt['suite']} first={receipt['first_result']} final={receipt['status']} attempts={len(receipt['attempts'])}" for receipt in receipts]
    lines.append(json.dumps(summary, sort_keys=True))
    (evidence / "summary.txt").write_text("\n".join(lines) + "\n")
    final = f"{summary['result']} model={env['A3S_TEST_MODEL']} mandatory={summary['mandatory']} measurement={summary['measurement']} receipt_integrity={summary['receipt_integrity']} evidence={evidence}"
    write_text(evidence / "FINAL.txt", final + "\n")
    write_text(base / "FINAL.txt", final + "\n")
    print(final, flush=True)
    return 0 if success else 1


if __name__ == "__main__":
    def terminate(_signal, _frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, terminate)
    sys.exit(main())
