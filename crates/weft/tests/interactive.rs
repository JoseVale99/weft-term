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
