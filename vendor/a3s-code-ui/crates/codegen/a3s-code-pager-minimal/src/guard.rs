//! Compile-time guard for minimal mode's resize strategy.
//!
//! Minimal reprints history from its scrollback entries through the commit renderer ([`crate::reprint`]).
//! It must NEVER use the inline crate's string-history helpers (`resize_purge_rerender`, `emit_to_scrollback`, `resize_viewport_height`).
//! They would print stale or hard-wrapped history from a string minimal does not keep.
//! This test fails loudly if such a call ever sneaks into the minimal module.

#[test]
fn minimal_never_uses_ris_rerender_or_emit_to_scrollback() {
    const FORBIDDEN: &[&str] = &[
        "resize_purge_rerender",
        "emit_to_scrollback",
        "resize_viewport_height",
    ];
    // EVERY module of this crate except this guard file (which names the forbidden identifiers)
    // Keep in sync with `lib.rs`'s module list: a module missing here is a hole in the guard
    let sources = [
        ("lib.rs", include_str!("lib.rs")),
        ("auth.rs", include_str!("auth.rs")),
        ("commit.rs", include_str!("commit.rs")),
        ("feedback.rs", include_str!("feedback.rs")),
        ("full_view.rs", include_str!("full_view.rs")),
        ("live.rs", include_str!("live.rs")),
        ("overlay.rs", include_str!("overlay.rs")),
        ("panel.rs", include_str!("panel.rs")),
        ("plan.rs", include_str!("plan.rs")),
        ("reprint.rs", include_str!("reprint.rs")),
        ("todo.rs", include_str!("todo.rs")),
        ("welcome.rs", include_str!("welcome.rs")),
    ];
    for (name, src) in sources {
        for needle in FORBIDDEN {
            assert!(
                !src.contains(needle),
                "minimal/{name} references forbidden resize helper `{needle}`; reprint \
                 history from the scrollback entries through `crate::reprint` instead"
            );
        }
    }
}
