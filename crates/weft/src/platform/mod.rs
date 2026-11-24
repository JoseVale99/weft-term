//! Platform-specific entry points.
//!
//! Everything that differs between operating systems lives here, so the rest of
//! the crate is free of `cfg` attributes: shells, command execution, raw mode
//! and the choice of front-end.

use std::io;

/// Runs the experimental GPU front-end.
pub fn gpu() -> io::Result<()> {
    crate::gpu::run()
}

#[cfg(any(unix, windows))]
mod tui;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::{
    columns, command, default_shell, enter_raw, home, interactive, is_interactive, window_size,
};

#[cfg(windows)]
pub use windows::{
    columns, command, default_shell, enter_raw, home, interactive, is_interactive, window_size,
};
