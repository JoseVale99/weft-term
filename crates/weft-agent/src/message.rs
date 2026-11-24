//! Conversation model.
//!
//! A conversation is an ordered list of messages with a single system prompt.
//! Roles are explicit instead of strings so a provider adapter cannot send a
//! typo as a role name.

/// Who produced a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Instructions that frame the conversation.
    System,
    /// Input from the person using Weft.
    User,
    /// Output from the model.
    Assistant,
    /// Output from a tool the model called.
    Tool,
}

impl Role {
    /// Wire name used by OpenAI-compatible providers.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

/// One message in a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Who produced it.
    pub role: Role,
    /// Text content.
    pub content: String,
}

impl Message {
    /// Creates a message.
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }
}

/// An ordered conversation with a bounded size.
#[derive(Debug, Clone)]
pub struct Conversation {
    messages: Vec<Message>,
    limit: usize,
}

impl Conversation {
    /// Starts a conversation with a system prompt, keeping at most `limit`
    /// non-system messages.
    pub fn new(system: impl Into<String>, limit: usize) -> Self {
        Self {
            messages: vec![Message::new(Role::System, system)],
            limit: limit.max(1),
        }
    }

    /// Appends a message, evicting the oldest non-system message at the limit.
    pub fn push(&mut self, role: Role, content: impl Into<String>) {
        if role != Role::System {
            let non_system = self
                .messages
                .iter()
                .filter(|message| message.role != Role::System)
                .count();
            if non_system >= self.limit
                && let Some(index) = self
                    .messages
                    .iter()
                    .position(|message| message.role != Role::System)
            {
                self.messages.remove(index);
            }
        }
        self.messages.push(Message::new(role, content));
    }

    /// All messages, oldest first.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Number of stored messages, including the system prompt.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Whether only the system prompt is present.
    pub fn is_empty(&self) -> bool {
        self.messages.len() <= 1
    }

    /// The most recent message.
    pub fn last(&self) -> Option<&Message> {
        self.messages.last()
    }
}
