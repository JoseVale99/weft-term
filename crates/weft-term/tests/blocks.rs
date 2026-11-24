//! Parity-style tests for the terminal core: they encode what the shell stream
//! means, so a regression shows up as a diff in behaviour, not in appearance.

use weft_term::{BlockState, Blocks, Event, Parser};

/// Feeds `input` in `chunk`-sized pieces and returns every event.
fn parse(input: &[u8], chunk: usize) -> Vec<Event> {
    let mut parser = Parser::new();
    let mut events = Vec::new();
    for piece in input.chunks(chunk.max(1)) {
        parser.feed(piece, &mut events);
    }
    events
}

fn blocks_from(input: &[u8]) -> Blocks {
    let mut blocks = Blocks::new();
    for event in parse(input, 3) {
        blocks.apply(&event);
    }
    blocks
}

#[test]
fn one_command_one_block() {
    let stream = b"\x1b]133;A\x07$ \x1b]133;B\x07\x1b]133;C\x07hello\r\n\x1b]133;D;0\x07";
    let blocks = blocks_from(stream);

    assert_eq!(blocks.len(), 1);
    let block = blocks.current().expect("one block");
    assert_eq!(block.state, BlockState::Done);
    assert_eq!(block.exit_code, Some(0));
    assert_eq!(block.status_label(), "ok");
    assert!(!block.failed());
    assert!(!block.aborted);
}

#[test]
fn exit_code_is_parsed_and_surfaced() {
    let stream = b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07x\x1b]133;D;130\x07";
    let blocks = blocks_from(stream);
    let block = blocks.current().expect("one block");
    assert_eq!(block.exit_code, Some(130));
    assert!(block.failed());
    assert_eq!(blocks.last_exit_code(), Some(130));
}

#[test]
fn new_prompt_aborts_unfinished_block() {
    // A long-running command that never reported D, then a fresh prompt.
    let stream = b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07still running\x1b]133;A\x07$ ";
    let blocks = blocks_from(stream);

    assert_eq!(blocks.len(), 2);
    let first = blocks.iter().next().expect("first block");
    assert_eq!(first.state, BlockState::Done);
    assert!(first.aborted);
    assert_eq!(first.exit_code, None);
    assert_eq!(blocks.current().expect("open").state, BlockState::Prompt);
}

#[test]
fn stream_can_be_split_anywhere() {
    let stream =
        b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07out\x1b]133;D;0\x07\x1b]7;file://h/tmp/a%20b\x07";
    let whole = parse(stream, stream.len());
    let byte_by_byte = parse(stream, 1);

    assert_eq!(whole, byte_by_byte);
    assert_eq!(
        whole,
        vec![
            Event::PromptStart,
            Event::PromptEnd,
            Event::CommandStart,
            Event::CommandFinished { exit_code: Some(0) },
            Event::Cwd("file://h/tmp/a%20b".to_owned()),
        ]
    );
}

#[test]
fn cwd_uri_is_decoded() {
    let blocks = blocks_from(b"\x1b]7;file://host/home/jar/My%20Code\x07\x1b]133;A\x07");
    assert_eq!(blocks.current().expect("prompt").cwd, "/home/jar/My Code");
}

#[test]
fn cwd_updates_open_prompt_but_not_closed_blocks() {
    let stream =
        b"\x1b]133;A\x07\x1b]7;file:///a\x07\x1b]133;D;0\x07\x1b]7;file:///b\x07\x1b]133;A\x07";
    let blocks = blocks_from(stream);

    let mut iter = blocks.iter();
    assert_eq!(iter.next().expect("closed").cwd, "/a");
    assert_eq!(iter.next().expect("open").cwd, "/b");
}

#[test]
fn foreign_sequences_are_ignored() {
    // Window titles, SGR attributes, an unknown OSC and an empty DCS: none of
    // them may produce events. The trailing prompt is the only real one.
    let stream =
        b"\x1b]0;my title\x07\x1b[1;33m\x1b]1337;SetMark\x07\x1bPq#0;2;0;0;0\x1b\\\x1b]133;A\x07";
    let events = parse(stream, 2);
    assert_eq!(events, vec![Event::PromptStart]);
}

#[test]
fn escape_inside_osc_aborts_it_like_vte() {
    // DEC behaviour: ESC inside an OSC aborts it and starts a new escape
    // sequence, so a malformed title followed by a marker still frames blocks.
    // This matches the `vte` state machine.
    let events = parse(b"\x1b]0;title \x1b]133;A\x07", 8);
    assert_eq!(events, vec![Event::PromptStart]);
}

#[test]
fn st_terminator_is_accepted() {
    let events = parse(b"\x1b]133;C\x1b\\", 1);
    assert_eq!(events, vec![Event::CommandStart]);
}

#[test]
fn incomplete_sequence_emits_nothing() {
    let events = parse(b"\x1b]133;", 2);
    assert!(events.is_empty());
}

#[test]
fn command_and_output_accounting() {
    let mut blocks = blocks_from(b"\x1b]133;A\x07\x1b]133;C\x07");
    blocks.set_command("echo abc");
    blocks.add_output(3);
    for event in parse(b"\x1b]133;D;0\x07", 4) {
        blocks.apply(&event);
    }

    let block = blocks.current().expect("block");
    assert_eq!(block.command, "echo abc");
    assert_eq!(block.output_bytes, 3);
    assert_eq!(block.state, BlockState::Done);
}

#[test]
fn output_before_command_start_is_not_attributed() {
    let mut blocks = blocks_from(b"\x1b]133;A\x07");
    blocks.add_output(10);
    assert_eq!(blocks.current().expect("prompt").output_bytes, 0);
}

#[test]
fn command_without_prompt_is_still_recorded() {
    // Half-installed shell integration: no A, but C and D arrive.
    let blocks = blocks_from(b"\x1b]133;C\x07out\x1b]133;D;0\x07");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks.current().expect("block").state, BlockState::Done);
}

#[test]
fn stray_finish_is_ignored() {
    // A freshly attached shell reports D for the prompt it already drew.
    let blocks = blocks_from(b"\x1b]133;D;1\x07\x1b]133;A\x07");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks.current().expect("prompt").state, BlockState::Prompt);
    assert_eq!(blocks.last_exit_code(), None);
}

#[test]
fn many_blocks_keep_order() {
    let mut stream = Vec::new();
    for code in [0, 1, 0] {
        stream.extend_from_slice(
            format!("\x1b]133;A\x07\x1b]133;C\x07o\x1b]133;D;{code}\x07").as_bytes(),
        );
    }
    let blocks = blocks_from(&stream);

    let ids: Vec<_> = blocks.done().map(|block| block.id.0).collect();
    assert_eq!(ids, vec![0, 1, 2]);
    assert_eq!(blocks.last_exit_code(), Some(0));
}
