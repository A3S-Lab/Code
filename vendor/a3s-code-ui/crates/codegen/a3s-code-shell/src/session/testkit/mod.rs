//! Session synthesis for the load-perf and fork bench tests.
//!
//! This module lives in `a3s-code-shell` (feature `test-support`) rather than `a3s-code-test-support`.
//! Synthesis drives the real `JsonlStorageAdapter`, so the reverse dependency would be circular.

pub mod synth;
