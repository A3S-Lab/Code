//! STD-UNWRAP2: the release profile is `panic = "abort"`, so a reached
//! `.unwrap()` / `.expect(..)` kills every session in the process. This
//! ratchet counts them in production code only and holds each file to its
//! baseline, which may only shrink.
//!
//! Production files come from `support/production_graph.rs`. Inside them,
//! `#[cfg(test)]` items, impl functions and `let` statements are skipped.
//! Calls inside macro invocations whose body parses as comma-separated
//! expressions (`format!`, `vec!`, `assert!`, ...) are counted too.
//!
//! Print a fresh baseline with
//! `A3S_UNWRAP_RATCHET_PRINT=1 cargo test -p a3s-code-core --test production_unwrap_ratchet -- --nocapture`.

use std::collections::BTreeMap;

use syn::punctuated::Punctuated;
use syn::visit::Visit;

#[path = "production_unwrap_ratchet/baseline.rs"]
mod baseline;
#[path = "support/production_graph.rs"]
mod production_graph;

use production_graph::{is_cfg_test, item_is_test_only};

#[derive(Default)]
struct Counter {
    count: usize,
}

impl<'ast> Visit<'ast> for Counter {
    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        let name = call.method.to_string();
        if (name == "unwrap" && call.args.is_empty()) || (name == "expect" && call.args.len() == 1)
        {
            self.count += 1;
        }
        syn::visit::visit_expr_method_call(self, call);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if let Ok(exprs) =
            mac.parse_body_with(Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated)
        {
            for expr in &exprs {
                self.visit_expr(expr);
            }
        }
    }

    fn visit_item(&mut self, item: &'ast syn::Item) {
        if item_is_test_only(item) {
            return;
        }
        syn::visit::visit_item(self, item);
    }

    fn visit_impl_item(&mut self, item: &'ast syn::ImplItem) {
        if let syn::ImplItem::Fn(function) = item {
            if is_cfg_test(&function.attrs) {
                return;
            }
        }
        syn::visit::visit_impl_item(self, item);
    }

    fn visit_stmt(&mut self, stmt: &'ast syn::Stmt) {
        if let syn::Stmt::Local(local) = stmt {
            if is_cfg_test(&local.attrs) {
                return;
            }
        }
        syn::visit::visit_stmt(self, stmt);
    }
}

fn production_counts() -> BTreeMap<String, usize> {
    production_graph::production_files()
        .into_iter()
        .map(|(path, file)| {
            let mut counter = Counter::default();
            counter.visit_file(&file.syntax);
            (path, counter.count)
        })
        .collect()
}

#[test]
fn production_unwrap_and_expect_only_shrink() {
    let counts = production_counts();
    if std::env::var_os("A3S_UNWRAP_RATCHET_PRINT").is_some() {
        println!("pub const BASELINE: &[(&str, usize)] = &[");
        for (path, count) in counts.iter().filter(|(_, count)| **count > 0) {
            println!("    (\"{path}\", {count}),");
        }
        println!("];");
        println!("// {} production files walked", counts.len());
    }
    let baseline: BTreeMap<&str, usize> = baseline::BASELINE.iter().copied().collect();
    let mut problems = Vec::new();
    for (path, count) in &counts {
        let allowed = baseline.get(path.as_str()).copied().unwrap_or(0);
        if *count > allowed {
            problems.push(format!(
                "{path}: {count} production unwrap/expect calls, baseline {allowed}; \
                 return the error instead"
            ));
        } else if *count < allowed {
            problems.push(format!(
                "{path}: {count} calls, baseline {allowed}; lower the baseline to {count}"
            ));
        }
    }
    for path in baseline.keys() {
        if !counts.contains_key(*path) {
            problems.push(format!(
                "{path}: in the baseline but no longer production code; remove the row"
            ));
        }
    }
    let total: usize = counts.values().sum();
    let allowed: usize = baseline.values().sum();
    assert!(
        problems.is_empty(),
        "production unwrap/expect ratchet ({total} counted, baseline {allowed}):\n{}",
        problems.join("\n")
    );
}

#[test]
fn the_walker_skips_test_code_and_counts_production_calls() {
    let parsed: syn::File = syn::parse_quote! {
        fn production() {
            let a = value.unwrap();
            let b = value.expect("named invariant");
            let c = value.unwrap_or_default();
            let d = format!("{}", other.unwrap());
        }
        #[cfg(test)]
        fn helper() { value.unwrap(); }
        #[cfg(all(test, unix))]
        mod unix_tests { fn t() { value.unwrap(); } }
        #[cfg(feature = "x")]
        fn gated() { value.unwrap(); }
        impl Thing {
            #[cfg(test)]
            fn only_in_tests(&self) { self.value.unwrap(); }
            fn real(&self) { self.value.expect("x"); }
        }
    };
    let mut counter = Counter::default();
    counter.visit_file(&parsed);
    assert_eq!(counter.count, 5);
}
