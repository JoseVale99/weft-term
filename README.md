# Weft

A native terminal with agents, written in Rust. Blocks, a real input editor and
an agent that works with what is on your screen, without a browser engine
underneath.

Weft is an independent project inspired by the Warp product experience. It is
not affiliated with Warp, and it contains no Warp code, assets or trademarks.

## Status

Early development, but already a working terminal on Unix. Version `0.0.0`.

| Area | State |
| --- | --- |
| Escape-sequence interpreter (CSI, OSC, DCS) | Implemented, covered by tests |
| Screen grid, scrollback, alternate screen, SGR | Implemented, covered by tests |
| PTY session driver (Linux, macOS) | Implemented, end-to-end test |
| Block model from OSC 133/633 and OSC 7 | Implemented, covered by tests |
| Full-screen front-end with status bar | Implemented (Unix) |
| Block shell front-end | Implemented (all platforms) |
| Shell integration scripts (bash, zsh, fish) | Implemented, loadable today |
| GPU window (winit, wgpu, glyphon) | Next milestone |
| Input editor, agent, persistence | Planned |

## Try it

```sh
cargo build
./target/debug/weft                 # interactive terminal on this machine
./target/debug/weft run 'git log'   # run one command, report its block
./target/debug/weft shell           # block shell without a PTY
./target/debug/weft integration bash >> ~/.bashrc
```

The interactive front-end runs your `$SHELL` on a real pseudo-terminal, renders
the screen with an ANSI renderer, keeps a scrollback buffer and shows a
status bar with the working directory, the state of the current block and the
exit code of the last command.

## What Weft is

A terminal where a command is a first-class object. Each prompt, the command and
its output form a block: it has an exit code, a working directory, a duration,
and can be collapsed, searched, copied or handed to an agent.

The terminal behavior comes from the shell, not from guessing. Weft speaks the
same semantic-prompt protocol used by iTerm2 and VS Code (OSC 133), so blocks
are exact with `vim`, `tmux`, multiline commands and `ssh`. When the shell does
not report a boundary, Weft says so instead of inventing one.

Everything that runs is Rust. There is no JavaScript runtime, no embedded
browser and no Python at runtime. The only non-Rust code in a user session is
the shell integration script the user sources in their own shell.

## Principles

- Native rendering. A text grid owned by the application, not a DOM, with a
  GPU path as the next milestone.
- Deterministic core. Parsing and block logic are pure, replayable and tested
  against recorded sessions.
- Honest degradation. Missing shell integration is a visible state, not a silent
  bug.
- Agent with approval. The agent proposes commands; execution requires an
  explicit action from the user.
- Offline first. Without a configured provider, the terminal works normally.

## How it works

```
keys -> PTY master -> shell -> PTY -> escape interpreter -> grid -> frame
                                 \-> OSC 133/633 -> block model -> status bar
```

The escape interpreter is an incremental state machine. It can be fed arbitrary
chunks, answers device status and attribute queries the way xterm does, and
ignores what it does not understand. The block model is driven only by the
shell's semantic markers, never by guessing prompt boundaries.

## Planned feature set for v0.1

| Feature | Scope |
| --- | --- |
| Terminal | VT/ANSI, true color, scrollback, selection, search, resize, mouse |
| Blocks | Exit code, duration, collapse, navigation, copy command or output |
| Input | Multiline editor, history search, completion, bracketed paste, IME |
| Shell | bash, zsh, fish integration with install and rollback |
| Window | Tabs, splits, command palette, themes, configurable keybindings |
| Workflows | Parameterized saved commands with validation |
| Agent | OpenAI-compatible providers, streaming, tools, approval gate |
| Storage | Local history and sessions in an embedded database |
| Platforms | Linux, macOS and Windows |

## Architecture

```
crates/weft         application: front-ends, renderer, input
crates/weft-term    PTY sessions, VT grid, scrollback, block model
crates/weft-config  configuration, themes, keybindings   (planned)
crates/weft-store   history, blocks, sessions            (planned)
crates/weft-agent   model providers, tools               (planned)
shell/              bash, zsh and fish integration
docs/               architecture and stack
```

`weft-term` has no UI dependencies. The UI depends on it, never the reverse.
The full design is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Building

Requirements:

- Rust stable (edition 2024, MSRV 1.88)
- Linux and macOS: a C toolchain is not needed; the terminal engine talks to
  POSIX directly
- Windows: MSVC toolchain for the block shell

The current build has no external crates, so it builds offline:

```sh
cargo build
cargo test        # 33 tests across the engine, the block model and a real PTY
```

The GPU window will add `winit`, `wgpu` and `glyphon`. Their versions are
resolved by a script instead of being guessed in the manifest:

```sh
./scripts/bootstrap-deps.sh
```

## Platform support

| Platform | Full-screen terminal | PTY | Status |
| --- | --- | --- | --- |
| Linux (Wayland, X11) | ANSI front-end | `posix_openpt` | Working |
| macOS (11+) | ANSI front-end | `posix_openpt` | Working |
| Windows (10+) | Block shell | ConPTY | Block shell working, full-screen pending |

Platform-specific behavior (IME, modifier keys, clipboard, credential storage)
is isolated behind small modules so no platform leaks into the core. See
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Rust stack and rationale](docs/STACK.md)

## License

MIT. See [LICENSE](LICENSE).

Warp is a trademark of Warp Technologies, Inc. Weft is not affiliated with,
endorsed by, or derived from Warp.
