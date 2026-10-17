//! CORE-FILES1: a production source file stays under `LIMIT` lines.
//!
//! Production files come from `support/production_graph.rs` (the core and
//! ACP module graphs without `cfg(test)` files). A file's whole text counts,
//! inline `#[cfg(test)]` modules included: tests belong in a sibling file.
//!
//! `OVERSIZE` lists the files that were over the limit when the ratchet
//! landed. Its caps may only shrink: a listed file may not grow past its
//! cap, a cap may not sit more than `CAP_SLACK` lines above its file, and a
//! file that drops under the limit loses its row.

use std::collections::BTreeMap;

#[path = "support/production_graph.rs"]
mod production_graph;

const LIMIT: usize = 2000;
const CAP_SLACK: usize = 250;

/// Files over `LIMIT`, with the line count they may not exceed.
const OVERSIZE: &[(&str, usize)] = &[
    ("acp/src/agent.rs", 4524),
    ("core/src/fact_control.rs", 4861),
];

fn line_count(source: &str) -> usize {
    source.lines().count()
}

#[test]
fn production_files_stay_under_the_line_limit() {
    let sizes: BTreeMap<String, usize> = production_graph::production_files()
        .into_iter()
        .map(|(path, file)| (path, line_count(&file.source)))
        .collect();
    let caps: BTreeMap<&str, usize> = OVERSIZE.iter().copied().collect();
    let mut problems = Vec::new();
    for (path, lines) in &sizes {
        match caps.get(path.as_str()) {
            None if *lines >= LIMIT => problems.push(format!(
                "{path}: {lines} lines, the limit is {LIMIT}; split the file"
            )),
            None => {}
            Some(_) if *lines < LIMIT => problems.push(format!(
                "{path}: {lines} lines, under the limit; remove its OVERSIZE row"
            )),
            Some(cap) if lines > cap => problems.push(format!(
                "{path}: {lines} lines, cap {cap}; split the file instead of growing it"
            )),
            Some(cap) if cap - lines > CAP_SLACK => problems.push(format!(
                "{path}: {lines} lines, cap {cap}; lower the cap to {lines}"
            )),
            Some(_) => {}
        }
    }
    for path in caps.keys() {
        if !sizes.contains_key(*path) {
            problems.push(format!(
                "{path}: in OVERSIZE but no longer production code; remove the row"
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "production file size ratchet ({} files):\n{}",
        sizes.len(),
        problems.join("\n")
    );
}
