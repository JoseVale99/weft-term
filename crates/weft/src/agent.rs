//! Agent command: configuration check and one-shot chat.
//!
//! The transport is plain HTTP over `std::net`, so it works with local
//! OpenAI-compatible servers (Ollama, LM Studio, llama.cpp) and needs no TLS
//! crate. HTTPS is reported as unavailable instead of failing obscurely: adding
//! a TLS client is a dependency decision, not a hidden fallback.

use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::net::TcpStream;

use weft_agent::{
    Conversation, Event, Guard, Proposal, Provider, Request, Role, StreamParser, ToolCall, parse,
};
use weft_config::{Agent, Config};

/// Runs `weft agent ...`.
pub fn run(args: &[String]) -> io::Result<()> {
    let config = Config::load(crate::config_path()).map_err(io::Error::other)?;
    match args.first().map(String::as_str) {
        None | Some("check") => check(&config.agent),
        Some("chat") => {
            let prompt = args[1..].join(" ");
            chat(&config.agent, &prompt)
        }
        Some(other) => {
            eprintln!("weft: unknown agent action `{other}` (check, chat)");
            Ok(())
        }
    }
}

fn provider(agent: &Agent) -> Result<Provider, String> {
    let model = agent
        .model
        .clone()
        .filter(|model| !model.trim().is_empty())
        .ok_or_else(|| "set `model` in the [agent] section".to_owned())?;
    let base_url = agent
        .base_url
        .clone()
        .unwrap_or_else(|| "https://api.openai.com/v1".to_owned());
    Ok(Provider { base_url, model })
}

fn check(agent: &Agent) -> io::Result<()> {
    let provider = match provider(agent) {
        Ok(provider) => provider,
        Err(message) => {
            eprintln!("weft: agent is not configured: {message}");
            return Ok(());
        }
    };
    let mut conversation = Conversation::new("check", 1);
    conversation.push(Role::User, "ping");
    let request = Request::new(provider, conversation).map_err(io::Error::other)?;
    println!("endpoint      {}", request.endpoint());
    println!("auto_approve  {}", agent.auto_approve);
    println!(
        "api key       {}",
        if api_key().is_some() {
            "present"
        } else {
            "missing (OPENAI_API_KEY or WEFT_API_KEY)"
        }
    );
    println!("streaming     server-sent events");
    Ok(())
}

fn chat(agent: &Agent, prompt: &str) -> io::Result<()> {
    if prompt.trim().is_empty() {
        eprintln!("usage: weft agent chat <prompt>");
        return Ok(());
    }
    let provider = provider(agent).map_err(io::Error::other)?;
    if provider.endpoint().starts_with("https://") {
        eprintln!(
            "weft: HTTPS transport is not available in this build; point [agent] base_url to a local http:// server"
        );
        return Ok(());
    }
    let mut conversation = Conversation::new(
        "You are Weft's assistant. Answer in the user's language and be brief. \
         Use read_file and list_dir to inspect the working directory before \
         answering questions about the project. Shell commands are proposals \
         that require the user's approval. Never invent tool output and never \
         claim to have run a command you did not run.",
        40,
    );
    conversation.push(Role::User, prompt);

    // A bounded tool loop: request, run calls, return their output, request
    // again. Without a bound a model that keeps proposing the same call could
    // spin forever.
    for _ in 0..4 {
        let request =
            Request::new(provider.clone(), conversation.clone()).map_err(io::Error::other)?;
        let events = send(&request, api_key().as_deref())?;
        let mut text = String::new();
        let mut calls = Vec::new();
        for event in events {
            match event {
                Event::Text(chunk) => {
                    print!("{chunk}");
                    io::stdout().flush()?;
                    text.push_str(&chunk);
                }
                Event::ToolCall(call) => calls.push(call),
                Event::Done => break,
            }
        }
        println!();

        if !text.is_empty() {
            conversation.push(Role::Assistant, text);
        }
        if calls.is_empty() {
            break;
        }

        let mut results = String::new();
        for call in &calls {
            let output = handle_call(agent, call)?;
            println!("{output}");
            results.push_str(&format!("### {}\n{output}\n", call.name));
        }
        conversation.push(
            Role::User,
            format!(
                "Tool output:\n{results}\nContinue with the task. Do not repeat a \
                 command that already ran."
            ),
        );
    }
    Ok(())
}

fn handle_call(agent: &Agent, call: &ToolCall) -> io::Result<String> {
    let arguments = match parse(&call.arguments) {
        Ok(arguments) => arguments,
        Err(error) => return Ok(format!("error: invalid tool arguments: {error}")),
    };
    let output = match call.name.as_str() {
        "read_file" => match read_file(&arguments) {
            Ok(text) => text,
            Err(error) => format!("error: {error}"),
        },
        "list_dir" => match list_dir(&arguments) {
            Ok(text) => text,
            Err(error) => format!("error: {error}"),
        },
        "propose_command" => {
            let command = match argument(&arguments, "command") {
                Ok(command) => command,
                Err(error) => return Ok(format!("error: {error}")),
            };
            let reason = argument(&arguments, "reason").unwrap_or_default();
            propose(agent, &command, &reason)?
        }
        other => format!("error: unknown tool `{other}`"),
    };
    Ok(output)
}

fn read_file(arguments: &weft_agent::Value) -> io::Result<String> {
    let path = argument(arguments, "path")?;
    let path = weft_agent::resolve(&path)?;
    let text = fs::read_to_string(&path)?;
    Ok(truncate(&text, 32 * 1024))
}

fn list_dir(arguments: &weft_agent::Value) -> io::Result<String> {
    let path = argument(arguments, "path")?;
    let path = weft_agent::resolve(&path)?;
    let mut entries: Vec<String> = fs::read_dir(&path)?
        .flatten()
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    entries.sort_unstable();
    let mut out = String::new();
    for entry in entries.iter().take(200) {
        out.push_str(entry);
        out.push('\n');
    }
    if entries.len() > 200 {
        out.push_str(&format!("... {} more entries\n", entries.len() - 200));
    }
    Ok(out)
}

fn argument(arguments: &weft_agent::Value, name: &str) -> io::Result<String> {
    arguments
        .get(name)
        .and_then(weft_agent::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other(format!("tool call is missing `{name}`")))
}

fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n... {} bytes omitted", &text[..end], text.len() - end)
}

fn propose(agent: &Agent, command: &str, reason: &str) -> io::Result<String> {
    if !reason.is_empty() {
        println!("weft: {reason}");
    }
    println!("weft: proposed command:\n  {command}");
    let answer = if agent.auto_approve {
        Some(true)
    } else {
        print!("run it? [y/N] ");
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        Some(matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
    };
    let proposal = Proposal::new(command, reason);
    let guard = Guard::new(agent.auto_approve);
    Ok(match guard.evaluate(&proposal, answer) {
        weft_agent::Decision::Run => {
            let block = crate::repl::execute(command)?;
            format!(
                "command ran with exit code {} in {} ms",
                block.code, block.elapsed_ms
            )
        }
        weft_agent::Decision::Rejected => "the user rejected the command".to_owned(),
        weft_agent::Decision::NeedsApproval => "the command is waiting for approval".to_owned(),
    })
}

fn api_key() -> Option<String> {
    env::var("OPENAI_API_KEY")
        .or_else(|_| env::var("WEFT_API_KEY"))
        .ok()
        .filter(|key| !key.is_empty())
}

fn send(request: &Request, key: Option<&str>) -> io::Result<Vec<Event>> {
    let endpoint = Endpoint::parse(&request.endpoint())?;
    let mut stream = TcpStream::connect((endpoint.host.as_str(), endpoint.port))?;
    let body = request.body();
    let mut head = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nAccept: text/event-stream\r\n",
        endpoint.path,
        endpoint.host,
        body.len()
    );
    if let Some(key) = key {
        head.push_str(&format!("Authorization: Bearer {key}\r\n"));
    }
    head.push_str("\r\n");

    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let (head, body) = split_headers(&raw)?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    let body = if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        dechunk(body)?
    } else {
        body.to_vec()
    };
    if status != 200 {
        let message = String::from_utf8_lossy(&body);
        return Err(io::Error::other(format!(
            "provider returned HTTP {status}: {}",
            message.trim()
        )));
    }

    let text = String::from_utf8_lossy(&body);
    let mut parser = StreamParser::new();
    let mut events = parser.push(&text).map_err(io::Error::other)?;
    events.extend(parser.finish().map_err(io::Error::other)?);
    Ok(events)
}

struct Endpoint {
    host: String,
    port: u16,
    path: String,
}

impl Endpoint {
    fn parse(url: &str) -> io::Result<Self> {
        let Some(rest) = url.strip_prefix("http://") else {
            return Err(io::Error::other(
                "only plain http:// endpoints are supported by this transport",
            ));
        };
        let (authority, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
        let (host, port) = authority
            .rsplit_once(':')
            .map_or((authority, 80), |(host, port)| {
                (host, port.parse::<u16>().unwrap_or(80))
            });
        if host.is_empty() {
            return Err(io::Error::other("endpoint has no host"));
        }
        Ok(Self {
            host: host.to_owned(),
            port,
            path: format!("/{path}"),
        })
    }
}

fn split_headers(raw: &[u8]) -> io::Result<(&str, &[u8])> {
    let marker = b"\r\n\r\n";
    let Some(index) = raw.windows(marker.len()).position(|window| window == marker) else {
        return Err(io::Error::other("malformed HTTP response: no header end"));
    };
    let head = std::str::from_utf8(&raw[..index])
        .map_err(|_| io::Error::other("response headers are not UTF-8"))?;
    Ok((head, &raw[index + marker.len()..]))
}

fn dechunk(body: &[u8]) -> io::Result<Vec<u8>> {
    let mut rest = body;
    let mut out = Vec::new();
    loop {
        let Some(end) = rest.windows(2).position(|window| window == b"\r\n") else {
            return Err(io::Error::other("malformed chunked body"));
        };
        let size_text = std::str::from_utf8(&rest[..end])
            .map_err(|_| io::Error::other("malformed chunk size"))?;
        let size_text = size_text.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| io::Error::other("malformed chunk size"))?;
        rest = &rest[end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if rest.len() < size {
            return Err(io::Error::other("truncated chunk"));
        }
        out.extend_from_slice(&rest[..size]);
        rest = rest.get(size + 2..).unwrap_or(&[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_keep_host_port_and_path() {
        let endpoint = Endpoint::parse("http://localhost:11434/v1/chat/completions").unwrap();
        assert_eq!(endpoint.host, "localhost");
        assert_eq!(endpoint.port, 11434);
        assert_eq!(endpoint.path, "/v1/chat/completions");

        let endpoint = Endpoint::parse("http://example.test/v1").unwrap();
        assert_eq!(endpoint.port, 80);
    }

    #[test]
    fn https_is_reported_not_guessed() {
        assert!(Endpoint::parse("https://api.openai.com/v1").is_err());
    }

    #[test]
    fn chunked_bodies_are_reassembled() {
        let body = b"4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
        assert_eq!(dechunk(body).unwrap(), b"Wikipedia");
    }

    #[test]
    fn a_truncated_chunk_is_an_error() {
        assert!(dechunk(b"5\r\nabc").is_err());
    }

    #[test]
    fn headers_are_split_from_the_body() {
        let raw = b"HTTP/1.1 200 OK\r\nX: y\r\n\r\nhello";
        let (head, body) = split_headers(raw).unwrap();
        assert!(head.contains("200 OK"));
        assert_eq!(body, b"hello");
    }
}
