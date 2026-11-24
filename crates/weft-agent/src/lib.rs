//! Agent core for Weft.
//!
//! This crate is pure: it models the conversation, the tool proposals and the
//! approval gate, and it never performs I/O. Transport and UI live in `weft`,
//! so the same logic can be tested without a network or a window.
//!
//! A proposed command is never executed by this crate. The caller asks
//! [`Guard`] for a [`Decision`] and runs it only when the decision is
//! [`Decision::Run`].

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod approval;
mod json;
mod message;

pub use approval::{Decision, Guard, Proposal};
pub use json::{JsonError, Value, parse};
pub use message::{Conversation, Message, Role};
