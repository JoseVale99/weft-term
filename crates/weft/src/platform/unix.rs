//! Unix glue: `sh`, `$SHELL`, `HOME`, termios raw mode and the full-screen UI.

use std::env;
use std::io;
use std::process::Command;

use weft_term::sys;

/// Guard that restores the terminal when dropped.
pub type RawInput = sys::RawMode;

/// Switches the terminal to raw mode.
pub fn enter_raw() -> io::Result<RawInput> {
    sys::RawMode::enable(0)
}

/// Builds the command used to run a shell line.
pub fn command(line: &str) -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(line);
    command
}

/// The user's home directory, or `.` when it cannot be determined.
pub fn home() -> String {
    env::var("HOME").unwrap_or_else(|_| ".".to_owned())
}

/// Shell used by the full-screen terminal.
pub fn default_shell() -> String {
    env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())
}

/// Width of the terminal, or 80 columns when it cannot be measured.
pub fn columns() -> usize {
    sys::window_size(0).map_or(80, |(columns, _)| columns)
}

/// Whether standard input is a terminal.
pub fn is_interactive() -> bool {
    sys::is_tty(0)
}

/// Runs the front-end that fits the environment.
pub fn interactive() -> io::Result<()> {
    if is_interactive() {
        super::tui::run()
    } else {
        crate::repl::run()
    }
}
