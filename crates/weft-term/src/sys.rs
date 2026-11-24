//! Thin platform layer: PTY creation, window size and terminal raw mode.
//!
//! These are the only `unsafe` calls in the engine. Every declaration mirrors
//! the C signature of the platform; callers get a safe, small API. Constants and
//! `termios` layouts differ per operating system, so each one is defined next to
//! its target instead of being imported from a C library wrapper.

use std::fs::File;
use std::io;
use std::os::fd::{FromRawFd, RawFd};

#[cfg(target_os = "linux")]
mod platform {
    pub const TIOCSCTTY: u64 = 0x540e;
    pub const TIOCGWINSZ: u64 = 0x5413;
    pub const TIOCSWINSZ: u64 = 0x5414;
    pub const TCSANOW: i32 = 0;
}

#[cfg(target_os = "macos")]
mod platform {
    pub const TIOCSCTTY: u64 = 0x2000_7461;
    pub const TIOCGWINSZ: u64 = 0x4008_7468;
    pub const TIOCSWINSZ: u64 = 0x8008_7467;
    pub const TCSANOW: i32 = 0;
}

/// `termios` as the C library lays it out on Linux (glibc/musl, x86_64/aarch64).
#[cfg(target_os = "linux")]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
struct Termios {
    c_iflag: u32,
    c_oflag: u32,
    c_cflag: u32,
    c_lflag: u32,
    c_line: u8,
    c_cc: [u8; 32],
    c_ispeed: u32,
    c_ospeed: u32,
}

/// `termios` on macOS: `tcflag_t` and `speed_t` are `unsigned long`.
#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
struct Termios {
    c_iflag: u64,
    c_oflag: u64,
    c_cflag: u64,
    c_lflag: u64,
    c_cc: [u8; 20],
    c_ispeed: u64,
    c_ospeed: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct WinSize {
    rows: u16,
    cols: u16,
    xpixel: u16,
    ypixel: u16,
}

unsafe extern "C" {
    fn posix_openpt(flags: i32) -> i32;
    fn grantpt(fd: i32) -> i32;
    fn unlockpt(fd: i32) -> i32;
    fn ptsname(fd: i32) -> *mut std::ffi::c_char;
    fn tcgetattr(fd: i32, termios: *mut Termios) -> i32;
    fn tcsetattr(fd: i32, action: i32, termios: *const Termios) -> i32;
    fn cfmakeraw(termios: *mut Termios);
    fn ioctl(fd: i32, request: u64, ...) -> i32;
    fn setsid() -> i32;
    fn isatty(fd: i32) -> i32;
}

const O_RDWR: i32 = 2;

/// Opens a pseudo-terminal pair. Returns `(master, slave, slave_path)`.
pub fn open_pty() -> io::Result<(File, File, String)> {
    // SAFETY: `posix_openpt` returns a new file descriptor or -1.
    let fd = unsafe { posix_openpt(O_RDWR) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the descriptor is valid and owned by us from here on.
    let master = unsafe { File::from_raw_fd(fd) };

    // SAFETY: plain ioctl-style calls on a descriptor we own.
    if unsafe { grantpt(fd) } != 0 || unsafe { unlockpt(fd) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `ptsname` returns a pointer to a static string for this fd.
    let name = unsafe { ptsname(fd) };
    if name.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the pointer is NUL-terminated and valid for the call.
    let path = unsafe { std::ffi::CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned();
    let slave = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)?;
    Ok((master, slave, path))
}

/// Size of the terminal behind `fd`, in `(columns, rows)`.
pub fn window_size(fd: RawFd) -> io::Result<(usize, usize)> {
    let mut size = WinSize::default();
    // SAFETY: `TIOCGWINSZ` writes a `struct winsize` into the passed pointer.
    let result = unsafe { ioctl(fd, platform::TIOCGWINSZ, std::ptr::addr_of_mut!(size)) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if size.cols == 0 || size.rows == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "terminal reported a zero size",
        ));
    }
    Ok((usize::from(size.cols), usize::from(size.rows)))
}

/// Resizes the terminal behind `fd`.
pub fn set_size(fd: RawFd, cols: usize, rows: usize) -> io::Result<()> {
    let size = WinSize {
        rows: rows.min(usize::from(u16::MAX)) as u16,
        cols: cols.min(usize::from(u16::MAX)) as u16,
        xpixel: 0,
        ypixel: 0,
    };
    // SAFETY: `TIOCSWINSZ` reads a `struct winsize` from the passed pointer.
    let result = unsafe { ioctl(fd, platform::TIOCSWINSZ, std::ptr::addr_of!(size)) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Whether `fd` refers to a terminal.
pub fn is_tty(fd: RawFd) -> bool {
    // SAFETY: `isatty` only inspects the descriptor.
    unsafe { isatty(fd) != 0 }
}

/// Starts a new session. Used in the child before `exec`.
pub fn new_session() -> i32 {
    // SAFETY: no memory is involved; the call only changes process attributes.
    unsafe { setsid() }
}

/// Makes the terminal behind `fd` the controlling terminal of the session.
pub fn take_control_tty(fd: RawFd) -> i32 {
    // SAFETY: `TIOCSCTTY` takes an int argument; zero means "this terminal".
    unsafe { ioctl(fd, platform::TIOCSCTTY, 0i32) }
}

/// Restores the original terminal state when dropped.
#[derive(Debug)]
pub struct RawMode {
    fd: RawFd,
    original: Termios,
}

impl RawMode {
    /// Puts the terminal into raw mode: no echo, no line buffering, no signal
    /// generation. The previous state is restored on drop.
    pub fn enable(fd: RawFd) -> io::Result<Self> {
        if !is_tty(fd) {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "not a terminal",
            ));
        }
        // SAFETY: `termios` is plain old data; zeroing is a valid initial state.
        let mut original: Termios = unsafe { std::mem::zeroed() };
        // SAFETY: the descriptor is a terminal and the pointer is valid.
        if unsafe { tcgetattr(fd, std::ptr::addr_of_mut!(original)) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut raw = original;
        // SAFETY: `cfmakeraw` mutates a caller-owned struct in place.
        unsafe { cfmakeraw(std::ptr::addr_of_mut!(raw)) };
        // SAFETY: same as above for `tcsetattr`.
        if unsafe { tcsetattr(fd, platform::TCSANOW, std::ptr::addr_of!(raw)) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { fd, original })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        // SAFETY: restores a snapshot previously returned by `tcgetattr`.
        unsafe {
            tcsetattr(self.fd, platform::TCSANOW, std::ptr::addr_of!(self.original));
        }
    }
}
