//! ACP agent over a3s-code-core 9.0.0.
//!
//! Speaks Agent Client Protocol on stdio so the forked a3s-build pager can
//! drive real A3S sessions instead of the in-process A3S Code `MvpAgent`.

mod agent;
mod cc_switch;
mod config;
mod goal;
mod grok_account;

pub use agent::A3sCodeAgent;
pub use config::{load_launch_config, LaunchConfig};
