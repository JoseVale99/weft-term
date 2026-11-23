//! `weft-term`: the terminal engine of Weft.
//!
//! The crate is UI-free on purpose. It owns:
//!
//! - [`osc`]: semantic-prompt and cwd escapes (OSC 133/633, OSC 7);
//! - [`vt`]: the escape-sequence interpreter;
//! - [`grid`]: cells, cursor, scrollback and screen operations;
//! - [`blocks`]: the block model (command, output, exit code);
//! - [`pty`]: a running shell on a pseudo-terminal (Unix).
//!
//! Nothing here may depend on winit, wgpu or any renderer. If a change needs a
//! window to be tested, it belongs in the `weft` binary crate.

#![deny(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod blocks;
pub mod grid;
pub mod osc;
pub mod vt;

#[cfg(unix)]
#[allow(unsafe_code)]
pub mod pty;
#[cfg(unix)]
#[allow(unsafe_code)]
pub mod sys;

pub use blocks::{Block, BlockId, BlockState, Blocks};
pub use grid::{Attrs, Cell, Color, Cursor, Grid, Pen, Sgr, Wide};
pub use osc::{Event, Parser};
pub use vt::Terminal;
