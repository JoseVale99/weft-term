//! Tool specifications sent to the model.
//!
//! The core only describes tools; it never executes them. Execution lives in
//! the binary because it touches the file system and the shell, and it must pass
//! through the approval gate for anything that changes state.

use crate::json::Value;

/// One function the model may call.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    /// Function name.
    pub name: &'static str,
    /// What it does, shown to the model.
    pub description: &'static str,
    /// JSON Schema for the arguments.
    pub parameters: Value,
}

/// Tools offered in every request.
///
/// `read_file` and `list_dir` are read-only. `propose_command` never runs
/// anything: it produces a proposal that the user must approve.
pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "read_file",
            description: "Read a UTF-8 text file inside the working directory.",
            parameters: object(
                vec![(
                    "path",
                    string_property("Path relative to the working directory."),
                )],
                &["path"],
            ),
        },
        ToolSpec {
            name: "list_dir",
            description: "List the entries of a directory inside the working directory.",
            parameters: object(
                vec![(
                    "path",
                    string_property("Directory relative to the working directory."),
                )],
                &["path"],
            ),
        },
        ToolSpec {
            name: "propose_command",
            description: "Propose a shell command. It is never executed without approval.",
            parameters: object(
                vec![
                    (
                        "command",
                        string_property("Exact command line to run."),
                    ),
                    (
                        "reason",
                        string_property("Why the command helps with the task."),
                    ),
                ],
                &["command", "reason"],
            ),
        },
    ]
}

/// A complete tool call assembled from streaming fragments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolCall {
    /// Provider identifier, needed to match the tool result.
    pub id: String,
    /// Function name.
    pub name: String,
    /// Raw JSON arguments.
    pub arguments: String,
}

fn object(properties: Vec<(&str, Value)>, required: &[&str]) -> Value {
    Value::Object(vec![
        ("type".to_owned(), Value::String("object".to_owned())),
        (
            "properties".to_owned(),
            Value::Object(
                properties
                    .into_iter()
                    .map(|(name, value)| (name.to_owned(), value))
                    .collect(),
            ),
        ),
        (
            "required".to_owned(),
            Value::Array(
                required
                    .iter()
                    .map(|name| Value::String((*name).to_owned()))
                    .collect(),
            ),
        ),
    ])
}

fn string_property(description: &str) -> Value {
    Value::Object(vec![
        ("type".to_owned(), Value::String("string".to_owned())),
        (
            "description".to_owned(),
            Value::String(description.to_owned()),
        ),
    ])
}
