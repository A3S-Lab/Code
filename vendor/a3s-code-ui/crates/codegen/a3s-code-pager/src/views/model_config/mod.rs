//! `/config` modal for adding and editing ACL providers and models.

mod document;
mod render;
mod state;

pub(crate) use render::{MODAL_TITLE, render_model_config};
pub(crate) use state::{ModelConfigOutcome, ModelConfigState};
