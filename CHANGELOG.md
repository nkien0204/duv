# Changelog

All notable changes to this project are documented here. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-10-03

First release.

### Added

- Disk list showing every mounted disk with used, available and total
  space; `s` scans the highlighted one.
- `duv [path]` to start by scanning a folder (`duv .` for the current
  one), plus `--help` and `--version`.
- Fast parallel scanning with a live progress popup; sizes are measured
  like `du -x` (allocated size, hard links counted once, symlinks not
  followed, other filesystems left out).
- Instant drill-down and back navigation over the scanned tree, keeping
  each folder's selection and scroll position.
- Sorting by size, name or modification time (`t`), in either direction
  (`r`), with a Modified column showing each entry's age.
- Filtering the current folder by name as you type (`/`).
- Moving files and folders to the Trash (`d`), with a confirmation that
  defaults to No; totals update without rescanning. The confirmation
  says how much emptying the Trash would free (naming the Trash folder on
  Linux), and a status line confirms where the entry went.
- Showing the highlighted entry in the file manager (`o`) and copying its
  path (`y`), including over SSH via the terminal (OSC 52).
- Vim-style and arrow-key navigation, with jumps (`gg`/`G`, `Home`/`End`)
  and paging (`PgUp`/`PgDn`, `Ctrl+u`/`Ctrl+d`).
- A `?` popup listing every key, and key hints on each screen.
- A memory budget for the scanned tree (`--memory-budget`, default
  256 MiB): folders beyond it keep exact totals and load when opened,
  evicting the least recently visited ones.
- Builds and tests on Linux, macOS and Windows in CI.

[Unreleased]: https://github.com/nkien0204/duv/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/nkien0204/duv/releases/tag/v0.1.0
