//! PTY session driver (Unix).
//!
//! A session owns the master side of a pseudo-terminal and the child shell. The
//! application reads bytes from the master, feeds them to the terminal and
//! writes user input back. All platform differences live in [`crate::sys`].

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use crate::sys;

/// One running shell attached to a pseudo-terminal.
#[derive(Debug)]
pub struct Session {
    master: File,
    child: Child,
}

impl Session {
    /// Spawns `program` with `args` on a new PTY of the given size.
    pub fn spawn(program: &str, args: &[&str], cols: usize, rows: usize) -> io::Result<Self> {
        let (master, slave, _path) = sys::open_pty()?;
        sys::set_size(slave.as_raw_fd(), cols, rows)?;

        let stdin = Stdio::from(slave.try_clone()?);
        let stdout = Stdio::from(slave.try_clone()?);
        let stderr = Stdio::from(slave);

        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(stdin)
            .stdout(stdout)
            .stderr(stderr)
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor");

        // SAFETY: `pre_exec` runs in the child between fork and exec. It only
        // calls async-signal-safe functions (`setsid`, `ioctl`) and allocates
        // nothing, which is the documented requirement.
        unsafe {
            command.pre_exec(|| {
                if sys::new_session() < 0 {
                    return Err(io::Error::last_os_error());
                }
                if sys::take_control_tty(0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let child = command.spawn()?;
        Ok(Self { master, child })
    }

    /// Sends bytes to the child, as typed input.
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.master.write_all(bytes)?;
        self.master.flush()
    }

    /// Reads whatever the child produced. Returns `Ok(0)` on end of stream.
    pub fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.master.read(buffer)
    }

    /// Duplicates the master side so a reader thread can own it.
    pub fn try_clone_master(&self) -> io::Result<File> {
        self.master.try_clone()
    }

    /// Informs the child of a new terminal size.
    pub fn resize(&self, cols: usize, rows: usize) -> io::Result<()> {
        sys::set_size(self.master.as_raw_fd(), cols, rows)
    }

    /// Non-blocking check for the child's exit status.
    pub fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }

    /// Terminates the child shell.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Closing the master would hang the child on its next write; end it.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
