//! PTY session driver.
//!
//! A session owns the host side of a pseudo-terminal and the child shell. The
//! application reads bytes from the host side, feeds them to the terminal and
//! writes user input back. Platform differences live in the `unix` and
//! `windows` child modules; `Session` is the only public name.

#[cfg(unix)]
mod unix;

#[cfg(unix)]
pub use unix::Session;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::Session;
