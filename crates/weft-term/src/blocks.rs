//! The block model: the unit Warp made popular and the unit our UI renders,
//! scrolls, searches, copies and feeds to the agent.
//!
//! A block binds together one command, its output and its exit code. Blocks are
//! driven only by the semantic events from [`crate::osc`], so the model is a
//! deterministic function of the shell byte stream and can be replayed in tests.
//! The UI layer never guesses where a command started: if the shell does not
//! report it, the block stays attached to the previous prompt instead of being
//! invented.

use core::fmt;

use crate::osc::{cwd_from_uri, Event};

/// Stable identifier for a block within one session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockId(pub u64);

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Lifecycle of a block, driven by OSC 133 A/B/C/D.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BlockState {
    /// `A` seen: the prompt is being drawn.
    #[default]
    Prompt,
    /// `B`..`D`: the user submitted a command, its output is streaming.
    Running,
    /// `D` seen (or superseded by a new prompt): the block is closed.
    Done,
}

/// One command and everything it produced.
#[derive(Clone, Debug, Default)]
pub struct Block {
    pub id: BlockId,
    /// Command line as captured by our input editor (not scraped from the PTY).
    pub command: String,
    /// Working directory reported by OSC 7 the last time it was known.
    pub cwd: String,
    /// Exit code reported by OSC 133;D. `None` when the shell stayed silent.
    pub exit_code: Option<i32>,
    pub state: BlockState,
    /// Number of output bytes attributed to this block. The UI uses it to size
    /// the scrollback slice; the bytes themselves live in the terminal grid.
    pub output_bytes: usize,
    /// True when a new prompt arrived before this block reported `D`.
    pub aborted: bool,
}

impl Block {
    /// Whether the command is known to have failed.
    pub fn failed(&self) -> bool {
        matches!(self.exit_code, Some(code) if code != 0)
    }

    /// Exit code rendered the way shells do it: `None` for still running.
    pub fn status_label(&self) -> &'static str {
        match (self.state, self.exit_code) {
            (BlockState::Running, _) => "running",
            (BlockState::Done, Some(0)) => "ok",
            (BlockState::Done, Some(_)) => "failed",
            _ => "pending",
        }
    }
}

/// Ordered block list for one session.
#[derive(Debug, Default)]
pub struct Blocks {
    blocks: Vec<Block>,
    next_id: u64,
    /// Last cwd reported while no prompt was open.
    pending_cwd: String,
}

impl Blocks {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one semantic event. Returns the block it touched, if any.
    pub fn apply(&mut self, event: &Event) -> Option<BlockId> {
        match event {
            Event::PromptStart => Some(self.open_prompt()),
            Event::PromptEnd => None,
            Event::CommandStart => Some(self.start_command()),
            Event::CommandFinished { exit_code } => self.finish_command(*exit_code),
            Event::Cwd(uri) => {
                let cwd = cwd_from_uri(uri);
                self.pending_cwd = cwd.clone();
                // A prompt can be redrawn (resize, Ctrl-C); keep the open block
                // accurate, but never rewrite closed history.
                if let Some(block) = self.blocks.last_mut()
                    && block.state == BlockState::Prompt
                {
                    block.cwd = cwd;
                }
                None
            }
        }
    }

    /// Closes the open block as aborted and opens a fresh one.
    fn open_prompt(&mut self) -> BlockId {
        if let Some(previous) = self.blocks.last_mut()
            && previous.state != BlockState::Done
        {
            previous.state = BlockState::Done;
            previous.aborted = true;
        }
        let id = BlockId(self.next_id);
        self.next_id += 1;
        self.blocks.push(Block {
            id,
            cwd: self.pending_cwd.clone(),
            ..Block::default()
        });
        id
    }

    fn start_command(&mut self) -> BlockId {
        let id = match self.blocks.last() {
            Some(block) if block.state == BlockState::Prompt => block.id,
            Some(block) if block.state == BlockState::Running => return block.id,
            // `C` without a preceding `A` (shell integration half installed):
            // still record the command instead of dropping its output.
            _ => {
                let id = BlockId(self.next_id);
                self.next_id += 1;
                self.blocks.push(Block {
                    id,
                    cwd: self.pending_cwd.clone(),
                    ..Block::default()
                });
                id
            }
        };
        if let Some(block) = self.blocks.last_mut() {
            block.state = BlockState::Running;
        }
        id
    }

    /// `D` with no open block is a stray report (a fresh shell emits one for
    /// the prompt it drew before we attached). It is ignored instead of
    /// fabricating an empty block.
    fn finish_command(&mut self, exit_code: Option<i32>) -> Option<BlockId> {
        let block = self.blocks.last_mut()?;
        block.state = BlockState::Done;
        block.exit_code = exit_code;
        Some(block.id)
    }

    /// Records the command line for the open block. Called by the input editor
    /// when the user submits; the PTY stream never contains it reliably.
    pub fn set_command(&mut self, command: impl Into<String>) -> Option<BlockId> {
        let block = self.blocks.last_mut()?;
        block.command = command.into();
        Some(block.id)
    }

    /// Attributes `bytes` of terminal output to the running block.
    pub fn add_output(&mut self, bytes: usize) {
        if let Some(block) = self.blocks.last_mut()
            && block.state == BlockState::Running
        {
            block.output_bytes += bytes;
        }
    }

    pub fn current(&self) -> Option<&Block> {
        self.blocks.last()
    }

    pub fn get(&self, id: BlockId) -> Option<&Block> {
        self.blocks.iter().find(|block| block.id == id)
    }

    /// Every block, oldest first, including the in-progress one.
    pub fn iter(&self) -> impl Iterator<Item = &Block> {
        self.blocks.iter()
    }

    /// Closed blocks only: what the palette, search and history see.
    pub fn done(&self) -> impl DoubleEndedIterator<Item = &Block> {
        self.blocks
            .iter()
            .filter(|block| block.state == BlockState::Done)
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn last_exit_code(&self) -> Option<i32> {
        self.done().next_back().and_then(|block| block.exit_code)
    }
}
