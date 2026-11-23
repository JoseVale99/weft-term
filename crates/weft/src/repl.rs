//! Block shell used on Windows and whenever stdin is not a terminal.
//!
//! It is the same product idea without a full-screen grid: each command is a
//! block with its working directory, exit code and duration. It requires no
//! PTY, so it runs everywhere Rust runs.

use std::env;
use std::io::{self, BufRead, Write};
use std::process::Command;
use std::time::Instant;

/// Runs the interactive block shell.
pub fn run() -> io::Result<()> {
    println!(
        "weft {} - block shell. Type `exit` to leave, `help` for commands.",
        env!("CARGO_PKG_VERSION")
    );
    let mut history: Vec<String> = Vec::new();
    loop {
        let cwd = env::current_dir().unwrap_or_default();
        print!("\x1b[1m{}\x1b[0m\n> ", cwd.display());
        io::stdout().flush()?;

        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line)? == 0 {
            println!();
            break;
        }
        let line = line.trim_end_matches(['\n', '\r']).trim().to_owned();
        if line.is_empty() {
            continue;
        }
        match line.as_str() {
            "exit" | "quit" => break,
            "help" => {
                print_help();
                continue;
            }
            "history" => {
                for (index, entry) in history.iter().enumerate() {
                    println!("{index:4}  {entry}");
                }
                continue;
            }
            "!!" => {
                let Some(previous) = history.last() else {
                    println!("weft: no previous command");
                    continue;
                };
                println!("\x1b[2m>{previous}\x1b[0m");
                execute(previous)?;
                continue;
            }
            _ => {}
        }
        // `cd` must change this process, not a child shell.
        if line == "cd" {
            let home = home_dir();
            let _ = env::set_current_dir(home);
            continue;
        }
        if let Some(target) = line.strip_prefix("cd ") {
            let path = expand_home(target.trim());
            if let Err(error) = env::set_current_dir(&path) {
                println!("weft: cd: {error}");
            }
            continue;
        }
        history.push(line.clone());
        execute(&line)?;
    }
    Ok(())
}

/// Runs a single command line and prints its block footer.
pub fn execute(line: &str) -> io::Result<()> {
    let started = Instant::now();
    let status = shell(line).status()?;
    let elapsed = started.elapsed();
    let code = status.code().unwrap_or(-1);
    let marker = if code == 0 { "ok" } else { "failed" };
    println!(
        "\x1b[2m[{marker} | exit {code} | {} ms | {}]\x1b[0m",
        elapsed.as_millis(),
        line
    );
    Ok(())
}

/// Runs a command line to completion and returns its exit code.
pub fn run_once(line: &str) -> io::Result<i32> {
    let status = shell(line).status()?;
    Ok(status.code().unwrap_or(-1))
}

fn shell(line: &str) -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(line);
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new("sh");
        command.arg("-c").arg(line);
        command
    }
}

fn home_dir() -> String {
    env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_owned())
}

fn expand_home(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        format!("{}/{}", home_dir(), rest)
    } else if path == "~" {
        home_dir()
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
  exit         leave the shell"
    );
}
