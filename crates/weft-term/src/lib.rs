//! `weft-term`: the terminal engine of Weft.
//!
//! The crate is UI-free on purpose. It owns:
//!
//! - [`osc`]: semantic-prompt and cwd escapes (OSC 133/633, OSC 7);
//! - [`vt`]: the escape-sequence interpreter;
//! - [`grid`]: cells, cursor, scrollback and screen operations;
//! - [`blocks`]: the block model (command, output, exit code);
//! - [`fuzzy`]: subsequence matching for history search and the palette;
//! - [`history`]: a bounded, deduplicated command history;
//! - [`input`]: the multiline input editor;
//! - [`layout`]: row allocation for split panes;
//! - [`pty`]: a running shell on a pseudo-terminal (Unix);
//! - [`conpty`]: the same session API over Windows ConPTY.
//!
//! Nothing here may depend on winit, wgpu or any renderer. If a change needs a
//! window to be tested, it belongs in the `weft` binary crate.

#![deny(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod blocks;
pub mod fuzzy;
pub mod grid;
pub mod history;
pub mod input;
pub mod layout;
pub mod osc;
pub mod vt;

#[cfg(windows)]
#[allow(unsafe_code)]
pub mod conpty;
#[cfg(unix)]
#[allow(unsafe_code)]
#[path = "pty.rs"]
pub mod pty;
#[cfg(any(unix, windows))]
#[allow(unsafe_code)]
pub mod sys;

#[cfg(windows)]
pub use conpty::Session;
/// The session API of the platform: PTY on Unix, ConPTY on Windows.
#[cfg(unix)]
pub use pty::Session;

pub use blocks::{Block, BlockId, BlockState, Blocks};
pub use grid::{Attrs, Cell, Color, Cursor, Grid, Pen, Sgr, Wide};
pub use history::History;
pub use input::{Completion, Editor};
pub use layout::{Layout, Orientation, Rect, Segment, Span, SplitTree, heights, spans};
pub use osc::{Event, Parser};
pub use vt::Terminal;
