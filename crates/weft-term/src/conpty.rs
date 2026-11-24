//! Pseudo-console session driver (Windows).
//!
//! The session owns a pseudo console, the two pipes the host reads and writes,
//! and the attached child process. Behaviour mirrors the Unix driver: `read`
//! blocks until bytes or end of stream, `resize` relays the new size, and
//! closing the session ends the child.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::process::ExitStatusExt;
use std::process::ExitStatus;

use crate::sys::Conpty;

/// One running shell attached to a pseudo console.
pub struct Session {
    input: File,
    output: File,
    conpty: Conpty,
    process: *mut core::ffi::c_void,
}

// SAFETY: the process handle is an opaque kernel object used only through its
// own API. The session is not shared across threads by the application.
unsafe impl Send for Session {}

// Clippy sees the raw handle; the safety argument is the one above.
impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").finish_non_exhaustive()
    }
}

fn exit_status(code: u32) -> ExitStatus {
    ExitStatus::from_raw(code)
}

impl Session {
    /// Spawns `program` with `args` attached to a new pseudo console.
    pub fn spawn(program: &str, args: &[&str], cols: usize, rows: usize) -> io::Result<Self> {
        let (conpty, input, output) = Conpty::open(cols, rows)?;

        // A single wide command line, Windows style.
        let mut command_line: Vec<u16> = Vec::new();
        push_quoted(&mut command_line, program);
        for arg in args {
            command_line.push(b' ' as u16);
            push_quoted(&mut command_line, arg);
        }
        command_line.push(0);

        let process = conpty.spawn(&command_line)?;
        Ok(Self {
            input,
            output,
            conpty,
            process,
        })
    }

    /// Sends bytes to the child, as typed input.
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.input.write_all(bytes)?;
        self.input.flush()
    }

    /// Reads whatever the child produced. Returns `Ok(0)` on end of stream.
    pub fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.output.read(buffer)
    }

    /// Duplicates the output side so a reader thread can own it.
    pub fn try_clone_master(&self) -> io::Result<File> {
        self.output.try_clone()
    }

    /// Informs the child of a new terminal size.
    pub fn resize(&self, cols: usize, rows: usize) -> io::Result<()> {
        self.conpty.resize(cols, rows)
    }

    /// Non-blocking check for the child's exit status.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        Ok(self.conpty.try_wait(self.process)?.map(exit_status))
    }

    /// Terminates the child shell.
    pub fn kill(&mut self) {
        self.conpty.kill(self.process);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.conpty.kill(self.process);
    }
}

/// Appends `text` in UTF-16, quoted when it contains spaces, Windows style.
fn push_quoted(out: &mut Vec<u16>, text: &str) {
    let needs_quotes = text.is_empty() || text.chars().any(char::is_whitespace);
    if needs_quotes {
        out.push(b'"' as u16);
    }
    for unit in text.encode_utf16() {
        if unit == b'"' as u16 {
            out.push(b'"' as u16);
        }
        out.push(unit);
    }
    if needs_quotes {
        out.push(b'"' as u16);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The console spawns, echoes input and can be killed without hanging.
    #[test]
    fn a_session_spawns_and_reports_output() {
        let mut session =
            Session::spawn("cmd.exe", &["/C", "echo conpty-ok"], 80, 24).expect("spawn");
        // Reading is blocking, so the deadline only works if the read happens
        // on another thread: a console that never delivers output or EOF must
        // fail the test instead of hanging the run.
        let mut reader = session.try_clone_master().expect("clone");
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 || sender.send(buffer[..count].to_vec()).is_err() {
                    break;
                }
            }
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut output = Vec::new();
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match receiver.recv_timeout(left) {
                Ok(chunk) => {
                    output.extend_from_slice(&chunk);
                    if String::from_utf8_lossy(&output).contains("conpty-ok") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let output = String::from_utf8_lossy(&output).into_owned();
        assert!(output.contains("conpty-ok"), "output was {output:?}");
        let mut exit = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if let Some(status) = session.try_wait().expect("try_wait") {
                exit = Some(status);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(exit.is_some(), "cmd.exe did not exit");
    }
}
