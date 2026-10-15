#!/usr/bin/env python3
"""Hermetic boundary faults for Layer C; the real a3s-acl reader is used."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest


ROOT = Path(__file__).resolve().parent.parent
RUNNER = ROOT / "scripts/layer_c_live_e2e.py"
READER_TARGET = ROOT / "target/layer-c-manifest"
READER = READER_TARGET / "debug/a3s-code-layer-c-manifest"

CONTRACT = '''
suite "core_live" {
  package = "a3s-code-core"
  track = "mandatory"
  minimum_tests = 1
}
suite "feature_live" {
  package = "a3s-code-core"
  track = "mandatory"
  features = ["research"]
  minimum_tests = 1
}
suite "acp_live" {
  package = "a3s-code-acp"
  track = "mandatory"
  minimum_tests = 1
}
suite "cache_measurement" {
  package = "a3s-code-core"
  track = "measurement"
  minimum_tests = 1
}
suite "injection_measurement" {
  package = "a3s-code-core"
  track = "measurement"
  minimum_tests = 1
}
'''

FAKE_CARGO = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys

args = sys.argv[1:]
scenario = os.environ['FAULT_SCENARIO']
state = Path(os.environ['FAULT_STATE'])
with (state / 'invocations').open('a') as output:
    output.write(json.dumps(args) + '\n')
if args[0] == 'metadata':
    if scenario == 'metadata_failure':
        print('error: manifest unavailable', file=sys.stderr)
        sys.exit(101)
    packages = []
    for package, names in [
        ('a3s-code-core', ['core_live', 'feature_live', 'cache_measurement', 'injection_measurement']),
        ('a3s-code-acp', ['acp_live']),
    ]:
        targets = []
        for name in names:
            if scenario == 'missing_target' and name == 'core_live':
                continue
            targets.append({'name': name, 'kind': ['test'],
                            'required-features': ['research'] if name == 'feature_live' else []})
        packages.append({'name': package, 'targets': targets})
    print(json.dumps({'packages': packages}))
    sys.exit(0)

suite = args[args.index('--test') + 1]
if suite == 'feature_live' and ('--features' not in args or args[args.index('--features') + 1] != 'research'):
    print('required features were not selected', file=sys.stderr)
    sys.exit(101)
if '--list' in args:
    if (scenario == 'compile_failure' and suite == 'feature_live') or (scenario == 'measurement_compile_failure' and suite == 'cache_measurement'):
        print('error[E0000]: injected compile failure', file=sys.stderr)
        sys.exit(101)
    if scenario == 'zero_discovered' and suite == 'core_live':
        print('0 tests, 0 benchmarks')
    else:
        print(suite + '_proves_behavior: test')
        print('1 test, 0 benchmarks')
    sys.exit(0)

assert '--ignored' in args and '--test-threads=1' in args and '--nocapture' in args
counter = state / (suite + '.attempts')
number = int(counter.read_text()) + 1 if counter.exists() else 1
counter.write_text(str(number))
if scenario == 'interrupt' and suite == 'core_live':
    import time
    (state / 'child.pid').write_text(str(os.getpid()))
    time.sleep(60)
fault = suite == 'core_live'
if scenario == 'retry_success' and fault and number == 1:
    print('test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s')
    sys.exit(101)
if (scenario == 'execution_failure' and fault) or (scenario == 'measurement_execution_failure' and suite == 'cache_measurement'):
    print('test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s')
    sys.exit(101)
if scenario == 'zero_executed' and fault:
    print('test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.01s')
    sys.exit(0)
if scenario == 'early_exit' and fault:
    print('test process exited before proving a test')
    sys.exit(0)
if scenario == 'ignored_execution' and fault:
    print('test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s')
    sys.exit(0)
if scenario == 'count_mismatch' and fault:
    print('test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s')
    sys.exit(0)
if scenario == 'summary_spoof' and fault:
    print('test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s')
    print('test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.01s')
    sys.exit(0)
if suite == 'injection_measurement':
    # Storage faults after the first suite has really produced its receipt.
    for run in Path(os.environ['A3S_LAYER_C_EVIDENCE']).glob('run-*'):
        original = run / 'receipts/core_live.json'
        if scenario == 'receipt_missing':
            original.unlink()
        if scenario == 'receipt_duplicate':
            (run / 'receipts/duplicate.json').write_text(original.read_text())
        if scenario == 'receipt_corrupt':
            original.write_text('truncated receipt')
print('test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s')
if scenario == 'nonzero_despite_summary' and fault:
    sys.exit(101)
'''


class LayerCFaultMatrix(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        result = subprocess.run([
            "rustup", "run", "1.97.1", "cargo", "build", "--locked", "--offline",
            "--manifest-path", str(ROOT / "scripts/layer_c_manifest/Cargo.toml"),
            "--target-dir", str(READER_TARGET),
        ], cwd=ROOT, capture_output=True, text=True)
        if result.returncode:
            raise RuntimeError("a3s-acl reader build failed:\n" + result.stdout + result.stderr)

    def campaign(self, scenario="pass", contract=CONTRACT):
        with tempfile.TemporaryDirectory(prefix="a3s-layer-c-fault-") as directory:
            root = Path(directory)
            fake = root / "cargo"
            fake.write_text(FAKE_CARGO)
            fake.chmod(0o755)
            manifest = root / "suites.acl"
            manifest.write_text(contract)
            evidence = root / "evidence"
            evidence.mkdir()
            (evidence / "FINAL.txt").write_text("LAYER_C_PASS stale_previous_run\n")
            env = {
                **os.environ, "A3S_LAYER_C_CARGO": str(fake),
                "A3S_LAYER_C_EVIDENCE": str(evidence), "FAULT_SCENARIO": scenario,
                "FAULT_STATE": str(root), "A3S_CONFIG_FILE": str(root / "not-read.acl"),
                "A3S_TEST_MODEL": "fixture/model",
            }
            result = subprocess.run([
                sys.executable, str(RUNNER), "--manifest", str(manifest),
                "--manifest-reader", str(READER),
            ], cwd=ROOT, env=env, capture_output=True, text=True, timeout=30)
            runs = list(evidence.glob("run-*"))
            self.assertEqual(len(runs), 1, result.stdout + result.stderr)
            run = runs[0]
            summary = json.loads((run / "summary.json").read_text())
            receipts = {path.stem: json.loads(path.read_text()) for path in (run / "receipts").glob("*.json")}
            invocations = [json.loads(line) for line in (root / "invocations").read_text().splitlines()] if (root / "invocations").exists() else []
            logs = {path.name: path.read_text() for path in run.glob("*.log")}
            final = (run / "FINAL.txt").read_text()
            self.assertEqual((evidence / "FINAL.txt").read_text(), final)
            self.assertNotIn("stale_previous_run", final)
            return result, summary, receipts, invocations, logs, final

    def assert_failure(self, scenario, status, retries=0):
        result, summary, receipts, _, _, final = self.campaign(scenario)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual(summary["result"], "LAYER_C_FAIL")
        self.assertNotIn("LAYER_C_PASS", final)
        self.assertTrue(summary["receipt_integrity"])
        self.assertEqual(len(receipts), 5)
        name = "feature_live" if scenario == "compile_failure" else "core_live"
        self.assertEqual(receipts[name]["status"], status)
        self.assertEqual(len(receipts[name]["attempts"]), retries)
        self.assertEqual(receipts["acp_live"]["final_result"], "passed")

    def test_complete_functional_and_measurement_campaign(self):
        result, summary, receipts, invocations, _, _ = self.campaign()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(summary["mandatory"]["first_pass"], 3)
        self.assertEqual(summary["measurement"]["first_pass"], 2)
        self.assertEqual(summary["mandatory"]["executed_tests_first_attempt"], 3)
        self.assertEqual(len(receipts), 5)
        self.assertTrue(summary["receipt_integrity"])
        self.assertTrue(any("a3s-code-acp" in args for args in invocations))

    def test_missing_target_fails_and_all_suites_keep_receipts(self):
        self.assert_failure("missing_target", "target_missing")

    def test_required_feature_compile_failure_cannot_skip_or_reset_failure(self):
        self.assert_failure("compile_failure", "compile_failed")

    def test_zero_discovered_tests_cannot_pass(self):
        self.assert_failure("zero_discovered", "discovery_failed")

    def test_a_two_case_mandatory_suite_refuses_only_one_discovered_test(self):
        result, summary, receipts, invocations, _, _ = self.campaign(
            contract=CONTRACT.replace("minimum_tests = 1", "minimum_tests = 2", 1)
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(summary["result"], "LAYER_C_FAIL")
        self.assertEqual(receipts["core_live"]["status"], "discovery_failed")
        self.assertFalse(any(
            "core_live" in invocation and "--list" not in invocation
            for invocation in invocations
        ))

    def test_zero_executed_tests_cannot_pass(self):
        self.assert_failure("zero_executed", "execution_count_mismatch", retries=2)

    def test_exit_zero_without_libtest_summary_cannot_pass(self):
        self.assert_failure("early_exit", "execution_summary_missing", retries=2)

    def test_ignored_tests_do_not_count_as_execution(self):
        self.assert_failure("ignored_execution", "execution_count_mismatch", retries=2)

    def test_execution_failure_remains_failed_after_bounded_retry(self):
        self.assert_failure("execution_failure", "execution_failed", retries=2)

    def test_nonzero_exit_cannot_borrow_success_from_a_summary(self):
        self.assert_failure("nonzero_despite_summary", "execution_failed", retries=2)

    def test_execution_count_must_match_discovered_live_tests(self):
        self.assert_failure("count_mismatch", "execution_count_mismatch", retries=2)

    def test_two_summaries_cannot_spoof_execution_evidence(self):
        self.assert_failure("summary_spoof", "execution_summary_missing", retries=2)

    def test_first_failure_and_retry_success_are_separate_receipts(self):
        result, summary, receipts, _, logs, _ = self.campaign("retry_success")
        self.assertEqual(result.returncode, 0)
        receipt = receipts["core_live"]
        self.assertEqual(receipt["first_result"], "execution_failed")
        self.assertEqual(receipt["status"], "passed_after_retry")
        self.assertEqual(summary["mandatory"]["first_pass"], 2)
        self.assertEqual(summary["mandatory"]["passed_after_retry"], 1)
        self.assertEqual(summary["mandatory"]["first_failed"], 1)
        self.assertEqual(summary["mandatory"]["retry_attempted"], 1)
        self.assertEqual(summary["mandatory"]["retry_failed"], 0)
        self.assertEqual(summary["mandatory"]["executed_tests_retry"], 1)
        self.assertIn("FAILED", logs["core_live.attempt-1.log"])
        self.assertIn("test result: ok", logs["core_live.attempt-2.log"])

    def test_measurement_compile_failure_is_visible_and_nonblocking(self):
        result, summary, receipts, _, _, final = self.campaign("measurement_compile_failure")
        self.assertEqual(result.returncode, 0)
        self.assertEqual(receipts["cache_measurement"]["status"], "compile_failed")
        self.assertEqual(summary["measurement"]["failed"], 1)
        self.assertIn("'failed': 1", final)

    def test_measurement_execution_failure_is_visible_and_nonblocking(self):
        result, summary, receipts, _, _, _ = self.campaign("measurement_execution_failure")
        self.assertEqual(result.returncode, 0)
        self.assertEqual(len(receipts["cache_measurement"]["attempts"]), 2)
        self.assertEqual(summary["measurement"]["failed"], 1)

    def test_metadata_failure_has_a_not_run_receipt_for_every_suite(self):
        result, summary, receipts, _, _, _ = self.campaign("metadata_failure")
        self.assertEqual(result.returncode, 1)
        self.assertTrue(summary["receipt_integrity"])
        self.assertEqual(len(receipts), 5)
        self.assertTrue(all(receipt["status"] == "not_run" for receipt in receipts.values()))

    def test_missing_terminal_receipt_blocks_certification(self):
        result, summary, _, _, _, _ = self.campaign("receipt_missing")
        self.assertEqual(result.returncode, 1)
        self.assertFalse(summary["receipt_integrity"])

    def test_duplicate_terminal_receipt_blocks_certification(self):
        result, summary, _, _, _, _ = self.campaign("receipt_duplicate")
        self.assertEqual(result.returncode, 1)
        self.assertFalse(summary["receipt_integrity"])

    def test_corrupt_terminal_receipt_blocks_certification_with_a_final_record(self):
        # campaign() intentionally reads every JSON receipt. Corrupt evidence is
        # read separately here so its absence from the reconciled set is tested.
        with tempfile.TemporaryDirectory(prefix="a3s-layer-c-corrupt-") as directory:
            root = Path(directory)
            fake = root / "cargo"
            fake.write_text(FAKE_CARGO)
            fake.chmod(0o755)
            manifest = root / "suites.acl"
            manifest.write_text(CONTRACT)
            env = {**os.environ, "A3S_LAYER_C_CARGO": str(fake),
                   "A3S_LAYER_C_EVIDENCE": str(root / "evidence"),
                   "FAULT_SCENARIO": "receipt_corrupt", "FAULT_STATE": str(root)}
            result = subprocess.run([sys.executable, str(RUNNER), "--manifest", str(manifest),
                                     "--manifest-reader", str(READER)], cwd=ROOT, env=env,
                                    capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            run = next((root / "evidence").glob("run-*"))
            summary = json.loads((run / "summary.json").read_text())
            self.assertFalse(summary["receipt_integrity"])
            self.assertEqual(len(summary["receipt_errors"]), 1)
            self.assertIn("LAYER_C_FAIL", (run / "FINAL.txt").read_text())

    def test_missing_feature_declaration_fails_before_execution(self):
        result, summary, receipts, _, _, _ = self.campaign(contract=CONTRACT.replace('features = ["research"]', 'features = []'))
        self.assertEqual(result.returncode, 1)
        self.assertEqual(receipts["feature_live"]["status"], "feature_missing")
        self.assertTrue(summary["receipt_integrity"])

    def test_acl_unknown_track_refuses_the_whole_contract(self):
        result, summary, receipts, invocations, _, _ = self.campaign(contract=CONTRACT.replace('track = "mandatory"', 'track = "optional"', 1))
        self.assertEqual(result.returncode, 1)
        self.assertEqual(receipts, {})
        self.assertEqual(invocations, [])
        self.assertIn("ACL manifest refused", summary["campaign_error"])

    def test_acl_duplicate_suite_refuses_the_whole_contract(self):
        result, _, receipts, invocations, _, _ = self.campaign(contract=CONTRACT + CONTRACT)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(receipts, {})
        self.assertEqual(invocations, [])

    def test_acl_zero_minimum_refuses_the_whole_contract(self):
        result, _, _, invocations, _, _ = self.campaign(contract=CONTRACT.replace("minimum_tests = 1", "minimum_tests = 0", 1))
        self.assertEqual(result.returncode, 1)
        self.assertEqual(invocations, [])

    def test_acl_reader_bounds_core_dependency_feature_qualification(self):
        with tempfile.TemporaryDirectory(prefix="a3s-layer-c-features-") as directory:
            path = Path(directory) / "features.acl"
            feature = "a3s-code-core/deep-live"
            path.write_text(CONTRACT.replace('features = ["research"]', f'features = ["{feature}"]'))
            result = subprocess.run([str(READER), str(path)], capture_output=True, text=True, check=True)
            self.assertIn(f"\t{feature}\t", result.stdout)
            for invalid in ("other/deep-live", "a3s-code-core/", "/deep-live",
                            "a3s-code-core/x/y", "a3s-code-core/../x", "a3s-code-core/x;echo"):
                with self.subTest(feature=invalid):
                    path.write_text(CONTRACT.replace('features = ["research"]', f'features = ["{invalid}"]'))
                    refused = subprocess.run([str(READER), str(path)], capture_output=True, text=True)
                    self.assertEqual(refused.returncode, 1)
                    self.assertEqual(refused.stdout, "")
            path.write_text(CONTRACT.replace('features = ["research"]', f'features = ["{feature}", "{feature}"]'))
            refused = subprocess.run([str(READER), str(path)], capture_output=True, text=True)
            self.assertEqual(refused.returncode, 1)
            self.assertEqual(refused.stdout, "")

    def test_acl_reader_refuses_symlink_and_fifo_before_opening(self):
        with tempfile.TemporaryDirectory(prefix="a3s-layer-c-input-") as directory:
            root = Path(directory)
            regular = root / "regular.acl"
            regular.write_text(CONTRACT)
            link = root / "link.acl"
            link.symlink_to(regular)
            paths = [link]
            if hasattr(os, "mkfifo"):
                fifo = root / "fifo.acl"
                os.mkfifo(fifo)
                paths.append(fifo)
            for path in paths:
                result = subprocess.run([str(READER), str(path)], capture_output=True, text=True, timeout=2)
                self.assertEqual(result.returncode, 1)
                self.assertEqual(result.stdout, "")
                self.assertIn("regular file", result.stderr)

    @unittest.skipIf(os.name == "nt", "POSIX process-group cancellation boundary")
    def test_interrupted_campaign_stops_its_child_and_keeps_all_terminal_receipts(self):
        with tempfile.TemporaryDirectory(prefix="a3s-layer-c-interrupt-") as directory:
            root = Path(directory)
            fake = root / "cargo"
            fake.write_text(FAKE_CARGO)
            fake.chmod(0o755)
            manifest = root / "suites.acl"
            manifest.write_text(CONTRACT)
            env = {**os.environ, "A3S_LAYER_C_CARGO": str(fake),
                   "A3S_LAYER_C_EVIDENCE": str(root / "evidence"),
                   "FAULT_SCENARIO": "interrupt", "FAULT_STATE": str(root)}
            process = subprocess.Popen([sys.executable, str(RUNNER), "--manifest", str(manifest),
                                        "--manifest-reader", str(READER)], cwd=ROOT, env=env,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                deadline = time.monotonic() + 10
                while not (root / "child.pid").exists() and time.monotonic() < deadline:
                    time.sleep(0.02)
                self.assertTrue((root / "child.pid").exists())
                child = int((root / "child.pid").read_text())
                process.terminate()
                output, error = process.communicate(timeout=10)
                self.assertEqual(process.returncode, 1, output + error)
                with self.assertRaises(ProcessLookupError):
                    os.kill(child, 0)
                run = next((root / "evidence").glob("run-*"))
                summary = json.loads((run / "summary.json").read_text())
                self.assertEqual(summary["result"], "LAYER_C_FAIL")
                self.assertTrue(summary["receipt_integrity"])
                self.assertEqual(summary["mandatory"]["receipts"], 3)
                receipt = json.loads((run / "receipts/core_live.json").read_text())
                self.assertEqual(receipt["status"], "interrupted")
                self.assertEqual(receipt["attempts"][0]["status"], "interrupted")
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()

    def test_default_contract_registers_previously_omitted_functional_rails(self):
        result = subprocess.run([str(READER), str(ROOT / "scripts/layer_c_suites.acl")], capture_output=True, text=True, check=True)
        rows = {fields[0]: fields for fields in (line.split("\t") for line in result.stdout.splitlines())}
        for name in ("test_background_run_real_llm", "test_worktree_run_real_llm", "test_long_horizon_capabilities_real_llm", "test_multi_patch_real_llm", "test_exact_id_checkpoint_real_llm", "test_skill_mcp_decisions_real_llm", "test_task_speed_real_llm"):
            self.assertEqual(rows[name][2], "mandatory")
        for name in ("test_cache_hit_rate_real_llm", "test_injection_budget_real_llm", "test_parallel_adoption_real_llm"):
            self.assertEqual(rows[name][2], "measurement")
        self.assertEqual(rows["test_research_review_real_llm"][3], "research")
        self.assertEqual(rows["test_extensibility_real_llm"][3], "advanced-harness")
        self.assertEqual(sum(row[2] == "mandatory" for row in rows.values()), 53)
        self.assertEqual(rows["test_engineering_discipline_real_llm"][2], "mandatory")
        self.assertEqual(rows["test_engineering_discipline_real_llm"][4], "8")
        self.assertEqual(rows["test_durable_goal_real_llm"][2], "mandatory")
        self.assertEqual(rows["test_durable_goal_real_llm"][4], "1")
        self.assertEqual(rows["test_durable_engineering_paired_real_llm"][2], "measurement")
        self.assertEqual(rows["test_durable_engineering_paired_real_llm"][4], "5")
        self.assertEqual(rows["tui_913_real_llm"][1], "a3s-code-acp")
        self.assertEqual(rows["tui_913_real_llm"][2], "mandatory")
        self.assertEqual(set(rows["tui_913_real_llm"][3].split(",")),
                         {"a3s-code-core/deep-live", "a3s-code-core/advanced-harness"})
        self.assertEqual(rows["tui_913_real_llm"][4], "10")
        self.assertEqual(rows["test_tool_effect_receipts_real_llm"][2], "mandatory")
        self.assertEqual(rows["test_tool_effect_receipts_real_llm"][4], "2")
        self.assertEqual(sum(row[2] == "measurement" for row in rows.values()), 4)


if __name__ == "__main__":
    unittest.main(verbosity=2)
