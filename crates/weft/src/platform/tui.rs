//! Interactive full-screen front-end (Unix).
//!
//! One reader thread per pane, one reader thread for the keyboard, and a main
//! thread that owns all terminal state. A tab holds one or more stacked panes;
//! each pane has its own PTY and screen model. Only the active pane receives
//! input, and every pane keeps its own scroll region.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use weft_term::BlockState;
use weft_term::layout::{Orientation, Rect, SplitTree};
use weft_term::pty::Session;
use weft_term::sys;
use weft_term::vt::Terminal;

use crate::keys::{Decoder, Key};
use crate::render::{self, PaneView};
use crate::theme::{self, Theme};

const READ_BUFFER: usize = 16 * 1024;
const STATUS_HEIGHT: usize = 1;

enum Message {
    /// Bytes typed by the user.
    Input(Vec<u8>),
    /// Standard input closed: the application is over.
    InputClosed,
    /// Bytes produced by one pane's shell.
    Output {
        tab: usize,
        pane: usize,
        bytes: Vec<u8>,
    },
    /// One pane's shell closed.
    Closed { tab: usize, pane: usize },
}

/// One PTY and its screen state.
struct Pane {
    id: usize,
    session: Session,
    terminal: Terminal,
}

/// A tab with one or more stacked panes.
struct Tab {
    id: usize,
    title: String,
    panes: Vec<Pane>,
    focus: usize,
    tree: SplitTree,
}

/// One entry in the command palette.
#[derive(Clone, Copy)]
enum Command {
    NewTab,
    NextTab,
    CloseTab,
    SplitDown,
    SplitRight,
    NextPane,
    ClosePane,
    GrowPane,
    ShrinkPane,
    CycleTheme,
    ClearScreen,
    ResetTerminal,
    Quit,
}

const PALETTE: [(&str, Command); 13] = [
    ("New tab", Command::NewTab),
    ("Next tab", Command::NextTab),
    ("Close tab", Command::CloseTab),
    ("Split down", Command::SplitDown),
    ("Split right", Command::SplitRight),
    ("Next pane", Command::NextPane),
    ("Close pane", Command::ClosePane),
    ("Grow pane", Command::GrowPane),
    ("Shrink pane", Command::ShrinkPane),
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

    /// Decodes palette keys in order; a chunk may contain several of them.
    fn feed(&mut self, bytes: &[u8]) -> Option<PaletteEvent> {
        self.decoder.push(bytes);
        let mut event = None;
        while let Some(key) = self.decoder.next() {
            match key {
                Key::Up | Key::CtrlP => {
                    self.selected = self.selected.saturating_sub(1);
                    event = Some(PaletteEvent::Moved);
                }
                Key::Down | Key::CtrlN => {
                    self.selected = (self.selected + 1).min(PALETTE.len() - 1);
                    event = Some(PaletteEvent::Moved);
                }
                Key::Enter => return Some(PaletteEvent::Choose(PALETTE[self.selected].1)),
                Key::Esc | Key::CtrlC => return Some(PaletteEvent::Close),
                _ => {}
            }
        }
        event
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
    let mut dimensions = (cols, rows.saturating_sub(STATUS_HEIGHT).max(1));

    let (sender, receiver) = mpsc::channel();
    let mut next_tab = 0;
    let mut next_pane = 0;

    let first = spawn_pane(next_pane, 0, &shell, dimensions.0, dimensions.1, &sender)?;
    let first_id = first.id;
    next_pane += 1;
    let mut tabs = vec![Tab {
        id: 0,
        title: shell_title(&shell),
        panes: vec![first],
        focus: first_id,
        tree: SplitTree::new(first_id),
    }];
    next_tab += 1;
    let mut active_tab = 0;

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
                let tab = &mut tabs[active_tab];
                if let Some(pane) = tab.panes.iter().find(|pane| pane.id == tab.focus) {
                    let title = &pane.terminal.grid.title;
                    if !title.is_empty() && *title != tab.title {
                        tab.title.clone_from(title);
                    }
                }
            }
            let labels: Vec<String> = tabs
                .iter()
                .enumerate()
                .map(|(index, tab)| format!("{}:{}", index + 1, tab.title))
                .collect();
            let tab = &tabs[active_tab];
            if tab.title != window_title {
                window_title.clone_from(&tab.title);
                set_title(&window_title)?;
            }
            let area = Rect {
                x: 0,
                y: 0,
                width: dimensions.0,
                height: dimensions.1,
            };
            let layout = tab.tree.layout(area);
            let mut views = Vec::with_capacity(layout.panes.len());
            for (id, rect) in &layout.panes {
                if let Some(pane) = tab.panes.iter().find(|pane| pane.id == *id) {
                    views.push(PaneView {
                        grid: &pane.terminal.grid,
                        active: *id == tab.focus,
                        row: rect.y,
                        column: rect.x,
                    });
                }
            }
            let mut frame =
                String::with_capacity(dimensions.0 * (dimensions.1 + STATUS_HEIGHT) * 4);
            let leaves = tab.tree.leaves();
            let position = leaves
                .iter()
                .position(|id| *id == tab.focus)
                .unwrap_or(0);
            let focused = tab.panes.iter().find(|pane| pane.id == tab.focus);
            let status = if notice.is_empty() {
                match focused {
                    Some(pane) => status_line(
                        &pane.terminal,
                        &labels,
                        active_tab,
                        position,
                        leaves.len(),
                    ),
                    None => "no pane".to_owned(),
                }
            } else {
                std::mem::take(&mut notice)
            };
            render::split(
                &views,
                &layout.separators,
                dimensions.1,
                dimensions.0,
                &status,
                theme,
                &mut frame,
            );
            if let Some(open) = &palette {
                let items: Vec<&str> = PALETTE.iter().map(|(label, _)| *label).collect();
                render::palette(&items, open.selected, theme, dimensions.0, &mut frame);
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
                            apply(
                                command,
                                &mut tabs,
                                &mut active_tab,
                                &mut next_tab,
                                &mut next_pane,
                                &shell,
                                &sender,
                                &mut theme,
                                dimensions,
                                &mut notice,
                                &mut quit,
                            )?;
                        }
                        Some(PaletteEvent::Close) => palette = None,
                        Some(PaletteEvent::Moved) | None => {}
                    }
                } else if is_palette_key(&bytes) {
                    palette = Some(Palette::new());
                } else {
                    let tab = &mut tabs[active_tab];
                    if let Some(pane) = tab.panes.iter_mut().find(|pane| pane.id == tab.focus) {
                        pane.session.write(&bytes)?;
                    }
                }
                dirty = true;
            }
            Ok(Message::Output { tab, pane, bytes }) => {
                if let Some(target) = tabs.iter_mut().find(|target| target.id == tab)
                    && let Some(target) = target.panes.iter_mut().find(|target| target.id == pane)
                {
                    let responses = target.terminal.feed(&bytes);
                    if !responses.is_empty() {
                        target.session.write(&responses)?;
                    }
                }
                dirty = true;
            }
            Ok(Message::Closed { tab, pane }) => {
                if remove_pane(&mut tabs, tab, pane, &mut active_tab, dimensions) {
                    break;
                }
                dirty = true;
            }
            Ok(Message::InputClosed) => break,
            Err(RecvTimeoutError::Timeout) => {
                if let Ok((cols, rows)) = sys::window_size(0) {
                    let rows = rows.saturating_sub(STATUS_HEIGHT).max(1);
                    if (cols, rows) != dimensions {
                        dimensions = (cols, rows);
                        for tab in &mut tabs {
                            resize_tab(tab, dimensions);
                        }
                        dirty = true;
                    }
                }
                let mut exited = Vec::new();
                for tab in &mut tabs {
                    for pane in &mut tab.panes {
                        if let Ok(Some(_)) = pane.session.try_wait() {
                            exited.push((tab.id, pane.id));
                        }
                    }
                }
                for (tab, pane) in exited {
                    if remove_pane(&mut tabs, tab, pane, &mut active_tab, dimensions) {
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

#[allow(clippy::too_many_arguments)]
fn apply(
    command: Command,
    tabs: &mut Vec<Tab>,
    active_tab: &mut usize,
    next_tab: &mut usize,
    next_pane: &mut usize,
    shell: &str,
    sender: &mpsc::Sender<Message>,
    theme: &mut &Theme,
    dimensions: (usize, usize),
    notice: &mut String,
    quit: &mut bool,
) -> io::Result<()> {
    match command {
        Command::NewTab => {
            match spawn_pane(
                *next_pane,
                *next_tab,
                shell,
                dimensions.0,
                dimensions.1,
                sender,
            ) {
                Ok(pane) => {
                    let pane_id = pane.id;
                    tabs.push(Tab {
                        id: *next_tab,
                        title: shell_title(shell),
                        panes: vec![pane],
                        focus: pane_id,
                        tree: SplitTree::new(pane_id),
                    });
                    *active_tab = tabs.len() - 1;
                    *next_tab += 1;
                    *next_pane += 1;
                }
                Err(error) => *notice = format!("could not open tab: {error}"),
            }
        }
        Command::NextTab => {
            if !tabs.is_empty() {
                *active_tab = (*active_tab + 1) % tabs.len();
            }
        }
        Command::CloseTab => {
            let id = tabs[*active_tab].id;
            if remove_tab(tabs, id, active_tab) {
                *quit = true;
            }
        }
        Command::SplitDown | Command::SplitRight => {
            let orientation = if matches!(command, Command::SplitDown) {
                Orientation::Stacked
            } else {
                Orientation::SideBySide
            };
            let tab = &mut tabs[*active_tab];
            let area = Rect {
                x: 0,
                y: 0,
                width: dimensions.0,
                height: dimensions.1,
            };
            let mut preview = tab.tree.clone();
            if !preview.split(tab.focus, *next_pane, orientation) {
                *notice = "pane is no longer available".to_owned();
                return Ok(());
            }
            let candidate = preview.layout(area);
            let mut fits = false;
            for (id, rect) in &candidate.panes {
                if *id == *next_pane {
                    fits = match orientation {
                        Orientation::Stacked => rect.height >= 3,
                        Orientation::SideBySide => rect.width >= 20,
                    };
                }
            }
            if !fits {
                *notice = format!(
                    "not enough room for another pane to the {}",
                    orientation.label()
                );
                return Ok(());
            }
            let focused = tab.panes.iter().find(|pane| pane.id == tab.focus);
            let (cols, rows) = match focused {
                Some(pane) => (pane.terminal.grid.cols(), pane.terminal.grid.rows()),
                None => (dimensions.0, dimensions.1),
            };
            match spawn_pane(*next_pane, tab.id, shell, cols, rows, sender) {
                Ok(pane) => {
                    let id = pane.id;
                    if !tab.tree.split(tab.focus, id, orientation) {
                        *notice = "could not place the new pane".to_owned();
                        return Ok(());
                    }
                    tab.focus = id;
                    tab.panes.push(pane);
                    *next_pane += 1;
                    resize_tab(tab, dimensions);
                }
                Err(error) => *notice = format!("could not split pane: {error}"),
            }
        }
        Command::NextPane => {
            let tab = &mut tabs[*active_tab];
            let leaves = tab.tree.leaves();
            if leaves.len() > 1
                && let Some(position) = leaves.iter().position(|id| *id == tab.focus)
            {
                tab.focus = leaves[(position + 1) % leaves.len()];
            }
        }
        Command::ClosePane => {
            let tab_id = tabs[*active_tab].id;
            let pane = tabs[*active_tab].focus;
            if tabs[*active_tab].panes.len() == 1 {
                if remove_tab(tabs, tab_id, active_tab) {
                    *quit = true;
                }
                return Ok(());
            }
            let tab = &mut tabs[*active_tab];
            let Some(next) = tab.tree.remove(pane) else {
                return Ok(());
            };
            tab.panes.retain(|found| found.id != pane);
            tab.focus = next;
            resize_tab(tab, dimensions);
        }
        Command::GrowPane | Command::ShrinkPane => {
            let delta = if matches!(command, Command::GrowPane) {
                5
            } else {
                -5
            };
            let tab = &mut tabs[*active_tab];
            if tab.tree.resize(tab.focus, delta) {
                resize_tab(tab, dimensions);
            } else {
                *notice = "no split to resize".to_owned();
            }
        }
        Command::CycleTheme => {
            *theme = if theme.name == theme::LIGHT.name {
                &theme::DARK
            } else {
                &theme::LIGHT
            };
        }
        Command::ClearScreen => {
            let tab = &mut tabs[*active_tab];
            if let Some(pane) = tab.panes.iter_mut().find(|pane| pane.id == tab.focus) {
                let _ = pane.terminal.feed(b"\x1b[2J\x1b[H");
            }
        }
        Command::ResetTerminal => {
            let tab = &mut tabs[*active_tab];
            if let Some(pane) = tab.panes.iter_mut().find(|pane| pane.id == tab.focus) {
                let _ = pane.terminal.feed(b"\x1bc");
            }
        }
        Command::Quit => *quit = true,
    }
    Ok(())
}

fn spawn_pane(
    pane: usize,
    tab: usize,
    shell: &str,
    cols: usize,
    rows: usize,
    sender: &mpsc::Sender<Message>,
) -> io::Result<Pane> {
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
                        tab,
                        pane,
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
        let _ = output_sender.send(Message::Closed { tab, pane });
    });
    Ok(Pane {
        id: pane,
        session,
        terminal: Terminal::new(cols, rows),
    })
}

fn resize_tab(tab: &mut Tab, dimensions: (usize, usize)) {
    let area = Rect {
        x: 0,
        y: 0,
        width: dimensions.0,
        height: dimensions.1,
    };
    let layout = tab.tree.layout(area);
    if layout.panes.len() != tab.panes.len() {
        return;
    }
    for (id, rect) in layout.panes {
        if let Some(pane) = tab.panes.iter_mut().find(|pane| pane.id == id) {
            pane.terminal.resize(rect.width, rect.height);
            let _ = pane.session.resize(rect.width, rect.height);
        }
    }
}

/// Removes a whole tab. Returns true when none are left.
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

/// Removes one pane, and its tab when it was the last one. Returns true when
/// no tabs remain.
fn remove_pane(
    tabs: &mut Vec<Tab>,
    tab_id: usize,
    pane_id: usize,
    active: &mut usize,
    dimensions: (usize, usize),
) -> bool {
    let Some(position) = tabs.iter().position(|tab| tab.id == tab_id) else {
        return tabs.is_empty();
    };
    let tab = &mut tabs[position];
    let Some(pane) = tab.panes.iter().position(|pane| pane.id == pane_id) else {
        return false;
    };
    if tab.panes.len() == 1 {
        tab.panes.remove(pane);
        tabs.remove(position);
        if tabs.is_empty() {
            return true;
        }
        if *active > position {
            *active -= 1;
        }
        *active = (*active).min(tabs.len() - 1);
        return false;
    }
    let was_focused = tab.focus == pane_id;
    let next = tab.tree.remove(pane_id);
    tab.panes.remove(pane);
    if was_focused {
        tab.focus = next.or_else(|| tab.tree.leaves().first().copied()).unwrap_or(0);
    } else if !tab.tree.contains(tab.focus) {
        tab.focus = tab.tree.leaves().first().copied().unwrap_or(0);
    }
    resize_tab(tab, dimensions);
    false
}

fn shell_title(shell: &str) -> String {
    Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| shell.to_owned(), str::to_owned)
}

/// Renders the block state, tab list and pane position into the bottom bar.
fn status_line(
    terminal: &Terminal,
    tabs: &[String],
    active_tab: usize,
    pane: usize,
    pane_count: usize,
) -> String {
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
            if index == active_tab {
                format!("[{name}*]")
            } else {
                format!("[{name}]")
            }
        })
        .collect();
    let mut line = String::with_capacity(terminal.grid.cols());
    let _ = write!(
        line,
        " {} | pane {}/{} | {cwd} | {state} | blocks {} | F1",
        tabs.join(" "),
        pane + 1,
        pane_count,
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
