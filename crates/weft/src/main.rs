//! Weft: a native terminal with agents, written in Rust.
//!
//! Commands:
//!
//! - `weft`             interactive terminal (full screen on Unix, block shell
//!                      when stdin is not a terminal)
//! - `weft run <cmd>`   run one command and report its block
//! - `weft shell`       block shell without a pseudo-terminal
//! - `weft integration` print the shell integration script for bash/zsh/fish
//! - `weft help`        command list

mod keys;
mod platform;
mod render;
mod repl;

use std::env;
use std::io;
use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None => interactive(),
        Some("run") => run_command(&args[1..]),
        Some("shell") => repl::run(),
        Some("integration") => integration(&args[1..]),
        Some("help" | "--help" | "-h") => {
            print_help();
            Ok(())
        }
        Some("--version" | "-V") => {
            println!("weft {VERSION}");
            Ok(())
        }
        Some(other) => {
            eprintln!("weft: unknown command `{other}`");
            eprintln!("Run `weft help` for the command list.");
            Ok(())
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("weft: {error}");
            ExitCode::FAILURE
        }
    }
}

fn interactive() -> io::Result<()> {
    platform::interactive()
}

fn run_command(args: &[String]) -> io::Result<()> {
    if args.is_empty() {
        eprintln!("usage: weft run <command>");
        return Ok(());
    }
    let line = args.join(" ");
    let code = repl::run_once(&line)?;
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

fn integration(args: &[String]) -> io::Result<()> {
    let shell = args.first().map(String::as_str).unwrap_or_default();
    let script = match shell {
        "bash" => Some(include_str!("../../../shell/weft.bash")),
        "zsh" => Some(include_str!("../../../shell/weft.zsh")),
        "fish" => Some(include_str!("../../../shell/weft.fish")),
        "auto" | "" => {
            println!("weft: choose a shell: bash, zsh or fish");
            println!("  weft integration bash >> ~/.bashrc");
            println!("  weft integration zsh  >> ~/.zshrc");
            println!("  weft integration fish >> ~/.config/fish/config.fish");
            return Ok(());
        }
        other => {
            eprintln!("weft: unsupported shell `{other}` (bash, zsh, fish)");
            return Ok(());
        }
    };
    print!("{}", script.unwrap_or_default());
    Ok(())
}

fn print_help() {
    println!(
        "\
weft {VERSION}
A native terminal with agents, written in Rust.

Usage:
  weft                  interactive terminal
  weft run <command>    run one command and report its block
  weft shell            block shell without a pseudo-terminal
  weft integration <sh> print integration for bash, zsh or fish
  weft help             this message
  weft --version        version

Environment:
  SHELL                 shell started by the interactive terminal
  RUST_LOG              log filter for internal diagnostics"
    );
}
