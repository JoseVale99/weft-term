//! End-to-end test: the real binary, on a real pseudo-terminal, running a real
//! shell. This is what proves the window-less front-end is a working terminal.

#![cfg(unix)]

use std::io::Read;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use weft_term::pty::Session;

fn collect(receiver: &mpsc::Receiver<Vec<u8>>, all: &mut String, limit: Duration) {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(chunk) => all.push_str(&String::from_utf8_lossy(&chunk)),
            Err(_) => break,
        }
    }
}

#[test]
fn interactive_terminal_runs_a_command_and_exits() {
    let mut session =
        Session::spawn(env!("CARGO_BIN_EXE_weft"), &[], 80, 24).expect("start weft");

    let mut master = session.try_clone_master().expect("clone master");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match master.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(count) => {
                    if sender.send(buffer[..count].to_vec()).is_err() {
                        return;
                    }
                }
            }
        }
    });

    let mut output = String::new();
    collect(&receiver, &mut output, Duration::from_secs(2));

    // The typed line echoes back through the PTY, so the marker is assembled
    // by the shell: only a successful execution can produce `42-e2e`.
    session
        .write(b"printf '\\x34\\x32-e2e\\n'\r")
        .expect("write command");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = false;
    while Instant::now() < deadline && !seen {
        collect(&receiver, &mut output, Duration::from_millis(200));
        seen = output.contains("42-e2e");
    }
    assert!(seen, "command output not found; terminal said:\n{output}");

    session.write(b"exit\r").expect("write exit");
    let mut exited = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if matches!(session.try_wait(), Ok(Some(_))) {
            exited = true;
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(exited, "terminal did not close after `exit`");
}

#[test]
fn the_palette_opens_a_second_tab() {
    let mut session =
        Session::spawn(env!("CARGO_BIN_EXE_weft"), &[], 80, 24).expect("start weft");

    let mut master = session.try_clone_master().expect("clone master");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match master.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(count) => {
                    if sender.send(buffer[..count].to_vec()).is_err() {
                        return;
                    }
                }
            }
        }
    });

    let mut output = String::new();
    collect(&receiver, &mut output, Duration::from_secs(2));

    // F1 opens the palette; the first entry is "New tab".
    session.write(b"\x1bOP").expect("open palette");
    thread::sleep(Duration::from_millis(300));
    session.write(b"\r").expect("choose new tab");
    thread::sleep(Duration::from_millis(500));

    session
        .write(b"printf '\\x34\\x33-tab\\n'\r")
        .expect("write command");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = false;
    while Instant::now() < deadline && !seen {
        collect(&receiver, &mut output, Duration::from_millis(200));
        seen = output.contains("43-tab");
    }
    assert!(seen, "command did not run in the new tab; terminal said:\n{output}");

    thread::sleep(Duration::from_millis(500));

    // Close the new tab, then the original one.
    session.write(b"exit\r").expect("exit new tab");
    thread::sleep(Duration::from_millis(500));
    session.write(b"exit\r").expect("exit first tab");

    let mut exited = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if matches!(session.try_wait(), Ok(Some(_))) {
            exited = true;
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(exited, "both tabs did not close; terminal said:\n{output}");
}

#[test]
fn a_pane_splits_and_runs_a_command() {
    let mut session =
        Session::spawn(env!("CARGO_BIN_EXE_weft"), &[], 80, 24).expect("start weft");

    let mut master = session.try_clone_master().expect("clone master");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match master.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(count) => {
                    if sender.send(buffer[..count].to_vec()).is_err() {
                        return;
                    }
                }
            }
        }
    });

    let mut output = String::new();
    collect(&receiver, &mut output, Duration::from_secs(2));

    // F1, then move from "New tab" down to "Split pane" and choose it.
    session.write(b"\x1bOP").expect("open palette");
    thread::sleep(Duration::from_millis(300));
    session
        .write(b"\x1b[B\x1b[B\x1b[B\r")
        .expect("choose split pane");
    thread::sleep(Duration::from_millis(600));

    session
        .write(b"printf '\\x34\\x34-split\\n'\r")
        .expect("write command");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = false;
    while Instant::now() < deadline && !seen {
        collect(&receiver, &mut output, Duration::from_millis(200));
        seen = output.contains("44-split");
    }
    assert!(seen, "command did not run in the split pane; terminal said:\n{output}");

    // Close the split pane, then the remaining one.
    thread::sleep(Duration::from_millis(500));
    session.write(b"exit\r").expect("exit split pane");
    thread::sleep(Duration::from_millis(600));
    session.write(b"exit\r").expect("exit remaining pane");

    let mut exited = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if matches!(session.try_wait(), Ok(Some(_))) {
            exited = true;
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(exited, "split session did not exit; terminal said:\n{output}");
}

#[test]
fn a_pane_splits_to_the_right_and_runs_a_command() {
    let mut session =
        Session::spawn(env!("CARGO_BIN_EXE_weft"), &[], 80, 24).expect("start weft");

    let mut master = session.try_clone_master().expect("clone master");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match master.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(count) => {
                    if sender.send(buffer[..count].to_vec()).is_err() {
                        return;
                    }
                }
            }
        }
    });

    let mut output = String::new();
    collect(&receiver, &mut output, Duration::from_secs(2));

    // F1, then move from "New tab" down to "Split right" and choose it.
    session.write(b"\x1bOP").expect("open palette");
    thread::sleep(Duration::from_millis(300));
    session
        .write(b"\x1b[B\x1b[B\x1b[B\x1b[B\r")
        .expect("choose split right");
    thread::sleep(Duration::from_millis(600));

    session
        .write(b"printf '\\x35\\x35-right\\n'\r")
        .expect("write command");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = false;
    while Instant::now() < deadline && !seen {
        collect(&receiver, &mut output, Duration::from_millis(200));
        seen = output.contains("55-right");
    }
    assert!(seen, "command did not run in the right pane; terminal said:\n{output}");

    thread::sleep(Duration::from_millis(500));
    session.write(b"exit\r").expect("exit right pane");
    thread::sleep(Duration::from_millis(600));
    session.write(b"exit\r").expect("exit left pane");

    let mut exited = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if matches!(session.try_wait(), Ok(Some(_))) {
            exited = true;
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(exited, "split session did not exit; terminal said:\n{output}");
}

#[test]
fn panes_mix_vertical_and_horizontal_splits() {
    let mut session =
        Session::spawn(env!("CARGO_BIN_EXE_weft"), &[], 80, 24).expect("start weft");

    let mut master = session.try_clone_master().expect("clone master");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match master.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(count) => {
                    if sender.send(buffer[..count].to_vec()).is_err() {
                        return;
                    }
                }
            }
        }
    });

    let mut output = String::new();
    collect(&receiver, &mut output, Duration::from_secs(2));

    // Split down, then split the new bottom pane to the right.
    session.write(b"\x1bOP").expect("open palette");
    thread::sleep(Duration::from_millis(300));
    session
        .write(b"\x1b[B\x1b[B\x1b[B\r")
        .expect("choose split down");
    thread::sleep(Duration::from_millis(500));
    session.write(b"\x1bOP").expect("open palette again");
    thread::sleep(Duration::from_millis(300));
    session
        .write(b"\x1b[B\x1b[B\x1b[B\x1b[B\r")
        .expect("choose split right");
    thread::sleep(Duration::from_millis(600));

    session
        .write(b"printf '\\x36\\x36-mixed\\n'\r")
        .expect("write command");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = false;
    while Instant::now() < deadline && !seen {
        collect(&receiver, &mut output, Duration::from_millis(200));
        seen = output.contains("66-mixed");
    }
    assert!(seen, "command did not run in the mixed pane; terminal said:\n{output}");

    thread::sleep(Duration::from_millis(500));
    session.write(b"exit\r").expect("exit mixed pane");
    thread::sleep(Duration::from_millis(600));
    session.write(b"exit\r").expect("exit bottom pane");
    thread::sleep(Duration::from_millis(600));
    session.write(b"exit\r").expect("exit top pane");

    let mut exited = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if matches!(session.try_wait(), Ok(Some(_))) {
            exited = true;
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(exited, "mixed session did not exit; terminal said:\n{output}");
}
