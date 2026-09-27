//! Hub tool ID constants, canonical in `a3s_code_workspace_types::rpc` and re-exported here for existing importers.
//!
//! `WORKSPACE_TOOL_NOTIFICATIONS_TOOL_ID` intentionally has no producer yet; see [`crate::hub_channel::extract_tool_notification`].

pub use a3s_code_workspace_types::rpc::{
    WORKSPACE_CLIENT_EXT_NOTIFICATIONS_TOOL_ID, WORKSPACE_EVENTS_TOOL_ID, WORKSPACE_RPC_TOOL_ID,
    WORKSPACE_TOOL_NOTIFICATIONS_TOOL_ID,
};
