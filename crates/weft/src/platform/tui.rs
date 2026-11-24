//! Interactive full-screen front-end (Unix).
//!
//! One reader thread per shell, one reader thread for the keyboard, and a main
//! thread that owns all terminal state. Each tab is an independent PTY plus its
//! own screen model; only the active tab is rendered and receives input.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use weft_term::BlockState;
use weft_term::pty::Session;
use weft_term::sys;
use weft_term::vt::Terminal;

use crate::keys::{Decoder, Key};
use crate::render;
use crate::theme::{self, Theme};

const READ_BUFFER: usize = 16 * 1024;
const STATUS_HEIGHT: usize = 1;

enum Message {
    /// Bytes typed by the user.
    Input(Vec<u8>),
    /// Standard input closed: the application is over.
    InputClosed,
    /// Bytes produced by one tab's shell.
    Output { tab: usize, bytes: Vec<u8> },
    /// One tab's shell closed.
    Closed { tab: usize },
}

/// One PTY and its screen state.
struct Tab {
    id: usize,
    title: String,
    session: Session,
    terminal: Terminal,
}

/// One entry in the command palette.
#[derive(Clone, Copy)]
enum Command {
    NewTab,
    NextTab,
    CloseTab,
    CycleTheme,
    ClearScreen,
    ResetTerminal,
    Quit,
}

const PALETTE: [(&str, Command); 7] = [
    ("New tab", Command::NewTab),
    ("Next tab", Command::NextTab),
    ("Close tab", Command::CloseTab),
    ("Cycle theme (dark/light)", Command::CycleTheme),
    ("Clear screen", Command::ClearScreen),
    ("Reset terminal", Command::ResetTerminal),
    ("Quit weft", Command::Quit),
];

struct Palette {
    selected: usize,
    decoder: Decoder,
}

enum PaletteEvent {
    Moved,
    Close,
    Choose(Command),
}

impl Palette {
    fn new() -> Self {
        Self {
            selected: 0,
            decoder: Decoder::new(),
        }
    }

    /// Decodes only palette keys and ignores everything else.
    fn feed(&mut self, bytes: &[u8]) -> Option<PaletteEvent> {
        self.decoder.push(bytes);
        while let Some(key) = self.decoder.next() {
            match key {
                Key::Up | Key::CtrlP => {
                    self.selected = self.selected.saturating_sub(1);
                    return Some(PaletteEvent::Moved);
                }
                Key::Down | Key::CtrlN => {
                    self.selected = (self.selected + 1).min(PALETTE.len() - 1);
                    return Some(PaletteEvent::Moved);
                }
                Key::Enter => return Some(PaletteEvent::Choose(PALETTE[self.selected].1)),
                Key::Esc | Key::CtrlC => return Some(PaletteEvent::Close),
                _ => {}
            }
        }
        None
    }
}

fn is_palette_key(bytes: &[u8]) -> bool {
    // F1 is unambiguous in raw mode and shells do not need it to edit a line.
    bytes == b"\x1bOP" || bytes == b"\x1b[11~"
}

/// Runs the interactive terminal until the last shell exits.
pub fn run() -> io::Result<()> {
    let config = weft_config::Config::load(crate::config_path()).unwrap_or_default();
    let mut theme: &Theme = theme::resolve(&config.terminal.theme);
    let shell = config
        .terminal
        .shell
        .filter(|shell| !shell.trim().is_empty())
        .unwrap_or_else(crate::platform::default_shell);
    let (cols, rows) = sys::window_size(0).unwrap_or((80, 24));
    let shell_rows = rows.saturating_sub(STATUS_HEIGHT).max(1);

    let (sender, receiver) = mpsc::channel();
    let mut next_id = 0;
    let mut tabs = vec![spawn_tab(next_id, &shell, cols, shell_rows, &sender)?];
    next_id += 1;
    let mut active = 0;

    let raw = sys::RawMode::enable(0)?;

    // Reader thread for the keyboard.
    let input_sender = sender.clone();
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
        let _ = input_sender.send(Message::InputClosed);
    });

    enter_screen()?;
    let mut dirty = true;
    let mut window_title = String::new();
    let mut palette: Option<Palette> = None;
    let mut notice = String::new();
    let mut quit = false;
    loop {
        if dirty {
            {
                let tab = &mut tabs[active];
                let title = &tab.terminal.grid.title;
                if !title.is_empty() && *title != tab.title {
                    tab.title.clone_from(title);
                }
            }
            let labels: Vec<String> = tabs
                .iter()
                .enumerate()
                .map(|(index, tab)| format!("{}:{}", index + 1, tab.title))
                .collect();
            let tab = &mut tabs[active];
            if tab.title != window_title {
                window_title.clone_from(&tab.title);
                set_title(&window_title)?;
            }
            let mut frame =
                String::with_capacity(tab.terminal.grid.cols() * (tab.terminal.grid.rows() + 1) * 4);
            let status = if notice.is_empty() {
                status_line(&tab.terminal, &labels, active)
            } else {
                std::mem::take(&mut notice)
            };
            render::frame(&tab.terminal.grid, &status, theme, &mut frame);
            if let Some(open) = &palette {
                let items: Vec<&str> = PALETTE.iter().map(|(label, _)| *label).collect();
                render::palette(
                    &items,
                    open.selected,
                    theme,
                    tab.terminal.grid.cols(),
                    &mut frame,
                );
            }
            let mut stdout = io::stdout();
            stdout.write_all(frame.as_bytes())?;
            stdout.flush()?;
            dirty = false;
        }

        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(Message::Input(bytes)) => {
                if let Some(open) = palette.as_mut() {
                    match open.feed(&bytes) {
                        Some(PaletteEvent::Choose(command)) => {
                            palette = None;
                            match command {
                                Command::NewTab => {
                                    let cols = tabs[active].terminal.grid.cols();
                                    let rows = tabs[active].terminal.grid.rows();
                                    match spawn_tab(next_id, &shell, cols, rows, &sender) {
                                        Ok(tab) => {
                                            tabs.push(tab);
                                            active = tabs.len() - 1;
                                            next_id += 1;
                                        }
                                        Err(error) => {
                                            notice = format!("could not open tab: {error}");
                                        }
                                    }
                                }
                                Command::NextTab => {
                                    if !tabs.is_empty() {
                                        active = (active + 1) % tabs.len();
                                    }
                                }
                                Command::CloseTab => {
                                    let id = tabs[active].id;
                                    if remove_tab(&mut tabs, id, &mut active) {
                                        quit = true;
                                    }
                                }
                                Command::CycleTheme => {
                                    theme = if theme.name == theme::LIGHT.name {
                                        &theme::DARK
                                    } else {
                                        &theme::LIGHT
                                    };
                                }
                                Command::ClearScreen => {
                                    let _ = tabs[active].terminal.feed(b"\x1b[2J\x1b[H");
                                }
                                Command::ResetTerminal => {
                                    let _ = tabs[active].terminal.feed(b"\x1bc");
                                }
                                Command::Quit => quit = true,
                            }
                        }
                        Some(PaletteEvent::Close) => palette = None,
                        Some(PaletteEvent::Moved) | None => {}
                    }
                } else if is_palette_key(&bytes) {
                    palette = Some(Palette::new());
                } else {
                    // The shell owns its line editor, so input is forwarded intact,
                    // escape sequences included.
                    tabs[active].session.write(&bytes)?;
                }
                dirty = true;
            }
            Ok(Message::Output { tab, bytes }) => {
                if let Some(target) = tabs.iter_mut().find(|target| target.id == tab) {
                    let responses = target.terminal.feed(&bytes);
                    if !responses.is_empty() {
                        target.session.write(&responses)?;
                    }
                }
                dirty = true;
            }
            Ok(Message::Closed { tab }) => {
                if remove_tab(&mut tabs, tab, &mut active) {
                    break;
                }
                dirty = true;
            }
            Ok(Message::InputClosed) => break,
            Err(RecvTimeoutError::Timeout) => {
                if let Ok((cols, rows)) = sys::window_size(0) {
                    let rows = rows.saturating_sub(STATUS_HEIGHT).max(1);
                    if let Some(first) = tabs.first()
                        && (cols != first.terminal.grid.cols()
                            || rows != first.terminal.grid.rows())
                    {
                        for tab in &mut tabs {
                            tab.terminal.resize(cols, rows);
                            let _ = tab.session.resize(cols, rows);
                        }
                        dirty = true;
                    }
                }
                let exited: Vec<usize> = tabs
                    .iter_mut()
                    .filter_map(|tab| match tab.session.try_wait() {
                        Ok(Some(_)) => Some(tab.id),
                        _ => None,
                    })
                    .collect();
                for id in exited {
                    if remove_tab(&mut tabs, id, &mut active) {
                        quit = true;
                        break;
                    }
                    dirty = true;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }

        if quit {
            break;
        }
    }

    leave_screen()?;
    drop(raw);
    println!("weft: session ended");
    Ok(())
}

fn spawn_tab(
    id: usize,
    shell: &str,
    cols: usize,
    rows: usize,
    sender: &mpsc::Sender<Message>,
) -> io::Result<Tab> {
    let session = Session::spawn(shell, &[], cols, rows)?;
    let mut master = session.try_clone_master()?;
    let output_sender = sender.clone();
    thread::spawn(move || {
        let mut buffer = [0u8; READ_BUFFER];
        loop {
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let message = Message::Output {
                        tab: id,
                        bytes: buffer[..count].to_vec(),
                    };
                    if output_sender.send(message).is_err() {
                        return;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = output_sender.send(Message::Closed { tab: id });
    });
    Ok(Tab {
        id,
        title: shell_title(shell),
        session,
        terminal: Terminal::new(cols, rows),
    })
}

/// Removes a tab and fixes the active index. Returns true when none are left.
fn remove_tab(tabs: &mut Vec<Tab>, id: usize, active: &mut usize) -> bool {
    let Some(position) = tabs.iter().position(|tab| tab.id == id) else {
        return tabs.is_empty();
    };
    tabs.remove(position);
    if tabs.is_empty() {
        return true;
    }
    if *active > position {
        *active -= 1;
    }
    *active = (*active).min(tabs.len() - 1);
    false
}

fn shell_title(shell: &str) -> String {
    Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| shell.to_owned(), str::to_owned)
}

/// Renders the block state and the tab list into the bottom bar.
fn status_line(terminal: &Terminal, tabs: &[String], active: usize) -> String {
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
    let tabs: Vec<String> = tabs
        .iter()
        .enumerate()
        .map(|(index, name)| {
            if index == active {
                format!("[{name}*]")
            } else {
                format!("[{name}]")
            }
        })
        .collect();
    let mut line = String::with_capacity(terminal.grid.cols());
    let _ = write!(
        line,
        " {} | {cwd} | {state} | blocks {} | F1 palette",
        tabs.join(" "),
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
