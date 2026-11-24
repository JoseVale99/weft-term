//! Agent core for Weft.
//!
//! This crate models the conversation, the tool proposals, the path sandbox and
//! the approval gate. It never runs a command and never opens a network
//! connection; transport and execution live in `weft`, so the same logic can be
//! tested without a window or a server.
//!
//! A proposed command is never executed by this crate. The caller asks
//! [`Guard`] for a [`Decision`] and runs it only when the decision is
//! [`Decision::Run`].

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod approval;
mod json;
mod message;
mod openai;
mod sandbox;
mod tools;

pub use approval::{Decision, Guard, Proposal};
pub use json::{JsonError, Value, parse, quote};
pub use message::{Conversation, Message, Role};
pub use openai::{Event, Provider, Request, RequestError, StreamParser};
pub use sandbox::{resolve, resolve_in};
pub use tools::{ToolCall, ToolSpec, specs};
