//! The production module graph shared by the source ratchets.
//!
//! Production code is what the crate roots compile without `cfg(test)`:
//! the graph is followed from each root through `mod` declarations
//! (including `#[path]` and inline modules), `#[cfg(test)]` modules are
//! skipped, and files no root reaches are not production code.

// Each ratchet test crate includes this module and uses a different subset.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use syn::punctuated::Punctuated;

/// Crate roots relative to the `apps/code` workspace.
pub const ROOTS: &[&str] = &[
    "core/src/lib.rs",
    "acp/src/lib.rs",
    "acp/src/bin/a3s-code-acp.rs",
];

/// One production file: its source and its parsed syntax tree.
pub struct ProductionFile {
    pub source: String,
    pub syntax: syn::File,
}

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("core sits inside the workspace")
        .to_path_buf()
}

pub fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
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

pub fn item_is_test_only(item: &syn::Item) -> bool {
    is_cfg_test(item_attrs(item))
}

/// Parse one file, then follow its out-of-line `mod` declarations.
/// `child_dir` is where `mod name;` resolves for this file.
fn walk_file(file: &Path, child_dir: &Path, files: &mut BTreeMap<PathBuf, ProductionFile>) {
    if files.contains_key(file) {
        return;
    }
    let source = std::fs::read_to_string(file)
        .unwrap_or_else(|error| panic!("read {}: {error}", file.display()));
    let syntax = syn::parse_file(&source)
        .unwrap_or_else(|error| panic!("parse {}: {error}", file.display()));
    if is_cfg_test(&syntax.attrs) {
        return;
    }
    let items = syntax.items.clone();
    files.insert(file.to_path_buf(), ProductionFile { source, syntax });
    let file_dir = file.parent().expect("a source file has a directory");
    walk_items(&items, file_dir, child_dir, files);
}

fn walk_items(
    items: &[syn::Item],
    file_dir: &Path,
    child_dir: &Path,
    files: &mut BTreeMap<PathBuf, ProductionFile>,
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
                walk_items(inner, file_dir, &child_dir.join(&name), files);
            }
            (None, Some(explicit)) => {
                let target = file_dir.join(explicit);
                let dir = if target.file_name().is_some_and(|name| name == "mod.rs") {
                    target.parent().expect("mod.rs directory").to_path_buf()
                } else {
                    target.with_extension("")
                };
                walk_file(&target, &dir, files);
            }
            (None, None) => {
                let flat = child_dir.join(format!("{name}.rs"));
                if flat.is_file() {
                    walk_file(&flat, &child_dir.join(&name), files);
                } else {
                    let nested = child_dir.join(&name).join("mod.rs");
                    walk_file(&nested, &child_dir.join(&name), files);
                }
            }
        }
    }
}

/// Every production file, keyed by its path relative to the workspace.
pub fn production_files() -> BTreeMap<String, ProductionFile> {
    let root = workspace_root();
    let mut files = BTreeMap::new();
    for crate_root in ROOTS {
        let file = root.join(crate_root);
        let dir = if crate_root.ends_with("lib.rs") {
            file.parent().expect("lib.rs directory").to_path_buf()
        } else {
            file.with_extension("")
        };
        walk_file(&file, &dir, &mut files);
    }
    files
        .into_iter()
        .map(|(path, file)| {
            let relative = path
                .strip_prefix(&root)
                .expect("walked files stay in the workspace")
                .to_string_lossy()
                .replace('\\', "/");
            (relative, file)
        })
        .collect()
}
