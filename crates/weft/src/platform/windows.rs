//! Windows glue: `COMSPEC`, console raw mode, ConPTY window size and the
//! full-screen GPU/TUI front-ends.

use std::env;
use std::io;
use std::process::Command;

use weft_term::sys;

/// Guard that restores the console modes when dropped.
pub type RawInput = sys::RawMode;

/// Puts the console into raw input mode and enables VT output.
pub fn enter_raw() -> io::Result<RawInput> {
    sys::RawMode::enable()
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

/// Size of the console output buffer, in `(columns, rows)`.
pub fn window_size() -> io::Result<(usize, usize)> {
    sys::window_size()
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
