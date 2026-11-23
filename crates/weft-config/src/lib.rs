//! Weft configuration.
//!
//! The configuration is a TOML file with three sections: `terminal`, `editor`
//! and `agent`. Every field has a default and an invalid value is a hard error:
//! a typo must never be silently ignored.
//!
//! This crate knows nothing about paths or platform directories. The caller
//! decides where the file lives, which keeps the crate pure and testable.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Settings that affect the terminal itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Terminal {
    /// Shell to run instead of the value of `SHELL` or `COMSPEC`.
    pub shell: Option<String>,
    /// Number of lines kept above the visible screen.
    pub scrollback: usize,
    /// Name of the theme to use.
    pub theme: String,
}

impl Default for Terminal {
    fn default() -> Self {
        Self {
            shell: None,
            scrollback: 10_000,
            theme: "weft-dark".to_owned(),
        }
    }
}

/// Settings for the input editor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Editor {
    /// Keep accepting line breaks instead of submitting on the first one.
    pub multiline: bool,
    /// Number of history entries kept per session.
    pub history_limit: usize,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            multiline: true,
            history_limit: 5_000,
        }
    }
}

/// Settings for the assistant.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Agent {
    /// Provider name, for example `openai`.
    pub provider: Option<String>,
    /// Model name sent with each request.
    pub model: Option<String>,
    /// Base URL for OpenAI-compatible providers.
    pub base_url: Option<String>,
    /// Never run a proposed command without asking. Defaults to false so an
    /// explicit opt-in is required to bypass the approval gate.
    pub auto_approve: bool,
}

/// The whole configuration file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Terminal settings.
    pub terminal: Terminal,
    /// Editor settings.
    pub editor: Editor,
    /// Assistant settings.
    pub agent: Agent,
}

/// Errors produced while loading, parsing or validating the configuration.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file exists but cannot be read.
    #[error("cannot read configuration {path}: {source}")]
    Read {
        /// Path of the file.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// The file exists but cannot be written.
    #[error("cannot write configuration {path}: {source}")]
    Write {
        /// Path of the file.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// The file is not valid TOML or contains an unknown key.
    #[error("invalid configuration in {path}: {source}")]
    Parse {
        /// Path of the file.
        path: PathBuf,
        /// Underlying TOML error.
        #[source]
        source: toml::de::Error,
    },
    /// A value is out of its accepted range.
    #[error("invalid value for {field}: {reason}")]
    Invalid {
        /// Name of the offending field.
        field: &'static str,
        /// Why the value was rejected.
        reason: String,
    },
}

impl Config {
    /// Reads a configuration file.
    ///
    /// A missing file yields the default configuration: first runs must not
    /// fail because the user has not created anything yet.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(Error::Read {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let config: Self = toml::from_str(&text).map_err(|source| Error::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Writes the configuration, creating parent directories as needed.
    ///
    /// The file is written to a sibling temporary file and renamed, so a crash
    /// never leaves a half-written configuration behind.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let path = path.as_ref();
        let text = toml::to_string_pretty(self).map_err(|source| Error::Write {
            path: path.to_path_buf(),
            source: io::Error::other(source.to_string()),
        })?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| Error::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let temporary = path.with_extension("toml.tmp");
        fs::write(&temporary, text).map_err(|source| Error::Write {
            path: temporary.clone(),
            source,
        })?;
        fs::rename(&temporary, path).map_err(|source| Error::Write {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Checks every value is inside its accepted range.
    pub fn validate(&self) -> Result<(), Error> {
        if !(100..=1_000_000).contains(&self.terminal.scrollback) {
            return Err(Error::Invalid {
                field: "terminal.scrollback",
                reason: "must be between 100 and 1000000".to_owned(),
            });
        }
        if self.terminal.theme.trim().is_empty() {
            return Err(Error::Invalid {
                field: "terminal.theme",
                reason: "must not be empty".to_owned(),
            });
        }
        if let Some(shell) = &self.terminal.shell
            && shell.trim().is_empty()
        {
            return Err(Error::Invalid {
                field: "terminal.shell",
                reason: "must not be empty when set".to_owned(),
            });
        }
        if !(10..=100_000).contains(&self.editor.history_limit) {
            return Err(Error::Invalid {
                field: "editor.history_limit",
                reason: "must be between 10 and 100000".to_owned(),
            });
        }
        if let Some(url) = &self.agent.base_url
            && !(url.starts_with("http://") || url.starts_with("https://"))
        {
            return Err(Error::Invalid {
                field: "agent.base_url",
                reason: "must start with http:// or https://".to_owned(),
            });
        }
        Ok(())
    }
}
