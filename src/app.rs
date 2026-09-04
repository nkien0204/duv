//! Application state (the "Model" in this crate's Model-Update-View split).
//!
//! `App` holds all state and has no dependency on ratatui's rendering types
//! or crossterm's event types — it doesn't know how it's drawn or how input
//! arrives. See `update.rs` for how input mutates this state and `ui.rs` for
//! how it's rendered. See `../ARCHITECTURE.md` for the full picture.

use crate::disks::{self, DiskInfo};

pub struct App {
    pub should_quit: bool,
    /// Every mounted disk/volume visible to the OS, as of the last refresh.
    pub disks: Vec<DiskInfo>,
    /// Index into `disks` of the currently highlighted entry.
    pub selected: usize,
}

impl Default for App {
    fn default() -> Self {
        Self {
            should_quit: false,
            disks: disks::list(),
            selected: 0,
        }
    }
}

impl App {
    /// Constructs a new instance of [`App`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Handles the tick event of the terminal.
    pub fn tick(&self) {}

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
    }

    /// Moves the selection to the previous disk, wrapping at the start.
    pub fn select_previous(&mut self) {
        if self.disks.is_empty() {
            return;
        }
        self.selected = self.selected.checked_sub(1).unwrap_or(self.disks.len() - 1);
    }

    /// Returns the currently selected disk, if any.
    pub fn selected_disk(&self) -> Option<&DiskInfo> {
        self.disks.get(self.selected)
    }
}
