//! Application state (the "Model" in this crate's Model-Update-View split).
//!
//! `App` holds all state and has no dependency on ratatui's rendering types
//! or crossterm's event types — it doesn't know how it's drawn or how input
//! arrives. See `update.rs` for how input mutates this state and `ui.rs` for
//! how it's rendered. See `../ARCHITECTURE.md` for the full picture.

use std::path::PathBuf;

use crate::disks::{self, DiskInfo};
use crate::scanner::{self, Enter, Scanner};

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
    /// `None` means the disk list is showing. A [`Scanner`] holds the
    /// whole tree under its root, so drilling into subdirectories and
    /// back out happens inside it without rescanning.
    pub scanner: Option<Scanner>,
    /// Earlier scans to restore when backing out (`go_back`) past the
    /// current scanner's root, most recent last. A new scanner is only
    /// pushed on top when the tree can't answer: opening a directory
    /// whose contents weren't collected (another filesystem, or
    /// unreadable), or rescanning a subdirectory with `s`.
    pub scanner_history: Vec<Scanner>,
    /// Memory budget for each scan's tree, in bytes (see `scanner.rs`).
    pub memory_budget: usize,
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
            memory_budget: scanner::DEFAULT_MEMORY_BUDGET,
            scanner_error: None,
        }
    }
}

impl App {
    /// Constructs an [`App`] showing the disk list, whose scans keep their
    /// trees within `memory_budget` bytes.
    pub fn new(memory_budget: usize) -> Self {
        Self {
            memory_budget,
            ..Self::default()
        }
    }

    /// Constructs an [`App`] that immediately scans `root` instead of
    /// showing the disk list. Backing out of that scan returns to the
    /// disk list.
    pub fn with_start_path(root: PathBuf, memory_budget: usize) -> Self {
        let mut app = Self::new(memory_budget);
        app.spawn_scan(root);
        app
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
    /// view, re-scans the *current* directory (refresh), preserving the
    /// existing navigation history.
    pub fn start_scan(&mut self) {
        let root = match &mut self.scanner {
            Some(scanner) => {
                let root = scanner.current_path();
                // Refreshing a subdirectory: keep the rest of this scan
                // (moved up to the parent) so backing out still works.
                if scanner.go_up()
                    && let Some(parent) = self.scanner.take()
                {
                    self.scanner_history.push(parent);
                }
                root
            }
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

    /// Drills into the selected directory entry of the current scan. Its
    /// contents normally come straight from the scanned tree; if they
    /// weren't collected, the current scan is pushed onto the history and
    /// a fresh scan of that directory starts. No-op unless the current
    /// scan is finished and its selected entry is a directory.
    pub fn enter_selected(&mut self) {
        let Some(scanner) = &mut self.scanner else {
            return;
        };
        if let Enter::NeedsScan(path) = scanner.enter_selected() {
            if let Some(current) = self.scanner.take() {
                self.scanner_history.push(current);
            }
            self.spawn_scan(path);
        }
    }

    /// Goes back to the parent directory: within the current scan's tree
    /// if possible, otherwise to the previous scan in history (without
    /// re-scanning), or all the way back to the disk list if there's
    /// nothing left.
    pub fn go_back(&mut self) {
        if self.scanner_error.is_none()
            && let Some(scanner) = &mut self.scanner
            && scanner.go_up()
        {
            return;
        }
        self.scanner = self.scanner_history.pop();
        self.scanner_error = None;
    }

    /// Spawns a scan of `root`, replacing the current scanner (on success)
    /// or surfacing an error (on failure).
    fn spawn_scan(&mut self, root: PathBuf) {
        match Scanner::spawn(root, self.memory_budget) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, thread, time::Duration, time::Instant};

    fn wait_until_finished(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app.scanner.as_ref().unwrap().finished && Instant::now() < deadline {
            app.tick();
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            app.scanner.as_ref().unwrap().finished,
            "scan did not finish in time"
        );
    }

    fn current_path(app: &App) -> PathBuf {
        app.scanner.as_ref().unwrap().current_path()
    }

    #[test]
    fn drill_down_and_back_stay_within_one_scan() {
        let dir = std::env::temp_dir().join(format!("duv-app-nav-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("sub").join("deeper")).unwrap();
        fs::write(dir.join("sub").join("deeper").join("c.txt"), [0u8; 10_000]).unwrap();

        let mut app = App::with_start_path(dir.clone(), scanner::DEFAULT_MEMORY_BUDGET);
        wait_until_finished(&mut app);

        app.enter_selected();
        app.enter_selected();
        assert_eq!(current_path(&app), dir.join("sub").join("deeper"));
        assert!(app.scanner_history.is_empty(), "no extra scans started");

        app.go_back();
        assert_eq!(current_path(&app), dir.join("sub"));
        app.go_back();
        assert_eq!(current_path(&app), dir);
        app.go_back();
        assert!(
            app.scanner.is_none(),
            "back past the root shows the disk list"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rescanning_a_subdirectory_keeps_the_way_back() {
        let dir = std::env::temp_dir().join(format!("duv-app-rescan-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub").join("a.txt"), b"a").unwrap();

        let mut app = App::with_start_path(dir.clone(), scanner::DEFAULT_MEMORY_BUDGET);
        wait_until_finished(&mut app);
        app.enter_selected();
        assert_eq!(current_path(&app), dir.join("sub"));

        app.start_scan();
        assert_eq!(current_path(&app), dir.join("sub"));
        assert_eq!(app.scanner_history.len(), 1);

        wait_until_finished(&mut app);
        app.go_back();
        assert_eq!(current_path(&app), dir);
        app.go_back();
        assert!(app.scanner.is_none());

        fs::remove_dir_all(&dir).ok();
    }
}
