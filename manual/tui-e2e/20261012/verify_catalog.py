#!/usr/bin/env python3
"""Validate this plan's coverage and links; never claim product test execution."""

import collections
import hashlib
import json
from pathlib import Path
import re


def main():
    directory = Path(__file__).resolve().parent
    root = directory.parents[4]
    catalog = json.loads((directory / "catalog.json").read_text())
    inventory = json.loads((directory / "inventory.json").read_text())
    features = catalog["features"]
    cases = [case for feature in features for case in feature["cases"]]
    feature_ids = {feature["id"] for feature in features}
    case_ids = [case["id"] for case in cases]
    checks = {
        "each_feature_at_least_five": all(len(f["cases"]) >= 5 for f in features),
        "unique_feature_ids": len(features) == len(feature_ids),
        "unique_case_ids": len(cases) == len(set(case_ids)),
        "all_cases_planned": all(c["status"] == "PLANNED" for c in cases),
        "no_empty_case_contract": all(
            all(c.get(k) for k in ("precondition", "steps", "oracle", "lane", "failure_rule", "evidence"))
            for c in cases
        ),
        "distinct_stimuli_per_feature": all(
            len({c["steps"][-1] for c in f["cases"]}) == len(f["cases"]) for f in features
        ),
        "each_feature_has_three_role_contracts": all(
            all(f.get(k) for k in ("architecture_lens", "product_lens", "test_lens"))
            for f in features
        ),
        "execution_lanes_are_declared": all(
            f["lane"] in catalog["lanes"]
            and all(c["lane"] in catalog["lanes"] for c in f["cases"])
            for f in features
        ),
        "all_source_paths_exist": all((root / p).is_file() for f in features for p in f["source"]),
        "builtin_and_dynamic_command_coverage": set(catalog["command_coverage"])
        == {c["name"] for c in inventory["commands"]} | {"goal"},
        "settings_coverage": {f["setting_key"] for f in features if "setting_key" in f}
        == {s["key"] for s in inventory["settings"]},
        "action_coverage": set(catalog["action_coverage"]) == {a["id"] for a in inventory["actions"]},
        "unavailable_gates_match_backend_contract": {
            f["command_name"] for f in features if f.get("category") == "unavailable-command"
        } == set(inventory["unavailable_commands"])
        and {
            f["setting_key"] for f in features if f.get("category") == "unavailable-setting"
        } == set(inventory["unavailable_settings"]),
        "all_mappings_have_features": all(
            bool(ids) and set(ids) <= feature_ids
            for mapping in (catalog["command_coverage"], catalog["action_coverage"])
            for ids in mapping.values()
        ),
        "source_tool_catalog_is_mapped": set(catalog["tool_coverage"])
        == {entry["name"] for entry in inventory["tool_entries"]},
        "tool_mappings_have_five_case_features": all(
            ids and all(
                any(f["id"] == id and len(f["cases"]) >= 5 for f in features)
                for id in ids
            )
            for ids in catalog["tool_coverage"].values()
        ),
        "tool_sources_exist_and_are_fingerprinted": all(
            (root / entry["source"]).is_file()
            and entry["source"] in inventory["source_hashes"]
            for entry in inventory["tool_entries"]
        ),
        "case_totals_match": len(cases) == catalog["counts"]["cases"] == 1020
        and len(features) == catalog["counts"]["feature_rows"] == 202,
        "welcome_entry_coverage": set(catalog["welcome_coverage"])
        == {entry["id"] for entry in inventory["welcome_entries"]},
        "welcome_mappings_have_features": all(
            bool(ids) and set(ids) <= feature_ids for ids in catalog["welcome_coverage"].values()
        ),
        "reported_minimum_matches_catalog": min(len(f["cases"]) for f in features)
        == catalog["counts"]["minimum_cases_per_feature"],
        "every_feature_source_has_a_fingerprint": all(
            p in inventory["source_hashes"] for f in features for p in f["source"]
        ),
    }
    registry = root / "apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs"
    checks["builtin_registry_count_matches"] = sum(
        "Arc::new(" in line for line in registry.read_text().split("#[cfg(test)]")[0].splitlines()
    ) == len(inventory["commands"])
    broken_links = []
    markdown_case_ids = []
    for file in directory.glob("*.md"):
        content = file.read_text()
        if file.name.endswith("-cases.md"):
            markdown_case_ids.extend(re.findall(r"\| (TUI-[A-Z0-9]+-\d{2}) \|", content))
        for target in re.findall(r"\]\(([^)]+)\)", content):
            if "://" in target or target.startswith("#"):
                continue
            target = target.split("#")[0]
            if not (file.parent / target).exists():
                broken_links.append({"file": file.name, "target": target})
    checks["markdown_machine_catalog_match"] = collections.Counter(markdown_case_ids) == collections.Counter(case_ids)
    # The validation report is created by this command itself.
    broken_links = [b for b in broken_links if b["target"] != "planning-validation.json"]
    checks["relative_links_exist"] = not broken_links
    source_drift = [
        path for path, digest in inventory["source_hashes"].items()
        if not (root / path).is_file() or hashlib.sha256((root / path).read_bytes()).hexdigest() != digest
    ]
    checks["inventory_source_hashes_still_match"] = not source_drift
    readme = (directory / "README.md").read_text()
    undocumented = sorted({f["fixture"] for f in features if "`" + f["fixture"] + "`" not in readme})
    checks["all_fixtures_documented"] = not undocumented
    report = {
        "kind": "planning_structure_validation_only",
        "date": "2026-10-12",
        "counts": catalog["counts"],
        "checks": checks,
        "broken_links": broken_links,
        "source_drift": source_drift,
        "undocumented_fixtures": undocumented,
        "product_e2e_runs_performed_by_this_validator": 0,
        "limitations": ["static coverage is not runtime qualification", "no implementation or release is certified"],
        "artifacts_sha256": {
            p.name: hashlib.sha256(p.read_bytes()).hexdigest()
            for p in directory.iterdir() if p.is_file() and p.name != "planning-validation.json"
        },
    }
    (directory / "planning-validation.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"counts": catalog["counts"], "checks": checks, "source_drift": source_drift}, ensure_ascii=False, indent=2))
    return 0 if all(checks.values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())
