//! Interactive full-screen front-end (Unix).
//!
//! One thread reads the user's terminal, one thread reads the PTY, and the main
//! thread owns all terminal state. No locks, no shared mutation, and the
//! renderer never blocks on the shell.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use weft_term::BlockState;
use weft_term::pty::Session;
use weft_term::sys;
use weft_term::vt::Terminal;

use crate::render;

const READ_BUFFER: usize = 16 * 1024;
const STATUS_HEIGHT: usize = 1;

enum Message {
    /// Bytes typed by the user.
    Input(Vec<u8>),
    /// Bytes produced by the shell.
    Output(Vec<u8>),
    /// One side closed: the session is over.
    Closed,
}

/// Runs the interactive terminal until the shell exits.
pub fn run() -> io::Result<()> {
    let config = weft_config::Config::load(crate::config_path()).unwrap_or_default();
    let theme = crate::theme::resolve(&config.terminal.theme);
    let shell = config
        .terminal
        .shell
        .filter(|shell| !shell.trim().is_empty())
        .unwrap_or_else(crate::platform::default_shell);
    let (cols, rows) = sys::window_size(0).unwrap_or((80, 24));
    let shell_rows = rows.saturating_sub(STATUS_HEIGHT).max(1);

    let mut session = Session::spawn(&shell, &[], cols, shell_rows)?;
    let raw = sys::RawMode::enable(0)?;
    let mut terminal = Terminal::new(cols, shell_rows);

    let (sender, receiver) = mpsc::channel();

    // Reader thread for the shell.
    let mut master = session.try_clone_master()?;
    let output_sender = sender.clone();
    thread::spawn(move || {
        let mut buffer = [0u8; READ_BUFFER];
        loop {
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if output_sender
                        .send(Message::Output(buffer[..count].to_vec()))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = output_sender.send(Message::Closed);
    });

    // Reader thread for the keyboard.
    let input_sender = sender;
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut stdin = stdin.lock();
        let mut buffer = [0u8; READ_BUFFER];
        loop {
            match stdin.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if input_sender
                        .send(Message::Input(buffer[..count].to_vec()))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = input_sender.send(Message::Closed);
    });

    enter_screen()?;
    let mut dirty = true;
    let mut title = String::new();
    loop {
        if dirty {
            if terminal.grid.title != title {
                title.clone_from(&terminal.grid.title);
                set_title(&title)?;
            }
            let mut frame =
                String::with_capacity(terminal.grid.cols() * (terminal.grid.rows() + 1) * 4);
            let status = status_line(&terminal, terminal.grid.cols());
            render::frame(&terminal.grid, &status, theme, &mut frame);
            let mut stdout = io::stdout();
            stdout.write_all(frame.as_bytes())?;
            stdout.flush()?;
            dirty = false;
        }

        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(Message::Input(bytes)) => {
                // The shell owns its line editor, so input is forwarded intact,
                // escape sequences included.
                session.write(&bytes)?;
                dirty = true;
            }
            Ok(Message::Output(bytes)) => {
                let responses = terminal.feed(&bytes);
                if !responses.is_empty() {
                    session.write(&responses)?;
                }
                dirty = true;
            }
            Ok(Message::Closed) => break,
            Err(RecvTimeoutError::Timeout) => {
                if let Ok((cols, rows)) = sys::window_size(0) {
                    let rows = rows.saturating_sub(STATUS_HEIGHT).max(1);
                    if cols != terminal.grid.cols() || rows != terminal.grid.rows() {
                        terminal.resize(cols, rows);
                        let _ = session.resize(cols, rows);
                        dirty = true;
                    }
                }
                if let Ok(Some(_status)) = session.try_wait() {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    leave_screen()?;
    drop(raw);
    println!("weft: session ended");
    Ok(())
}

/// Renders the block state into the bottom bar.
fn status_line(terminal: &Terminal, cols: usize) -> String {
    let cwd = terminal
        .blocks
        .current()
        .map(|block| block.cwd.as_str())
        .filter(|cwd| !cwd.is_empty())
        .unwrap_or("~");
    let state = match terminal.blocks.current() {
        Some(block) => match (block.state, block.exit_code) {
            (BlockState::Running, _) => "running".to_owned(),
            (BlockState::Done, Some(code)) => format!("exit {code}"),
            _ => "ready".to_owned(),
        },
        None => "ready".to_owned(),
    };
    let mut line = String::with_capacity(cols);
    let _ = write!(
        line,
        " weft | {cwd} | {state} | blocks {}",
        terminal.blocks.len()
    );
    line
}

fn set_title(title: &str) -> io::Result<()> {
    let mut stdout = io::stdout();
    if title.is_empty() {
        stdout.write_all(b"\x1b]0;weft\x07")?;
    } else {
        write!(stdout, "\x1b]0;{title} - weft\x07")?;
    }
    stdout.flush()
}

fn enter_screen() -> io::Result<()> {
    let mut stdout = io::stdout();
    // Alternate screen keeps the user's previous terminal contents intact.
    stdout.write_all(b"\x1b[?1049h\x1b[?25l\x1b[2J\x1b[H")?;
    stdout.flush()
}

fn leave_screen() -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l")?;
    stdout.flush()
}
