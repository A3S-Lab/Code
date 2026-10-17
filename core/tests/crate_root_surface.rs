//! CORE-API1: the documented crate root stays small.
//!
//! A crate-root `pub use` name is part of the documented surface unless its
//! statement is `#[doc(hidden)]`. Hidden names are deprecated compatibility
//! aliases (import from the module path instead); they may only shrink and
//! are removed no earlier than 10.0. New public API lands in its module, and
//! enters the root only within `VISIBLE_LIMIT`.

use syn::{Item, UseTree, Visibility};

const VISIBLE_LIMIT: usize = 150;
/// Deprecated `#[doc(hidden)]` root aliases. Lower it when one is removed.
const HIDDEN_BASELINE: usize = 644;
/// Public top-level modules. Lower it when one becomes private.
const PUB_MOD_BASELINE: usize = 81;

fn leaf_count(tree: &UseTree) -> usize {
    match tree {
        UseTree::Path(path) => leaf_count(&path.tree),
        UseTree::Name(_) | UseTree::Rename(_) => 1,
        UseTree::Glob(_) => usize::MAX / 4,
        UseTree::Group(group) => group.items.iter().map(leaf_count).sum(),
    }
}

fn is_doc_hidden(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("doc")
            && attr
                .parse_args::<syn::Ident>()
                .is_ok_and(|ident| ident == "hidden")
    })
}

struct Surface {
    visible: usize,
    hidden: usize,
    pub_mods: usize,
}

fn surface(source: &str) -> Surface {
    let file = syn::parse_file(source).expect("lib.rs parses");
    let mut surface = Surface {
        visible: 0,
        hidden: 0,
        pub_mods: 0,
    };
    for item in &file.items {
        match item {
            Item::Use(item) if matches!(item.vis, Visibility::Public(_)) => {
                let names = leaf_count(&item.tree);
                if is_doc_hidden(&item.attrs) {
                    surface.hidden += names;
                } else {
                    surface.visible += names;
                }
            }
            Item::Mod(item) if matches!(item.vis, Visibility::Public(_)) => {
                surface.pub_mods += 1;
            }
            _ => {}
        }
    }
    surface
}

#[test]
fn the_documented_crate_root_stays_within_its_budget() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs");
    let source = std::fs::read_to_string(path).expect("read lib.rs");
    let Surface {
        visible,
        hidden,
        pub_mods,
    } = surface(&source);
    let mut problems = Vec::new();
    if visible > VISIBLE_LIMIT {
        problems.push(format!(
            "{visible} documented crate-root names, the limit is {VISIBLE_LIMIT}; \
             export the new API from its module"
        ));
    }
    if hidden > HIDDEN_BASELINE {
        problems.push(format!(
            "{hidden} hidden crate-root aliases, baseline {HIDDEN_BASELINE}; \
             a new name is not a deprecated alias"
        ));
    } else if hidden < HIDDEN_BASELINE {
        problems.push(format!(
            "{hidden} hidden crate-root aliases; lower HIDDEN_BASELINE to {hidden}"
        ));
    }
    if pub_mods > PUB_MOD_BASELINE {
        problems.push(format!(
            "{pub_mods} public top-level modules, baseline {PUB_MOD_BASELINE}; \
             nest the new module under an existing one"
        ));
    } else if pub_mods < PUB_MOD_BASELINE {
        problems.push(format!(
            "{pub_mods} public top-level modules; lower PUB_MOD_BASELINE to {pub_mods}"
        ));
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn hidden_and_grouped_names_are_counted_apart() {
    let surface = surface(
        r#"
        pub mod a;
        mod b;
        pub use a::{One, inner::{Two, Three as Four}};
        #[cfg(feature = "x")]
        #[doc(hidden)]
        pub use a::{Five, Six};
        pub(crate) use a::Seven;
        "#,
    );
    assert_eq!(surface.visible, 3);
    assert_eq!(surface.hidden, 2);
    assert_eq!(surface.pub_mods, 1);
}
