//! U-RX-04: the default build must not compile the research module.
//!
//! This file is not compiled when `--features research` is on. The default
//! `cargo test -p a3s-code-core --test research_feature_absence` run is the
//! check. `lib.rs` gates `pub mod research` with `#[cfg(feature = "research")]`.

#![cfg(not(feature = "research"))]

#[test]
fn research_module_is_absent_from_the_default_build() {
    assert!(
        !cfg!(feature = "research"),
        "default features compiled a3s_code_core::research"
    );
    assert!(
        option_env!("CARGO_FEATURE_RESEARCH").is_none(),
        "Cargo enabled the research feature for the default test binary"
    );
}
