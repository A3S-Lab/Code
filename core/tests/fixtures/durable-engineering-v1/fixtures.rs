#![cfg(test)]

use std::path::Path;

pub struct Fixture {
    pub id: &'static str,
    pub request: &'static str,
    pub files: &'static [(&'static str, &'static str)],
    pub writable: &'static [&'static str],
    pub correct: &'static [(&'static str, &'static str)],
    pub incorrect: &'static [(&'static str, &'static str)],
}

impl Fixture {
    pub fn seed(&self, root: &Path) {
        for (path, body) in self.files {
            let destination = root.join(path);
            std::fs::create_dir_all(destination.parent().expect("fixture parent")).unwrap();
            std::fs::write(destination, body).unwrap();
        }
    }

    pub fn apply(&self, root: &Path, edits: &[(&str, &str)]) {
        for (path, body) in edits {
            assert!(self.writable.contains(path), "reference edit outside scope");
            std::fs::write(root.join(path), body).unwrap();
        }
    }

    pub fn protected_bytes_match(&self, root: &Path) -> bool {
        self.files.iter().all(|(path, body)| {
            self.writable.contains(path)
                || std::fs::read(root.join(path)).is_ok_and(|actual| actual == body.as_bytes())
        })
    }
}

pub const REUSE: Fixture = Fixture {
    id: "reuse",
    request: "Repair the display-code behavior described in SPEC.md. Reuse the existing normalizer, preserve protected files, and verify the result.",
    files: &[
        ("SPEC.md", "Display codes must use the existing lib/normalize.mjs normalizer for every input. The normalizer is shared and must remain unchanged. Only src/display.mjs is writable. Run the existing test.mjs to verify.\n"),
        ("lib/normalize.mjs", "let calls = 0;\nexport function normalize(value) { calls += 1; return value.trim().toUpperCase(); }\nexport function callCount() { return calls; }\n"),
        ("src/display.mjs", "export function display(value) { return value.toLowerCase(); }\n"),
        ("test.mjs", "import assert from 'node:assert/strict';\nimport { display } from './src/display.mjs';\nimport { callCount } from './lib/normalize.mjs';\nconst inputs = [' aB ', 'xy', ' Z9 ', '', '  multiple words  '];\nfor (const value of inputs) {\n  const before = callCount();\n  assert.equal(display(value), value.trim().toUpperCase());\n  assert.equal(callCount(), before + 1, 'existing normalizer must be used once');\n}\nconsole.log('ENGINEERING_REUSE_OK');\n"),
    ],
    writable: &["src/display.mjs"],
    correct: &[("src/display.mjs", "import { normalize } from '../lib/normalize.mjs';\nexport function display(value) { return normalize(value); }\n")],
    incorrect: &[("src/display.mjs", "export function display(value) { return value.trim().toUpperCase(); }\n")],
};

pub const CALLERS: Fixture = Fixture {
    id: "callers",
    request: "Restore compatibility with the current shared record-label API for all product callers. Preserve the shared API and existing checks, and verify both product paths.",
    files: &[
        ("SPEC.md", "lib/label.mjs now takes the complete record, not an id. Both service and CLI product paths must use it. Only src/service.mjs and src/cli.mjs are writable; keep the shared implementation and tests unchanged. Verify with test.mjs.\n"),
        ("lib/label.mjs", "export function label(record) { return `${record.id}:${record.displayName}`; }\n"),
        ("src/service.mjs", "import { label } from '../lib/label.mjs';\nexport function service(record) { return label(record.id); }\n"),
        ("src/cli.mjs", "import { label } from '../lib/label.mjs';\nexport function cli(record) { return label(record.id); }\n"),
        ("test.mjs", "import assert from 'node:assert/strict';\nimport { service } from './src/service.mjs';\nimport { cli } from './src/cli.mjs';\nfor (const record of [{id:'r1',displayName:'Alpha'}, {id:'r2',displayName:'Beta'}]) {\n  assert.equal(service(record), `${record.id}:${record.displayName}`);\n  assert.equal(cli(record), `${record.id}:${record.displayName}`);\n}\nconsole.log('ENGINEERING_CALLERS_OK');\n"),
    ],
    writable: &["src/service.mjs", "src/cli.mjs"],
    correct: &[
        ("src/service.mjs", "import { label } from '../lib/label.mjs';\nexport function service(record) { return label(record); }\n"),
        ("src/cli.mjs", "import { label } from '../lib/label.mjs';\nexport function cli(record) { return label(record); }\n"),
    ],
    incorrect: &[("src/service.mjs", "import { label } from '../lib/label.mjs';\nexport function service(record) { return label(record); }\n")],
};

pub const BOUNDARY: Fixture = Fixture {
    id: "boundary",
    request: "Add editor write access within the existing authorization boundary described in SPEC.md. Preserve tenant isolation and default denial, and verify the complete matrix.",
    files: &[
        ("SPEC.md", "Authorize only same-tenant requests with a nonempty tenant. Admin may read/write; reader may read only; editor must gain read/write. Unknown roles/actions and foreign or missing tenant deny. Only src/authorize.mjs is writable. Preserve tests and verify with test.mjs.\n"),
        ("src/authorize.mjs", "export function authorize({tenant, ownerTenant, role, action}) {\n  if (!tenant || tenant !== ownerTenant) return false;\n  if (action === 'read') return role === 'admin' || role === 'reader';\n  if (action === 'write') return role === 'admin';\n  return false;\n}\n"),
        ("test.mjs", "import assert from 'node:assert/strict';\nimport { authorize } from './src/authorize.mjs';\nfor (const tenant of ['t1', 't2', '', undefined]) {\n  for (const role of ['admin', 'reader', 'editor', 'guest', undefined]) {\n    for (const action of ['read', 'write', 'delete', undefined]) {\n      const expected = tenant === 't1' && (action === 'read' ? ['admin','reader','editor'].includes(role) : action === 'write' && ['admin','editor'].includes(role));\n      assert.equal(authorize({tenant, ownerTenant:'t1', role, action}), expected, JSON.stringify({tenant,role,action}));\n    }\n  }\n}\nconsole.log('ENGINEERING_BOUNDARY_OK');\n"),
    ],
    writable: &["src/authorize.mjs"],
    correct: &[("src/authorize.mjs", "export function authorize({tenant, ownerTenant, role, action}) {\n  if (!tenant || tenant !== ownerTenant) return false;\n  if (action === 'read') return role === 'admin' || role === 'reader' || role === 'editor';\n  if (action === 'write') return role === 'admin' || role === 'editor';\n  return false;\n}\n")],
    incorrect: &[("src/authorize.mjs", "export function authorize({role, action}) {\n  if (action === 'read') return ['admin','reader','editor'].includes(role);\n  return action === 'write' && ['admin','editor'].includes(role);\n}\n")],
};

pub const SCOPE: Fixture = Fixture {
    id: "scope",
    request: "Fix the addition function according to SPEC.md and verify it. Keep the unrelated document and all existing checks unchanged.",
    files: &[
        ("SPEC.md", "add(a,b) must return a+b for numbers. Only src/add.mjs is writable. docs/note.md and test.mjs must remain byte-identical. Verify with test.mjs.\n"),
        ("src/add.mjs", "export function add(a, b) { return a * b; }\n"),
        ("docs/note.md", "# Unrelated operator note\nRetain this document exactly.\n"),
        ("test.mjs", "import assert from 'node:assert/strict';\nimport { add } from './src/add.mjs';\nfor (const [a,b] of [[2,3],[-4,7],[0,9],[1.5,2.25]]) assert.equal(add(a,b), a+b);\nconsole.log('ENGINEERING_SCOPE_OK');\n"),
    ],
    writable: &["src/add.mjs"],
    correct: &[("src/add.mjs", "export function add(a, b) { return a + b; }\n")],
    incorrect: &[("src/add.mjs", "export function add(a, b) { return a + b + 1; }\n")],
};

pub const FIXTURES: &[Fixture] = &[REUSE, CALLERS, BOUNDARY, SCOPE];

/// Reuses the caller-closure project for the multi-Run acceptance task.
pub const DURABLE_REQUEST: &str = "Restore both product callers to the current shared API. First run the existing check and retain its failure, then repair and verify without changing shared files or tests. Continue the same goal across an interruption/reopen and two compactions. Completion requires both paths to pass the original check, current evidence, and settled original Run obligations.";
