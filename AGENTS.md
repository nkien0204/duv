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

The terminal shell follows a Model-Update-View split across five modules
(`app`, `event`, `update`, `ui`, `tui`) plus a thin `main.rs` and a `cli`
module (`duv [path]`: with a path, e.g. `duv .`, `App::with_start_path` scans it
immediately; without one, the disk list shows). A `disks`
module enumerates mounted disks/volumes (via `sysinfo`), feeding `App` a
plain, crate-local `DiskInfo` list that `ui` renders as a selectable table.
Pressing `s` on a selected disk triggers `scanner::Scanner`, which walks
everything under that directory on a background thread (parallelized with
`rayon`) and streams each directory's listing back via an `mpsc` channel
polled on every `Event::Tick`, adding it to a `model::Tree` (a flat,
`u32`-indexed node list of scanned files and directories) kept within a
memory budget (`--memory-budget`, default 256 MiB): over budget,
directories are stored as totals only, loaded in place when opened, and
least recently visited branches are evicted to make room. `ui`
shows a progress `Gauge` (with a border) while it runs and a size-sorted
`Table` once finished. Drilling into a subdirectory (`l`/`Right`/`Enter`)
and backing out (`h`/`Left`/`Backspace`/`Esc`) move around that tree with
no rescanning (or a load in place for budget-skipped directories); only
directories on another filesystem or unreadable ones spawn another
`Scanner`, with the previous one
kept on `App::scanner_history`. A confirmation popup appears when the user
presses `q` to quit, allowing them to choose Yes or No. `d`/`Delete` asks
(same kind of popup, defaulting to No) to move the highlighted entry to
the Trash via the `trash` crate; on success it's removed from the current
scan's tree and from every scan in `App::scanner_history`. `/` filters the
current folder's entries by name as you type (`Scanner::set_filter`); the
filter belongs to that folder and is dropped when navigating away. `t`
cycles the sort column (size, name, modified) and `r` reverses its
direction, for every folder and scan; each
tree node stores its modification time. `o`
shows the highlighted entry in the file manager and `y` copies its path;
these and the Trash go through `src/desktop.rs`, which shells out to the
platform's own tools (or OSC 52 over SSH) instead of adding GUI crates. See [ARCHITECTURE.md](./ARCHITECTURE.md)
for the full breakdown of each module and how they communicate — keep
that document up to date alongside this one whenever the module structure
changes.

- Dependencies: `ratatui` (pulls in the `crossterm` backend by default),
  `anyhow` (error propagation), `clap` (`duv [path]` parsing in `src/cli.rs`,
  minimal feature set), `sysinfo` (disk/volume enumeration only,
  via `default-features = false, features = ["disk"]`), `rayon`
  (parallel directory-size scanning), `trash` (moving entries to the
  system Trash), `libc` (Unix only, peak memory for the hidden `--stats`
  developer flag).
- Disk enumeration (`src/disks.rs`), full-tree directory scanning
  (`src/scanner.rs`) into a memory-budgeted in-memory tree
  (`src/model.rs`), drill-down navigation of that tree, and moving entries
  to the Trash exist; no other manage actions yet.
- Keybindings support both vim-style (`j`/`k`/`h`/`l`, `gg`/`G`,
  `Ctrl+d`/`Ctrl+u`) and non-vim (arrow keys, `Enter`, `Backspace`,
  `Home`/`End`, `PgUp`/`PgDn`) navigation for the same actions.

## Planned architecture

As the project grows, prefer organizing code by responsibility, e.g.:

- `scanner` — walks the filesystem, computes directory/file sizes (likely
  parallelized, e.g. with `rayon` or async I/O).
- `model` — in-memory tree representation of scanned paths and sizes;
  implemented in `src/model.rs`.
- `ui` — terminal UI rendering and input handling. **Decided:** `ratatui`
  (with its default `crossterm` backend) — see below for rationale.
- `app` — application state/event loop tying scanner + model + ui together.
- `cli` — argument parsing (`clap`); implemented in `src/cli.rs`.

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

CI (`.github/workflows/ci.yml`) runs the same checks on every push to
`main` and every pull request, on Linux, macOS and Windows, with
`cargo clippy --all-targets -- -D warnings` (so any warning fails the
build). Platform-specific code (`#[cfg(unix)]`, `#[cfg(target_os =
"macos")]`) must therefore compile without warnings everywhere: gate
helpers, fields and imports that only one platform uses, and gate tests
that depend on platform behaviour. To check other platforms locally, add
the target (`rustup target add x86_64-pc-windows-msvc` or
`x86_64-unknown-linux-gnu`) and run
`cargo clippy --target <target> --all-targets -- -D warnings`.

For changes to scanning or the tree model, also compare
`cargo build --release && ./target/release/duv --stats <path>` before and
after on a large directory (e.g. your home directory) to catch time or
memory regressions.

## Releasing

1. Add user-facing changes to the `[Unreleased]` section of
   `CHANGELOG.md` as they land.
2. To release: bump `version` in `Cargo.toml` (and `Cargo.lock` via
   `cargo build`), rename `[Unreleased]` in `CHANGELOG.md` to the new
   version with today's date (adding a fresh empty `[Unreleased]` and
   updating the compare links at the bottom), and merge to `main`.
3. Tag and push: `git tag vX.Y.Z && git push origin vX.Y.Z`. The release
   workflow (`.github/workflows/release.yml`) checks that the tag matches
   `Cargo.toml` and that `CHANGELOG.md` has that section, runs the tests,
   builds binaries for macOS (arm64, x86_64), Linux (x86_64, static
   musl) and Windows (x86_64), and publishes a GitHub Release with
   checksums and that changelog section as the notes.
4. Publish to crates.io by hand: `cargo publish` (needs a crates.io
   token).

`rust-version` in `Cargo.toml` (currently 1.95, set by `sysinfo`) is
checked by the `msrv` CI job; raise both together when a dependency or
language feature needs a newer Rust.

## Working with this repo as an agent

- Keep changes minimal and scoped to what was asked. This is a young
  codebase — avoid scaffolding large amounts of unrequested structure.
- When adding a new feature area (e.g. the scanner or the TUI), add a
  short module-level doc comment (`//!`) explaining its purpose.
- Update `README.md` when user-facing behavior (CLI flags, installation,
  usage) changes, and add an entry to `CHANGELOG.md`'s `[Unreleased]`
  section.
- Update this file (`AGENTS.md`) when architecture, conventions, or
  validation steps change materially.
- When adding or changing a key binding, update the `HELP` table in
  `src/ui.rs` (the `?` popup) and the README's key table too; keep the
  popup within an 80×24 terminal (`help_fits_a_standard_terminal`).
- Prefer real filesystem-backed integration tests for scanning logic over
  heavily mocked tests, since disk-usage correctness is the core value of
  this tool.
