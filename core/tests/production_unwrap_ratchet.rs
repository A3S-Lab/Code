//! STD-UNWRAP2: the release profile is `panic = "abort"`, so a reached
//! `.unwrap()` / `.expect(..)` kills every session in the process. This
//! ratchet counts them in production code only and holds each file to its
//! baseline, which may only shrink.
//!
//! Production code is what the crate roots compile without `cfg(test)`:
//! the module graph is followed from each root through `mod` declarations
//! (including `#[path]`), `#[cfg(test)]` modules and items are skipped, and
//! files no root reaches are not counted. Calls inside macro invocations
//! whose body parses as comma-separated expressions (`format!`, `vec!`,
//! `assert!`, ...) are counted too.
//!
//! Print a fresh baseline with
//! `A3S_UNWRAP_RATCHET_PRINT=1 cargo test -p a3s-code-core --test production_unwrap_ratchet -- --nocapture`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use syn::punctuated::Punctuated;
use syn::visit::Visit;

#[path = "production_unwrap_ratchet/baseline.rs"]
mod baseline;

/// Crate roots relative to the `apps/code` workspace.
const ROOTS: &[&str] = &[
    "core/src/lib.rs",
    "acp/src/lib.rs",
    "acp/src/bin/a3s-code-acp.rs",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("core sits inside the workspace")
        .to_path_buf()
}

fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("cfg") {
            return false;
        }
        let Ok(meta) = attr.parse_args::<syn::Meta>() else {
            return false;
        };
        cfg_requires_test(&meta)
    })
}

/// `test`, or `all(..)` with `test` among its predicates.
fn cfg_requires_test(meta: &syn::Meta) -> bool {
    match meta {
        syn::Meta::Path(path) => path.is_ident("test"),
        syn::Meta::List(list) if list.path.is_ident("all") => list
            .parse_args_with(Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated)
            .is_ok_and(|items| items.iter().any(cfg_requires_test)),
        _ => false,
    }
}

fn path_attr(attrs: &[syn::Attribute]) -> Option<String> {
    attrs.iter().find_map(|attr| {
        if !attr.path().is_ident("path") {
            return None;
        }
        match &attr.meta {
            syn::Meta::NameValue(syn::MetaNameValue {
                value:
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(value),
                        ..
                    }),
                ..
            }) => Some(value.value()),
            _ => None,
        }
    })
}

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

fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(item) => &item.attrs,
        syn::Item::Enum(item) => &item.attrs,
        syn::Item::Fn(item) => &item.attrs,
        syn::Item::Impl(item) => &item.attrs,
        syn::Item::Macro(item) => &item.attrs,
        syn::Item::Mod(item) => &item.attrs,
        syn::Item::Static(item) => &item.attrs,
        syn::Item::Struct(item) => &item.attrs,
        syn::Item::Trait(item) => &item.attrs,
        syn::Item::Use(item) => &item.attrs,
        _ => &[],
    }
}

fn item_is_test_only(item: &syn::Item) -> bool {
    is_cfg_test(item_attrs(item))
}

/// Count one file, then follow its out-of-line `mod` declarations.
/// `child_dir` is where `mod name;` resolves for this file.
fn walk_file(file: &Path, child_dir: &Path, counts: &mut BTreeMap<PathBuf, usize>) {
    if counts.contains_key(file) {
        return;
    }
    let source = std::fs::read_to_string(file)
        .unwrap_or_else(|error| panic!("read {}: {error}", file.display()));
    let parsed = syn::parse_file(&source)
        .unwrap_or_else(|error| panic!("parse {}: {error}", file.display()));
    if is_cfg_test(&parsed.attrs) {
        return;
    }
    let mut counter = Counter::default();
    counter.visit_file(&parsed);
    counts.insert(file.to_path_buf(), counter.count);
    let file_dir = file.parent().expect("a source file has a directory");
    walk_items(&parsed.items, file_dir, child_dir, counts);
}

fn walk_items(
    items: &[syn::Item],
    file_dir: &Path,
    child_dir: &Path,
    counts: &mut BTreeMap<PathBuf, usize>,
) {
    for item in items {
        let syn::Item::Mod(module) = item else {
            continue;
        };
        if is_cfg_test(&module.attrs) {
            continue;
        }
        let name = module.ident.to_string();
        match (&module.content, path_attr(&module.attrs)) {
            (Some((_, inner)), _) => {
                walk_items(inner, file_dir, &child_dir.join(&name), counts);
            }
            (None, Some(explicit)) => {
                let target = file_dir.join(explicit);
                let dir = if target.file_name().is_some_and(|name| name == "mod.rs") {
                    target.parent().expect("mod.rs directory").to_path_buf()
                } else {
                    target.with_extension("")
                };
                walk_file(&target, &dir, counts);
            }
            (None, None) => {
                let flat = child_dir.join(format!("{name}.rs"));
                if flat.is_file() {
                    walk_file(&flat, &child_dir.join(&name), counts);
                } else {
                    let nested = child_dir.join(&name).join("mod.rs");
                    walk_file(&nested, &child_dir.join(&name), counts);
                }
            }
        }
    }
}

fn production_counts() -> BTreeMap<String, usize> {
    let root = workspace_root();
    let mut counts = BTreeMap::new();
    for crate_root in ROOTS {
        let file = root.join(crate_root);
        let dir = if crate_root.ends_with("lib.rs") {
            file.parent().expect("lib.rs directory").to_path_buf()
        } else {
            file.with_extension("")
        };
        walk_file(&file, &dir, &mut counts);
    }
    counts
        .into_iter()
        .map(|(path, count)| {
            let relative = path
                .strip_prefix(&root)
                .expect("walked files stay in the workspace")
                .to_string_lossy()
                .replace('\\', "/");
            (relative, count)
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
