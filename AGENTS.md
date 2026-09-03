# AGENTS.md

Guidance for AI coding agents (Claude, Codex, Zed Agent, etc.) working in
the `duv` repository. Keep this file up to date as the project evolves —
it is the single source of truth for agent conventions in this repo.

## Project summary

`duv` is a fast, terminal-based disk usage manager and monitor, written in
Rust. It scans a directory tree, presents an interactive TUI showing what's
consuming disk space, and lets the user drill down and manage
(delete/inspect) files without leaving the terminal.

The project is in **early development**. Expect the architecture below to
be aspirational until it lands in code — check `src/` for the current state
before assuming something exists.

## Current state

- `src/main.rs` — thin entry point, wires up the terminal and hands off to
  `app`.
- `src/app.rs` — minimal application loop/state (draws one frame, exits on
  `q`/`Esc`/`Ctrl+C`). Scanner and model are not yet implemented.
- `src/ui.rs` — terminal setup/teardown (`crossterm` raw mode + alternate
  screen) and rendering of the current app state via `ratatui`.
- Dependency: `ratatui` (pulls in the `crossterm` backend by default).
- No scanning logic or tree model yet.

## Planned architecture

As the project grows, prefer organizing code by responsibility, e.g.:

- `scanner` — walks the filesystem, computes directory/file sizes (likely
  parallelized, e.g. with `rayon` or async I/O).
- `model` — in-memory tree representation of scanned paths and sizes.
- `ui` — terminal UI rendering and input handling. **Decided:** `ratatui`
  (with its default `crossterm` backend) — see below for rationale.
- `app` — application state/event loop tying scanner + model + ui together.
- `cli` — argument parsing (likely `clap`) and entry point in `main.rs`.

Don't create this structure preemptively — introduce modules as real
functionality is added, and keep `main.rs` thin.

### TUI framework: ratatui

`ratatui` was chosen over alternatives (`cursive`, the abandoned `tui-rs`,
raw `termion`) because:

- It's the actively maintained successor to `tui-rs` (which is deprecated),
  with a large user base and healthy release cadence.
- Its immediate-mode rendering model (redraw the full frame from state each
  tick) fits a disk-usage dashboard well, and is the same approach used by
  comparable tools (`dua-cli`, `bottom`).
- It ships a `crossterm` backend by default, giving cross-platform support
  (Windows + Unix) without extra dependency wrangling.
- It has ready-made widgets (`Table`, `List`, `Gauge`, `Sparkline`, `Block`)
  that map well onto directory listings and size visualizations.

Stick with `ratatui`/`crossterm` for all TUI work unless a concrete
limitation is found.

## Conventions

- **Edition**: Rust 2024 (`Cargo.toml` sets `edition = "2024"`). Use modern
  idioms compatible with this edition.
- **Formatting**: run `cargo fmt` before finishing any change.
- **Linting**: run `cargo clippy` and address warnings where reasonable.
- **Error handling**: prefer `Result`/`?` over `unwrap()`/`expect()` outside
  of tests and truly unreachable cases. Consider `anyhow`/`thiserror` once
  error handling grows beyond trivial cases.
- **Dependencies**: this is a terminal application meant to be fast and
  lightweight — justify new dependencies, prefer well-maintained crates,
  and avoid heavy transitive dependency trees when a simpler option exists.
- **Naming**: standard Rust naming (`snake_case` for functions/variables,
  `CamelCase` for types, `SCREAMING_SNAKE_CASE` for consts).

## Validation

Before considering a change complete, run:

```sh
cargo build
cargo test
cargo fmt --check
cargo clippy
```

If any of these fail because of pre-existing issues unrelated to your
change, note it rather than silently ignoring it.

## Working with this repo as an agent

- Keep changes minimal and scoped to what was asked. This is a young
  codebase — avoid scaffolding large amounts of unrequested structure.
- When adding a new feature area (e.g. the scanner or the TUI), add a
  short module-level doc comment (`//!`) explaining its purpose.
- Update `README.md` when user-facing behavior (CLI flags, installation,
  usage) changes.
- Update this file (`AGENTS.md`) when architecture, conventions, or
  validation steps change materially.
- Prefer real filesystem-backed integration tests for scanning logic over
  heavily mocked tests, since disk-usage correctness is the core value of
  this tool.
