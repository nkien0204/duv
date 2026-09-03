# duv (Disk Usage Visualizer)

A fast terminal-based disk usage manager and monitor, written in Rust.

`duv` scans directories, shows an interactive breakdown of what's consuming
disk space, and lets you drill down and clean up files without leaving the
terminal.

> **Status:** early development. The core scanning/UI is not implemented yet.

## Features (planned)

- 🚀 Fast, parallel directory scanning
- 📊 Interactive terminal UI (TUI) for browsing disk usage, sorted by size
- 🌳 Drill-down navigation through nested directories
- 🗑️ Delete files/directories directly from the UI (with confirmation)
- 🔍 Filter/search by name, extension, or size
- 📈 Live monitoring mode to watch disk usage change over time
- 🎨 Configurable themes and keybindings
- 💻 Cross-platform (Linux, macOS, Windows)

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

Runs `duv` against the given path (defaults to the current directory).

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
