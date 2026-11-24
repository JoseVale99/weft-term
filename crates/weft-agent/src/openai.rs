//! OpenAI-compatible chat protocol.
//!
//! The module builds the JSON request and reads server-sent events. It performs
//! no I/O: the caller owns the HTTP client, which keeps the protocol testable
//! and lets the binary choose its transport without polluting the core.

use std::fmt;

use crate::json::{self, JsonError, Value};
use crate::message::Conversation;

/// Where to send the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    /// Base URL, for example `https://api.openai.com/v1`.
    pub base_url: String,
    /// Model name sent in the request body.
    pub model: String,
}

impl Provider {
    /// Endpoint for chat completions.
    pub fn endpoint(&self) -> String {
        format!(
            "{}/chat/completions",
            self.base_url.trim_end_matches('/')
        )
    }
}

/// Why a request could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestError {
    /// The model name is empty.
    EmptyModel,
    /// The base URL is not HTTP or HTTPS.
    InvalidBaseUrl,
    /// There is nothing to send besides the system prompt.
    EmptyConversation,
}

impl fmt::Display for RequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::EmptyModel => "the agent model is empty",
            Self::InvalidBaseUrl => "the agent base URL must start with http:// or https://",
            Self::EmptyConversation => "the conversation has no user or assistant messages",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for RequestError {}

/// A chat completion request.
#[derive(Debug, Clone)]
pub struct Request {
    provider: Provider,
    conversation: Conversation,
}

impl Request {
    /// Validates the provider and the conversation.
    pub fn new(provider: Provider, conversation: Conversation) -> Result<Self, RequestError> {
        if provider.model.trim().is_empty() {
            return Err(RequestError::EmptyModel);
        }
        if !(provider.base_url.starts_with("http://")
            || provider.base_url.starts_with("https://"))
        {
            return Err(RequestError::InvalidBaseUrl);
        }
        if conversation.is_empty() {
            return Err(RequestError::EmptyConversation);
        }
        Ok(Self {
            provider,
            conversation,
        })
    }

    /// Endpoint the request must be sent to.
    pub fn endpoint(&self) -> String {
        self.provider.endpoint()
    }

    /// Compact JSON body, with streaming enabled.
    pub fn body(&self) -> String {
        let messages = self
            .conversation
            .messages()
            .iter()
            .map(|message| {
                Value::Object(vec![
                    (
                        "role".to_owned(),
                        Value::String(message.role.as_str().to_owned()),
                    ),
                    ("content".to_owned(), Value::String(message.content.clone())),
                ])
            })
            .collect();
        Value::Object(vec![
            (
                "model".to_owned(),
                Value::String(self.provider.model.clone()),
            ),
            ("messages".to_owned(), Value::Array(messages)),
            ("stream".to_owned(), Value::Bool(true)),
        ])
        .to_json()
    }
}

/// One decoded streaming event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A piece of assistant text.
    Text(String),
    /// The provider finished the response.
    Done,
}

/// Incremental server-sent-events reader.
#[derive(Debug, Default)]
pub struct StreamParser {
    buffer: String,
}

impl StreamParser {
    /// Creates an empty parser.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds a chunk and returns every event completed by it.
    ///
    /// A line may be split across chunks; the parser keeps the tail and retries
    /// on the next call.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<Event>, JsonError> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();
        while let Some(index) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=index).collect();
            self.line(line.trim_end_matches(['\r', '\n']), &mut events)?;
        }
        Ok(events)
    }

    /// Processes a trailing line that arrived without a newline.
    pub fn finish(&mut self) -> Result<Vec<Event>, JsonError> {
        let line = std::mem::take(&mut self.buffer);
        let mut events = Vec::new();
        if !line.is_empty() {
            self.line(line.trim_end(), &mut events)?;
        }
        Ok(events)
    }

    fn line(&self, line: &str, events: &mut Vec<Event>) -> Result<(), JsonError> {
        let Some(data) = line.strip_prefix("data:") else {
            // Comments, ids and event names carry nothing we need.
            return Ok(());
        };
        let data = data.trim();
        if data.is_empty() {
            return Ok(());
        }
        if data == "[DONE]" {
            events.push(Event::Done);
            return Ok(());
        }
        let value = json::parse(data)?;
        if let Some(text) = value
            .get("choices")
            .and_then(|choices| choices.index(0))
            .and_then(|choice| choice.get("delta"))
            .and_then(|delta| delta.get("content"))
            .and_then(Value::as_str)
            && !text.is_empty()
        {
            events.push(Event::Text(text.to_owned()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Role;

    fn request() -> Request {
        let mut conversation = Conversation::new("be brief", 10);
        conversation.push(Role::User, "list files");
        Request::new(
            Provider {
                base_url: "https://example.test/v1/".to_owned(),
                model: "test-model".to_owned(),
            },
            conversation,
        )
        .unwrap()
    }

    #[test]
    fn the_endpoint_has_one_slash() {
        assert_eq!(
            request().endpoint(),
            "https://example.test/v1/chat/completions"
        );
    }

    #[test]
    fn the_body_streams_and_carries_every_message() {
        let body = request().body();
        assert!(body.contains(r#""model":"test-model""#));
        assert!(body.contains(r#""stream":true"#));
        assert!(body.contains(r#""role":"system""#));
        assert!(body.contains(r#""content":"list files""#));
    }

    #[test]
    fn invalid_providers_are_rejected() {
        let conversation = Conversation::new("system", 10);
        let provider = Provider {
            base_url: "ftp://example.test".to_owned(),
            model: "m".to_owned(),
        };
        assert_eq!(
            Request::new(provider, conversation.clone()).unwrap_err(),
            RequestError::InvalidBaseUrl
        );

        let provider = Provider {
            base_url: "https://example.test".to_owned(),
            model: "  ".to_owned(),
        };
        assert_eq!(
            Request::new(provider, conversation.clone()).unwrap_err(),
            RequestError::EmptyModel
        );
        assert_eq!(
            Request::new(
                Provider {
                    base_url: "https://example.test".to_owned(),
                    model: "m".to_owned(),
                },
                conversation
            )
            .unwrap_err(),
            RequestError::EmptyConversation
        );
    }

    #[test]
    fn events_split_across_chunks_are_reassembled() {
        let mut parser = StreamParser::new();
        assert_eq!(
            parser
                .push("data: {\"choices\":[{\"delta\":{\"cont")
                .unwrap(),
            []
        );
        let events = parser
            .push("ent\":\"hel\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n")
            .unwrap();
        let events = events
            .into_iter()
            .chain(parser.finish().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            events,
            [Event::Text("hel".to_owned()), Event::Text("lo".to_owned())]
        );
    }

    #[test]
    fn done_and_ignored_lines_are_handled() {
        let mut parser = StreamParser::new();
        let events = parser
            .push(": comment\nevent: message\ndata: [DONE]\n")
            .unwrap();
        assert_eq!(events, [Event::Done]);
    }

    #[test]
    fn malformed_data_is_an_error() {
        let mut parser = StreamParser::new();
        assert!(parser.push("data: {oops}\n").is_err());
    }
}
