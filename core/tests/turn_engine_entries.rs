//! CORE-LOOP1: no new way into the legacy turn engine.
//!
//! Session runs drive model turns through `FactRun` (`fact_control.rs`).
//! `TurnEngine` (still named `AgentLoop` at call sites) is crate-private.
//! `skill` and `task` children enter through `ChildRun` / `FactRun`.
//! The old standalone/planning routes are test-only. This
//! test pins every production site that builds an `AgentLoop` or enters its
//! loop. The list may only shrink: a new site fails, and a site that moved
//! to `FactRun` must leave the list. Test-only code is not scanned.

#[path = "support/production_graph.rs"]
mod production_graph;

use std::collections::BTreeSet;
use syn::visit::Visit;

/// Entry methods of the legacy engine.
const ENTRY_METHODS: &[&str] = &[
    "execute_loop",
    "execute_loop_inner",
    "execute_with_session",
    "execute_with_invocation",
    "execute_with_planning",
];

/// `(file, site)` pairs present when the ratchet landed.
const ALLOWED: &[(&str, &str)] = &[
    // The engine's own plumbing.
    ("core/src/agent/execution_entry.rs", ".execute_loop_inner"),
    // The session's loop: FactRun borrows its configuration and tools.
    ("core/src/agent_api/agent_loop_runtime.rs", "AgentLoop::new"),
    // Child runs build an engine the fact run borrows. They do not enter it.
    ("core/src/tools/skill.rs", "AgentLoop::new"),
    ("core/src/tools/task.rs", "AgentLoop::new"),
];

#[derive(Default)]
struct Sites {
    found: BTreeSet<String>,
}

fn is_test_fn(attrs: &[syn::Attribute]) -> bool {
    production_graph::is_cfg_test(attrs)
        || attrs.iter().any(|attr| {
            attr.path()
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "test")
        })
}

impl<'ast> Visit<'ast> for Sites {
    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        if !production_graph::is_cfg_test(&module.attrs) {
            syn::visit::visit_item_mod(self, module);
        }
    }

    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        if !is_test_fn(&function.attrs) {
            syn::visit::visit_item_fn(self, function);
        }
    }

    fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
        if !is_test_fn(&function.attrs) {
            syn::visit::visit_impl_item_fn(self, function);
        }
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = call.func.as_ref() {
            let segments: Vec<String> = path
                .path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect();
            if segments.ends_with(&["AgentLoop".to_string(), "new".to_string()]) {
                self.found.insert("AgentLoop::new".into());
            }
        }
        syn::visit::visit_expr_call(self, call);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        let method = call.method.to_string();
        if ENTRY_METHODS.contains(&method.as_str()) {
            self.found.insert(format!(".{method}"));
        }
        syn::visit::visit_expr_method_call(self, call);
    }
}

fn sites(syntax: &syn::File) -> BTreeSet<String> {
    let mut sites = Sites::default();
    sites.visit_file(syntax);
    sites.found
}

#[test]
fn legacy_turn_engine_entries_only_shrink() {
    let mut found = BTreeSet::new();
    for (path, file) in production_graph::production_files() {
        for site in sites(&file.syntax) {
            found.insert((path.clone(), site));
        }
    }
    let allowed: BTreeSet<(String, String)> = ALLOWED
        .iter()
        .map(|(path, site)| ((*path).to_string(), (*site).to_string()))
        .collect();
    let added: Vec<String> = found
        .difference(&allowed)
        .map(|(path, site)| format!("    (\"{path}\", \"{site}\"),"))
        .collect();
    let gone: Vec<String> = allowed
        .difference(&found)
        .map(|(path, site)| format!("    (\"{path}\", \"{site}\"),"))
        .collect();
    assert!(
        added.is_empty(),
        "new legacy turn-engine entries; route the run through FactRun instead:\n{}",
        added.join("\n")
    );
    assert!(
        gone.is_empty(),
        "these entries are gone; remove them from ALLOWED:\n{}",
        gone.join("\n")
    );
}

#[test]
fn the_scanner_sees_entries_but_not_tests() {
    let syntax: syn::File = syn::parse_quote! {
        fn child() {
            let agent = crate::agent::AgentLoop::new(client, tools, context, config);
            agent.execute_with_session(history, prompt, None, None, None);
        }
        #[cfg(test)]
        mod tests {
            fn helper() {
                AgentLoop::new(client, tools, context, config).execute_loop();
            }
        }
    };
    assert_eq!(
        sites(&syntax),
        BTreeSet::from([
            "AgentLoop::new".to_string(),
            ".execute_with_session".to_string()
        ])
    );
}
