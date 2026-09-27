//! End-to-end tests for the `browser_tab` and `browser_network_details` tools
//! against a real Chrome.
//!
//! The crate-level suite in `a3s-code-browser-tools` covers the CDP layer;
//! these drive the tools themselves, so the output the model actually sees is
//! what gets asserted.
//!
//! Ignored by default — CI has no Chrome. Run them with:
//!
//! ```bash
//! cargo test -p a3s-code-tools --features browser --test browser_tab_chrome_e2e \
//!     -- --ignored --test-threads=1
//! ```
