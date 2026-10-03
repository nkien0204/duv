# duv (Disk Usage Visualizer)

[![CI](https://github.com/nkien0204/duv/actions/workflows/ci.yml/badge.svg)](https://github.com/nkien0204/duv/actions/workflows/ci.yml)

A fast terminal-based disk usage manager and monitor, written in Rust.

`duv` scans directories, shows an interactive breakdown of what's consuming
disk space, and lets you drill down and clean up files without leaving the
terminal.

> **Status:** early development. Core disk enumeration and directory scanning are implemented.

## Features (planned)

- 🚀 Fast, parallel directory scanning
- 📊 Interactive terminal UI (TUI) for browsing disk usage, sorted by size
- 🌳 Drill-down navigation through nested directories
- 🗑️ Delete files/directories directly from the UI (with confirmation)
- 🔍 Filter/search by name, extension, or size
- 📈 Live monitoring mode to watch disk usage change over time
- 🎨 Configurable themes and keybindings
- 💻 Cross-platform (Linux, macOS, Windows)

`/` filters the current folder by name as you type (case-insensitive, any
part of the name, so `.mp4` or `cache` both work). `Enter` keeps the
filter and returns to navigation, `Esc` cancels or clears it, and opening
or leaving the folder clears it too.

`o` opens the file manager with the entry selected (Finder, Explorer; on
Linux, its folder via `xdg-open`). Where there's no desktop to show it on —
over SSH, or on a server without a graphical display — it says so and
suggests `y` instead. `y` copies the entry's full path using
the system clipboard tool (`pbcopy`, PowerShell, or `wl-copy`/`xclip`/`xsel`
on Linux). Over SSH, it asks your terminal to copy instead (OSC 52), so the
path lands on *your* machine's clipboard; most modern terminals support
this, macOS Terminal.app does not. Both also work on the disk list.

Deleting moves the entry to the system Trash (Recycle Bin on Windows), so
it can be restored; the space is freed once the Trash is emptied. The
confirmation popup defaults to **No**.

## Installation

```sh
cargo install duv
```

Or build from source:

```sh
git clone https://github.com/nkien0204/duv.git
cd duv
cargo build --release
```

## Usage

```sh
duv [path]
```

Without arguments, `duv` opens on the list of mounted disks. With a path
(e.g. `duv .` for the current directory), it starts by scanning that
directory; pressing `h`/`Esc` from there goes back to the disk list. Also
supports `--help` and `--version`.

### Memory use

`duv` keeps what it scans in memory so browsing is instant, up to a
budget of 256 MiB by default (roughly 3 million files and folders). On
larger trees, folders beyond the budget are kept as totals only (sizes
stay exact) and are scanned again when you open them, dropping the
folders you visited least recently to make room. Change the budget with:

```sh
duv --memory-budget 512 [path]
```

The budget covers `duv`'s own record of the tree; the process as a whole
uses more (typically up to about 1.5× the budget, plus some fixed
overhead).

## Controls

| Key                             | Action                     |
| ------------------------------- | -------------------------- |
| `j` / `k` / `↓` / `↑`           | Move selection             |
| `g` `g` / `Home`                | Jump to first row          |
| `G` / `End`                     | Jump to last row           |
| `PgDn` / `PgUp`                 | Move one page down / up    |
| `Ctrl+d` / `Ctrl+u`             | Move half a page down / up |
| `l` / `Enter` / `→`             | Open directory             |
| `h` / `Backspace` / `←` / `Esc` | Go back / Cancel           |
| `s`                             | Start/Rescan               |
| `d` / `Delete`                  | Move to Trash (asks first) |
| `/`                             | Filter current folder      |
| `o`                             | Show in file manager       |
| `y`                             | Copy full path             |
| `?`                             | Show all keys              |
| `q`                             | Quit (with confirmation)   |
| `Ctrl+C`                        | Quit unconditionally       |

## Development

This project uses standard Cargo workflows:

```sh
cargo build
cargo run
cargo test
cargo fmt
cargo clippy
```

See [AGENTS.md](./AGENTS.md) for conventions and guidance if you're
contributing with the help of an AI coding agent.

## License

Licensed under the [MIT License](./LICENSE).
