//! Application state (the "Model" in this crate's Model-Update-View split).
//!
//! `App` holds all state and has no dependency on ratatui's rendering types
//! or crossterm's event types — it doesn't know how it's drawn or how input
//! arrives. See `update.rs` for how input mutates this state and `ui.rs` for
//! how it's rendered. See `../ARCHITECTURE.md` for the full picture.

use std::path::PathBuf;

use crate::disks::{self, DiskInfo};
use crate::scanner::Scanner;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitOption {
    Yes,
    No,
}

pub struct App {
    pub should_quit: bool,
    /// Whether to show the quit confirmation popup, and which option is selected.
    pub quit_confirmation: Option<QuitOption>,
    /// Every mounted disk/volume visible to the OS, as of the last refresh.
    pub disks: Vec<DiskInfo>,
    /// Index into `disks` of the currently highlighted entry.
    pub selected: usize,
    /// Scroll/selection state for the disk-list table, kept here (rather
    /// than recreated each frame) so ratatui can track the scroll offset
    /// across renders and keep `selected` in view as the list scrolls.
    pub disks_table_state: ratatui::widgets::TableState,
    /// The currently displayed directory scan (in progress or finished).
    /// `None` means the disk list is showing. A [`Scanner`] only ever
    /// measures one level of a directory tree; drilling into a
    /// subdirectory (`enter_selected`) spawns a fresh one rooted there.
    pub scanner: Option<Scanner>,
    /// Completed parent scans to restore when backing out (`go_back`),
    /// most-recently-entered last. Together with `scanner`, this forms the
    /// drill-down navigation stack: entering a directory pushes the
    /// current scanner here instead of discarding it, so going back up
    /// doesn't require re-scanning.
    pub scanner_history: Vec<Scanner>,
    /// Set if the most recent scan attempt failed to even list its root
    /// directory (e.g. a permissions error), so the UI can surface it.
    pub scanner_error: Option<String>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            should_quit: false,
            quit_confirmation: None,
            disks: disks::list(),
            selected: 0,
            disks_table_state: ratatui::widgets::TableState::default().with_selected(Some(0)),
            scanner: None,
            scanner_history: Vec::new(),
            scanner_error: None,
        }
    }
}

impl App {
    /// Constructs a new instance of [`App`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Handles the tick event of the terminal: polls the active scan (if
    /// any) for newly completed results.
    pub fn tick(&mut self) {
        if let Some(scanner) = &mut self.scanner {
            scanner.poll();
        }
    }

    /// Set should_quit to true to quit the application.
    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    /// Moves the selection to the next disk, wrapping at the end.
    pub fn select_next(&mut self) {
        if self.disks.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.disks.len();
        self.disks_table_state.select(Some(self.selected));
    }

    /// Moves the selection to the previous disk, wrapping at the start.
    pub fn select_previous(&mut self) {
        if self.disks.is_empty() {
            return;
        }
        self.selected = self.selected.checked_sub(1).unwrap_or(self.disks.len() - 1);
        self.disks_table_state.select(Some(self.selected));
    }

    /// Returns the currently selected disk, if any.
    pub fn selected_disk(&self) -> Option<&DiskInfo> {
        self.disks.get(self.selected)
    }

    /// Starts scanning: from the disk list, scans the selected disk's
    /// mount point and resets the navigation stack; from within a scan
    /// view, re-scans the *current* directory in place (refresh),
    /// preserving the existing navigation history.
    pub fn start_scan(&mut self) {
        let root = match &self.scanner {
            Some(scanner) => scanner.root.clone(),
            None => {
                let Some(mount_point) = self.selected_disk().map(|disk| disk.mount_point.clone())
                else {
                    return;
                };
                self.scanner_history.clear();
                PathBuf::from(mount_point)
            }
        };
        self.spawn_scan(root);
    }

    /// Drills into the selected directory entry of the current scan,
    /// pushing the current view onto the navigation history and starting
    /// a fresh scan of that subdirectory. No-op unless the current scan
    /// is finished and its selected entry is a directory.
    pub fn enter_selected(&mut self) {
        let Some(scanner) = &self.scanner else {
            return;
        };
        if !scanner.finished {
            return;
        }
        let Some(entry) = scanner.selected_entry() else {
            return;
        };
        if !entry.is_dir {
            return;
        }
        let path = entry.path.clone();
        if let Some(current) = self.scanner.take() {
            self.scanner_history.push(current);
        }
        self.spawn_scan(path);
    }

    /// Goes back to the parent directory's scan, restoring it from history
    /// without re-scanning — or all the way back to the disk list if
    /// there's no parent to return to.
    pub fn go_back(&mut self) {
        self.scanner = self.scanner_history.pop();
        self.scanner_error = None;
    }

    /// Spawns a scan of `root`, replacing the current scanner (on success)
    /// or surfacing an error (on failure).
    fn spawn_scan(&mut self, root: PathBuf) {
        match Scanner::spawn(root) {
            Ok(scanner) => {
                self.scanner = Some(scanner);
                self.scanner_error = None;
            }
            Err(err) => {
                self.scanner = None;
                self.scanner_error = Some(format!("failed to scan: {err}"));
            }
        }
    }
}
