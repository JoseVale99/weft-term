//! Block shell: the product idea without a full-screen grid.
//!
//! It runs on every platform and needs no PTY. When standard input is a
//! terminal it uses the real editor (movement, history, completion); otherwise
//! it degrades to reading whole lines, which is what tests and pipes use.

use std::env;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Instant;

use weft_store::{SessionEntry, Store};
use weft_term::History;
use weft_term::grid::char_width;
use weft_term::history::Search;
use weft_term::input::Editor;

use crate::keys::{Decoder, Key};
use crate::platform;

const BUILTINS: [&str; 5] = ["cd", "exit", "help", "history", "quit"];

/// Runs the block shell.
pub fn run() -> io::Result<()> {
    let history_limit = 5_000;
    let store = Store::open_default().ok();
    if platform::is_interactive()
        && let Ok(_raw) = platform::enter_raw()
    {
        return interactive(history_limit, store.as_ref());
    }
    line_mode(history_limit, store.as_ref())
}

/// Restores history from disk so a new session starts where the last one ended.
fn load_history(limit: usize, store: Option<&Store>) -> History {
    let mut history = History::new(limit);
    if let Some(store) = store {
        if let Ok(entries) = store.load_history(limit) {
            for entry in entries {
                history.record(&entry);
            }
        }
        let _ = store.compact_history(limit);
    }
    history
}

fn interactive(history_limit: usize, store: Option<&Store>) -> io::Result<()> {
    println!(
        "weft {} - block shell. Type `exit` to leave, `help` for commands.",
        env!("CARGO_PKG_VERSION")
    );
    let mut editor = Editor::new();
    let mut history = load_history(history_limit, store);
    let mut decoder = Decoder::new();
    let mut search: Option<Search> = None;
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let mut buffer = [0u8; 1024];

    loop {
        draw(&editor, search.as_ref())?;
        let submitted = loop {
            let count = stdin.read(&mut buffer)?;
            if count == 0 {
                println!();
                return Ok(());
            }
            decoder.push(&buffer[..count]);
            let mut submitted = None;
            while let Some(key) = decoder.next() {
                if search.is_some() {
                    let mut finish = None;
                    if let Some(active) = search.as_mut() {
                        match key {
                            Key::Char(c) => active.push(c, &history, &mut editor),
                            Key::Backspace => active.backspace(&history, &mut editor),
                            Key::CtrlR => active.next(&mut editor),
                            Key::Enter => finish = Some(true),
                            Key::Esc | Key::CtrlC => finish = Some(false),
                            _ => {}
                        }
                    }
                    match finish {
                        Some(true) => {
                            search = None;
                            history.reset();
                        }
                        Some(false) => {
                            if let Some(active) = search.take() {
                                active.cancel(&mut editor);
                            }
                            history.reset();
                        }
                        None => {}
                    }
                    continue;
                }
                match key {
                    Key::Enter => {
                        submitted = Some(editor.take());
                        history.reset();
                        break;
                    }
                    Key::AltEnter => editor.insert_newline(),
                    Key::CtrlD if editor.is_empty() => {
                        println!();
                        return Ok(());
                    }
                    Key::CtrlC => {
                        editor.take();
                        history.reset();
                        println!("^C");
                        submitted = Some(String::new());
                        break;
                    }
                    Key::CtrlL => {
                        let mut stdout = io::stdout();
                        stdout.write_all(b"\x1b[2J\x1b[H")?;
                        stdout.flush()?;
                    }
                    Key::CtrlU => editor.kill_to_start(),
                    Key::CtrlK => editor.kill_to_end(),
                    Key::CtrlW => editor.kill_word_back(),
                    Key::CtrlY => editor.yank(),
                    Key::CtrlR => {
                        let mut active = Search::start(&editor);
                        active.refresh(&history, &mut editor);
                        search = Some(active);
                    }
                    Key::CtrlP => {
                        if editor.rows() == 1 {
                            if let Some(entry) = history.previous(editor.text()) {
                                editor.set_text(entry);
                            }
                        } else {
                            editor.move_up();
                        }
                    }
                    Key::CtrlN => {
                        if editor.rows() == 1 {
                            if let Some(entry) = history.next_entry() {
                                editor.set_text(entry);
                            }
                        } else {
                            editor.move_down();
                        }
                    }
                    Key::Up => {
                        if editor.rows() == 1
                            && let Some(entry) = history.previous(editor.text())
                        {
                            editor.set_text(entry);
                        } else {
                            editor.move_up();
                        }
                    }
                    Key::Down => {
                        if editor.rows() == 1
                            && let Some(entry) = history.next_entry()
                        {
                            editor.set_text(entry);
                        } else {
                            editor.move_down();
                        }
                    }
                    Key::Left => editor.move_left(),
                    Key::Right => editor.move_right(),
                    Key::WordLeft => editor.move_word_left(),
                    Key::WordRight => editor.move_word_right(),
                    Key::Home => editor.move_home(),
                    Key::End => editor.move_end(),
                    Key::Backspace => editor.backspace(),
                    Key::Delete => editor.delete_forward(),
                    Key::Tab => {
                        let candidates = completions(&history);
                        let _ = editor.complete(&candidates);
                    }
                    Key::Char(c) => editor.insert(&c.to_string()),
                    Key::Esc | Key::CtrlD => {}
                }
            }
            if let Some(line) = submitted {
                break line;
            }
            draw(&editor, search.as_ref())?;
        };

        let Some(action) = handle(&submitted, &mut history)? else {
            continue;
        };
        match action {
            Action::Exit => return Ok(()),
            Action::Run(line) => run_line(&line, &mut history, store)?,
        }
    }
}

fn line_mode(history_limit: usize, store: Option<&Store>) -> io::Result<()> {
    println!(
        "weft {} - block shell. Type `exit` to leave, `help` for commands.",
        env!("CARGO_PKG_VERSION")
    );
    let mut history = load_history(history_limit, store);
    loop {
        print!("{}\n> ", cwd_prompt());
        io::stdout().flush()?;
        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line)? == 0 {
            println!();
            return Ok(());
        }
        let line = line.trim_end_matches(['\n', '\r']).to_owned();
        let Some(action) = handle(&line, &mut history)? else {
            continue;
        };
        match action {
            Action::Exit => return Ok(()),
            Action::Run(line) => run_line(&line, &mut history, store)?,
        }
    }
}

enum Action {
    Exit,
    Run(String),
}

fn handle(line: &str, history: &mut History) -> io::Result<Option<Action>> {
    let line = line.trim_end();
    if line.trim().is_empty() {
        return Ok(None);
    }
    match line {
        "exit" | "quit" => return Ok(Some(Action::Exit)),
        "help" => {
            print_help();
            return Ok(None);
        }
        "history" => {
            for (index, entry) in history.entries().enumerate() {
                println!("{index:4}  {entry}");
            }
            return Ok(None);
        }
        "!!" => {
            let Some(previous) = history.entries().next_back().map(str::to_owned) else {
                println!("weft: no previous command");
                return Ok(None);
            };
            println!("\x1b[2m>{previous}\x1b[0m");
            return Ok(Some(Action::Run(previous)));
        }
        "cd" => {
            let _ = env::set_current_dir(platform::home());
            return Ok(None);
        }
        _ => {}
    }
    if let Some(target) = line.strip_prefix("cd ") {
        let path = expand_home(target.trim());
        if let Err(error) = env::set_current_dir(&path) {
            println!("weft: cd: {error}");
        }
        return Ok(None);
    }
    Ok(Some(Action::Run(line.to_owned())))
}

/// A finished block.
#[derive(Debug, Clone, Copy)]
pub struct Block {
    /// Exit code reported by the shell.
    pub code: i32,
    /// Wall-clock duration in milliseconds.
    pub elapsed_ms: u64,
}

fn run_line(line: &str, history: &mut History, store: Option<&Store>) -> io::Result<()> {
    history.record(line);
    if let Some(store) = store {
        let _ = store.append_history(line);
    }
    let cwd = env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let block = execute(line)?;
    if let Some(store) = store {
        let _ = store.append_session(&SessionEntry::now(cwd, line, block.code, block.elapsed_ms));
    }
    Ok(())
}

/// Runs a single command line and prints its block footer.
///
/// Standard input is inherited on purpose: interactive commands (`cat`, `ssh`,
/// pagers) need it while they run. The flip side is that keys typed during a
/// command belong to the command, not to the next prompt. The full-screen TUI
/// does not have this limitation because every pane owns its own PTY.
pub fn execute(line: &str) -> io::Result<Block> {
    let started = Instant::now();
    let status = platform::command(line).status()?;
    let elapsed = started.elapsed();
    let code = status.code().unwrap_or(-1);
    let marker = if code == 0 { "ok" } else { "failed" };
    println!(
        "\x1b[2m[{marker} | exit {code} | {} ms | {}]\x1b[0m",
        elapsed.as_millis(),
        line
    );
    Ok(Block {
        code,
        elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
    })
}

/// Runs a command line to completion and returns its exit code.
pub fn run_once(line: &str) -> io::Result<i32> {
    let status = platform::command(line).status()?;
    Ok(status.code().unwrap_or(-1))
}

fn cwd_prompt() -> String {
    let cwd = env::current_dir().unwrap_or_default();
    let home = platform::home();
    let shown = cwd.display().to_string();
    let text = shown.strip_prefix(&home).map_or(shown.clone(), |rest| {
        if rest.is_empty() {
            "~".to_owned()
        } else {
            format!("~{rest}")
        }
    });
    let columns = platform::columns().max(20);
    truncate_left(&text, columns.saturating_sub(2))
}

fn truncate_left(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    let mut tail: String = text
        .chars()
        .rev()
        .take(width.saturating_sub(2))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    tail.insert_str(0, "..");
    tail
}

fn draw(editor: &Editor, search: Option<&Search>) -> io::Result<()> {
    let columns = platform::columns().max(20);
    let (prompt, prompt_width) = if let Some(search) = search {
        let mut query: String = search.query().to_owned();
        let limit = columns.saturating_sub(24).max(4);
        if query.chars().count() > limit {
            query = query
                .chars()
                .rev()
                .take(limit)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
        }
        let last = format!("(reverse-i-search)`{query}': ");
        let width = last.chars().map(char_width).sum();
        (format!("{}\n{last}", cwd_prompt()), width)
    } else {
        (format!("{}\n> ", cwd_prompt()), 2)
    };
    let layout = layout(editor, columns, prompt_width);

    let mut frame = String::with_capacity(editor.text().len() + 64);
    frame.push_str("\r\x1b[J");
    frame.push_str(&prompt);
    frame.push_str(editor.text());

    let up = layout.rows.saturating_sub(1 + layout.cursor_row);
    if up > 0 {
        frame.push_str(&format!("\x1b[{up}A"));
    }
    frame.push('\r');
    if layout.cursor_column > 0 {
        frame.push_str(&format!("\x1b[{}C", layout.cursor_column));
    }

    let mut stdout = io::stdout();
    stdout.write_all(frame.as_bytes())?;
    stdout.flush()
}

struct Layout {
    cursor_row: usize,
    cursor_column: usize,
    rows: usize,
}

/// Computes where the cursor lands once the line has wrapped.
fn layout(editor: &Editor, columns: usize, prompt_width: usize) -> Layout {
    let text = editor.text();
    let cursor = editor.cursor();
    let mut row = 0;
    let mut column = prompt_width;
    let mut cursor_row = 0;
    let mut cursor_column = prompt_width;
    let mut seen_cursor = cursor == 0;

    for (offset, c) in text.char_indices() {
        if offset == cursor {
            cursor_row = row;
            cursor_column = column;
            seen_cursor = true;
        }
        if c == '\n' {
            row += 1;
            column = 0;
            continue;
        }
        let width = char_width(c);
        if column + width > columns {
            row += 1;
            column = 0;
        }
        column += width;
    }
    if !seen_cursor {
        cursor_row = row;
        cursor_column = column;
    }
    Layout {
        cursor_row,
        cursor_column,
        rows: row + 1,
    }
}

fn completions(history: &History) -> Vec<String> {
    let mut candidates: Vec<String> = BUILTINS
        .iter()
        .map(|builtin| (*builtin).to_owned())
        .collect();
    candidates.extend(history.entries().map(str::to_owned));
    candidates.extend(executables().iter().cloned());
    if let Ok(entries) = fs::read_dir(".") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() {
                candidates.push(format!("{name}/"));
            } else {
                candidates.push(name);
            }
        }
    }
    candidates.sort_unstable();
    candidates.dedup();
    candidates
}

fn executables() -> &'static Vec<String> {
    static EXECUTABLES: OnceLock<Vec<String>> = OnceLock::new();
    EXECUTABLES.get_or_init(|| {
        let mut names = Vec::new();
        let Some(path) = env::var_os("PATH") else {
            return names;
        };
        for directory in env::split_paths(&path) {
            let Ok(entries) = fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if is_executable(&path)
                    && let Some(name) = path.file_name().and_then(|name| name.to_str())
                {
                    names.push(name.to_owned());
                }
            }
        }
        names.sort_unstable();
        names.dedup();
        names
    })
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn expand_home(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        format!("{}/{}", platform::home(), rest)
    } else if path == "~" {
        platform::home()
    } else {
        path.to_owned()
    }
}

fn print_help() {
    println!(
        "\
  <command>    run it as a shell command
  cd <dir>     change directory (also `cd` for home)
  history      list this session's commands
  !!           repeat the previous command
  exit         leave the shell

Editing: arrows, Alt-B/F by word, Ctrl-U/K/W kill, Ctrl-Y yank, Tab
completion, Up/Down history, Ctrl-R fuzzy search, Alt-Enter newline, Ctrl-L
clear."
    );
}
