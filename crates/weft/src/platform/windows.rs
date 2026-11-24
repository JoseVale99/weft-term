//! Windows glue.
//!
//! Raw mode needs a console handle and ConPTY is not wired yet, so the block
//! shell stays available and the full-screen front-end is refused. Both limits
//! are explicit instead of a silent fallback.

use std::env;
use std::io;
use std::process::Command;

/// No-op guard: Windows has no raw mode here yet.
#[derive(Debug)]
pub struct RawInput;

/// Raw mode is not available; the caller falls back to line input.
pub fn enter_raw() -> io::Result<RawInput> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "raw input requires a console and is not implemented on Windows yet",
    ))
}

/// Builds the command used to run a shell line.
pub fn command(line: &str) -> Command {
    let mut command = Command::new("cmd");
    command.arg("/C").arg(line);
    command
}

/// The user's home directory, or `.` when it cannot be determined.
pub fn home() -> String {
    env::var("USERPROFILE").unwrap_or_else(|_| ".".to_owned())
}

/// Width of the terminal, or 80 columns when it cannot be measured.
pub fn columns() -> usize {
    sys::window_size().map_or(80, |(columns, _)| columns)
}

/// Whether standard input is a terminal.
pub fn is_interactive() -> bool {
    std::io::IsTerminal::is_terminal(&io::stdin())
}

/// The user's shell, for the full-screen terminal.
pub fn default_shell() -> String {
    env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_owned())
}

/// Runs the front-end that fits the environment.
pub fn interactive() -> io::Result<()> {
    super::tui::run()
}
