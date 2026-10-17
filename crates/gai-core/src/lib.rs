//! Core types shared by every gai crate.
//!
//! These are provider-neutral: each model dialect translates them to and from
//! its own wire format, so agents, tools, storage and the server only ever see
//! these shapes.

mod id;
mod message;
mod tool;
mod usage;

pub use id::new_id;
pub use message::{ContentBlock, MediaSource, Message, Role};
pub use tool::{ToolChoice, ToolSpec};
pub use usage::{Pricing, Usage};
