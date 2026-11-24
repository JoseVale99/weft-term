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

/// Shell used by the full-screen terminal.
pub fn default_shell() -> String {
    env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_owned())
}

/// Width of the terminal; fixed until the console is wired.
pub fn columns() -> usize {
    80
}

/// Whether standard input is a terminal.
pub fn is_interactive() -> bool {
    std::io::IsTerminal::is_terminal(&io::stdin())
}

/// Windows always uses the block shell until ConPTY is implemented.
pub fn interactive() -> io::Result<()> {
    crate::repl::run()
}
