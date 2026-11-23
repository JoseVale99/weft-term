//! Engine tests: the terminal must behave like a terminal, not just parse bytes.
//!
//! These run without a window and without a shell, which is what makes the
//! renderer replaceable and the behavior reviewable.

use weft_term::grid::{char_width, Color, Wide};
use weft_term::vt::Terminal;

fn term(cols: usize, rows: usize) -> Terminal {
    Terminal::new(cols, rows)
}

fn text(terminal: &Terminal, y: usize) -> String {
    let mut line = String::new();
    for x in 0..terminal.grid.cols() {
        let cell = terminal.grid.cell(x, y);
        if cell.wide != Wide::Trailing {
            line.push(cell.ch);
        }
    }
    line.trim_end().to_owned()
}

#[test]
fn printing_wraps_at_the_margin() {
    let mut terminal = term(5, 3);
    terminal.feed(b"abcdefg");
    assert_eq!(text(&terminal, 0), "abcde");
    assert_eq!(text(&terminal, 1), "fg");
}

#[test]
fn scrolling_moves_lines_to_scrollback() {
    let mut terminal = term(5, 2);
    terminal.feed(b"a\r\nb\r\nc");
    assert_eq!(terminal.grid.scrollback_len(), 1);
    assert_eq!(text(&terminal, 0), "b");
    assert_eq!(text(&terminal, 1), "c");
}

#[test]
fn carriage_return_and_backspace() {
    let mut terminal = term(10, 1);
    // Backspace moves the cursor; it does not erase.
    terminal.feed(b"abc\rX\x08Y");
    assert_eq!(text(&terminal, 0), "Ybc");
}

#[test]
fn erase_line_and_display() {
    let mut terminal = term(6, 2);
    terminal.feed(b"abcdef\x1b[3G\x1b[K");
    assert_eq!(text(&terminal, 0), "ab");
    terminal.feed(b"zz\x1b[2J");
    assert_eq!(text(&terminal, 0), "");
    assert_eq!(text(&terminal, 1), "");
}

#[test]
fn cursor_position_and_erase_below() {
    let mut terminal = term(10, 3);
    terminal.feed(b"one\r\ntwo\r\nthree");
    terminal.feed(b"\x1b[2;2H\x1b[J");
    assert_eq!(text(&terminal, 0), "one");
    assert_eq!(text(&terminal, 1), "t");
    assert_eq!(text(&terminal, 2), "");
}

#[test]
fn sgr_sets_colors_and_attributes() {
    let mut terminal = term(10, 1);
    terminal.feed(b"\x1b[1;31;44mX");
    let cell = terminal.grid.cell(0, 0);
    assert_eq!(cell.ch, 'X');
    assert_eq!(cell.fg, Color::Indexed(1));
    assert_eq!(cell.bg, Color::Indexed(4));
    assert!(cell.attrs.bold);

    terminal.feed(b"\x1b[0m\x1b[38;2;10;20;30mY");
    let cell = terminal.grid.cell(1, 0);
    assert_eq!(cell.fg, Color::Rgb(10, 20, 30));
    assert!(!cell.attrs.bold);
}

#[test]
fn wide_characters_take_two_cells() {
    assert_eq!(char_width('漢'), 2);
    assert_eq!(char_width('a'), 1);
    assert_eq!(char_width('\u{0301}'), 0);

    let mut terminal = term(6, 1);
    terminal.feed("漢a".as_bytes());
    assert_eq!(terminal.grid.cell(0, 0).wide, Wide::Leading);
    assert_eq!(terminal.grid.cell(1, 0).wide, Wide::Trailing);
    assert_eq!(terminal.grid.cell(2, 0).ch, 'a');
}

#[test]
fn insert_and_delete_characters() {
    let mut terminal = term(6, 1);
    terminal.feed(b"abcd\x1b[1;2H\x1b[2@");
    assert_eq!(text(&terminal, 0), "a  bcd");
    terminal.feed(b"\x1b[2P");
    assert_eq!(text(&terminal, 0), "abcd");
}

#[test]
fn scroll_region_keeps_the_header() {
    let mut terminal = term(4, 4);
    terminal.feed(b"head\r\n1\r\n2\r\n3\x1b[2;4r\x1b[4;1H\n");
    assert_eq!(text(&terminal, 0), "head");
    assert_eq!(text(&terminal, 1), "2");
    assert_eq!(text(&terminal, 2), "3");
}

#[test]
fn alternate_screen_restores_the_main_buffer() {
    let mut terminal = term(10, 2);
    terminal.feed(b"main\x1b[?1049h");
    assert_eq!(text(&terminal, 0), "");
    terminal.feed(b"alt");
    assert_eq!(text(&terminal, 0), "alt");
    terminal.feed(b"\x1b[?1049l");
    assert_eq!(text(&terminal, 0), "main");
    assert!(!terminal.grid.alt_active());
}

#[test]
fn device_reports_are_answered() {
    let mut terminal = term(77, 24);
    let response = terminal.feed(b"\x1b[c");
    assert_eq!(response, b"\x1b[?6c");
    terminal.feed(b"\x1b[3;5H");
    let response = terminal.feed(b"\x1b[6n");
    assert_eq!(response, b"\x1b[3;5R");
    let response = terminal.feed(b"\x1b[5n");
    assert_eq!(response, b"\x1b[0n");
}

#[test]
fn title_is_tracked() {
    let mut terminal = term(10, 1);
    terminal.feed(b"\x1b]0;my project\x07");
    assert_eq!(terminal.grid.title, "my project");
}

#[test]
fn shell_markers_create_blocks() {
    let mut terminal = term(20, 4);
    terminal.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07\x1b]133;C\x07out\r\n\x1b]133;D;0\x07");
    let events = terminal.take_events();
    assert_eq!(events.len(), 4);
    let block = terminal.blocks.current().expect("block");
    assert_eq!(block.exit_code, Some(0));
}

#[test]
fn resize_keeps_visible_content() {
    let mut terminal = term(6, 3);
    terminal.feed(b"hello\r\nworld");
    terminal.resize(10, 4);
    assert_eq!(terminal.grid.cols(), 10);
    assert_eq!(terminal.grid.rows(), 4);
    assert_eq!(text(&terminal, 0), "hello");
    assert_eq!(text(&terminal, 1), "world");
}

#[test]
fn malformed_sequences_do_not_crash_or_print() {
    let mut terminal = term(10, 2);
    terminal.feed(b"\x1b[999999999999999999D");
    terminal.feed(b"\x1b[");
    terminal.feed(b"\x1b]133;A\x07");
    terminal.feed(b"\xff\xfe");
    terminal.feed(b"ok");
    assert_eq!(text(&terminal, 0), "ok");
}

#[cfg(unix)]
#[test]
fn pty_session_runs_a_real_process() {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use weft_term::pty::Session;

    let mut session = Session::spawn("sh", &["-c", "printf 'pty-ok\\n'"], 80, 24)
        .expect("spawn pty session");
    let mut master = session.try_clone_master().expect("clone master");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        use std::io::Read;
        let mut buffer = [0u8; 1024];
        let mut collected = Vec::new();
        loop {
            match master.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => collected.extend_from_slice(&buffer[..count]),
            }
        }
        let _ = sender.send(collected);
    });

    let mut output = Vec::new();
    while let Ok(chunk) = receiver.recv_timeout(Duration::from_secs(5)) {
        output = chunk;
        break;
    }
    session.kill();
    let text = String::from_utf8_lossy(&output);
    assert!(text.contains("pty-ok"), "output was {text:?}");
}

#[cfg(unix)]
#[test]
fn pty_resize_does_not_fail() {
    use weft_term::pty::Session;
    let session = Session::spawn("sh", &["-c", "sleep 0.1"], 80, 24).expect("spawn");
    session.resize(100, 40).expect("resize");
}
