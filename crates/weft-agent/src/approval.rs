//! Approval gate for commands proposed by the model.
//!
//! The gate is a pure decision function. It does not run anything and it does
//! not remember answers: the caller owns the prompt and the execution, which
//! keeps approval testable and impossible to bypass by accident.

/// A command the model wants to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    /// Exact command line to execute.
    pub command: String,
    /// Why the model proposes it, shown to the user.
    pub reason: String,
}

impl Proposal {
    /// Creates a proposal.
    pub fn new(command: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            reason: reason.into(),
        }
    }
}

/// The user's answer to a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Run the command as proposed.
    Run,
    /// The user said no.
    Rejected,
    /// The user has not answered yet.
    NeedsApproval,
}

/// Approval policy plus the explicit answer for one proposal.
#[derive(Debug, Clone, Copy)]
pub struct Guard {
    auto_approve: bool,
}

impl Guard {
    /// Creates a gate. `auto_approve` bypasses the prompt, which is an explicit
    /// opt-in in the configuration and never the default.
    pub fn new(auto_approve: bool) -> Self {
        Self { auto_approve }
    }

    /// Decides what to do with `proposal`.
    ///
    /// An empty command is always rejected, even with `auto_approve`: a blank
    /// line is a malformed proposal, not an approval.
    pub fn evaluate(&self, proposal: &Proposal, user_answer: Option<bool>) -> Decision {
        if proposal.command.trim().is_empty() {
            return Decision::Rejected;
        }
        if self.auto_approve {
            return Decision::Run;
        }
        match user_answer {
            Some(true) => Decision::Run,
            Some(false) => Decision::Rejected,
            None => Decision::NeedsApproval,
        }
    }
}
