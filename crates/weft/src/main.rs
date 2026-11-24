//! Weft: a native terminal with agents, written in Rust.
//!
//! Commands:
//!
//! - `weft`             interactive terminal (full screen on Unix, block shell
//!   when stdin is not a terminal)
//! - `weft run <cmd>`   run one command and report its block
//! - `weft shell`       block shell without a pseudo-terminal
//! - `weft integration` print the shell integration script for bash/zsh/fish
//! - `weft help`        command list

mod agent;
mod keys;
mod platform;
mod render;
mod repl;
mod theme;

use std::env;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use weft_config::Config;
use weft_store::Store;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None => interactive(),
        Some("run") => run_command(&args[1..]),
        Some("shell") => repl::run(),
        Some("integration") => integration(&args[1..]),
        Some("history") => history(&args[1..]),
        Some("sessions") => sessions(&args[1..]),
        Some("workflow" | "workflows") => workflow(&args[1..]),
        Some("agent") => agent::run(&args[1..]),
        Some("theme" | "themes") => themes(),
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

fn config_path() -> PathBuf {
    if let Some(path) = env::var_os("WEFT_CONFIG") {
        return PathBuf::from(path);
    }
    Store::open_default().map_or_else(
        |_| PathBuf::from("weft.toml"),
        |store| store.root().join("config.toml"),
    )
}

fn workflow(args: &[String]) -> io::Result<()> {
    let config = Config::load(config_path()).map_err(io::Error::other)?;
    match args.first().map(String::as_str) {
        None | Some("list") => {
            if config.workflows.is_empty() {
                println!("weft: no workflows in {}", config_path().display());
            }
            for entry in &config.workflows {
                let params = entry.placeholders();
                let suffix = if params.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", params.join(", "))
                };
                println!("{}{suffix}\t{}", entry.name, entry.description);
            }
        }
        Some("run") => {
            let Some(name) = args.get(1) else {
                eprintln!("usage: weft workflow run <name> [key=value ...]");
                return Ok(());
            };
            let Some(entry) = config.workflows.iter().find(|entry| entry.name == *name) else {
                eprintln!("weft: unknown workflow `{name}`");
                return Ok(());
            };
            let mut owned = Vec::new();
            for argument in &args[2..] {
                let Some((key, value)) = argument.split_once('=') else {
                    eprintln!("weft: expected key=value, got `{argument}`");
                    return Ok(());
                };
                owned.push((key.to_owned(), value.to_owned()));
            }
            let values: Vec<(&str, &str)> = owned
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect();
            let command = entry.expand(&values).map_err(io::Error::other)?;
            let block = repl::execute(&command)?;
            if block.code != 0 {
                std::process::exit(block.code);
            }
        }
        Some(other) => eprintln!("weft: unknown workflow action `{other}` (list, run)"),
    }
    Ok(())
}

fn themes() -> io::Result<()> {
    let config = Config::load(config_path()).map_err(io::Error::other)?;
    let current = crate::theme::resolve(&config.terminal.theme).name;
    for name in crate::theme::names() {
        let theme = crate::theme::resolve(name);
        let marker = if theme.name == current { "*" } else { " " };
        let (fr, fg, fb) = theme.foreground;
        let (br, bg, bb) = theme.background;
        println!(
            "{marker} {:<12} fg #{fr:02x}{fg:02x}{fb:02x} bg #{br:02x}{bg:02x}{bb:02x}",
            theme.name
        );
    }
    Ok(())
}

fn history(args: &[String]) -> io::Result<()> {
    let store = Store::open_default()?;
    match args.first().map(String::as_str) {
        Some("clear") => {
            store.clear_history()?;
            println!("weft: history cleared");
        }
        None | Some("list") => {
            for (index, entry) in store.load_history(5_000)?.iter().enumerate() {
                println!("{index:4}  {entry}");
            }
        }
        Some(other) => eprintln!("weft: unknown history action `{other}` (list, clear)"),
    }
    Ok(())
}

fn sessions(args: &[String]) -> io::Result<()> {
    let store = Store::open_default()?;
    match args.first().map(String::as_str) {
        Some("clear") => {
            store.clear_sessions()?;
            println!("weft: session log cleared");
        }
        None | Some("list") => {
            for entry in store.sessions()? {
                let command = entry.command.replace('\n', "\\n");
                println!(
                    "{}\t{:+}\t{} ms\t{}\t{}",
                    entry.at, entry.exit, entry.duration_ms, entry.cwd, command
                );
            }
        }
        Some(other) => eprintln!("weft: unknown sessions action `{other}` (list, clear)"),
    }
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
  weft history [clear]  list or clear the persistent command history
  weft sessions [clear] list or clear the session log
  weft workflow list   list saved workflows
  weft workflow run <name> [key=value ...]  run a saved workflow
  weft agent check     validate the assistant configuration
  weft agent chat <p>  send one prompt over plain HTTP (streaming)
  weft theme           list built-in themes and the active one
  weft help             this message
  weft --version        version

Environment:
  SHELL                 shell started by the interactive terminal
  WEFT_DATA_DIR         override the directory for history and sessions
  WEFT_CONFIG           configuration file (default: data dir/config.toml)
  OPENAI_API_KEY        bearer token for the assistant (or WEFT_API_KEY)
  RUST_LOG              log filter for internal diagnostics"
    );
}
