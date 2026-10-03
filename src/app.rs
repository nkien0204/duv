//! Application state (the "Model" in this crate's Model-Update-View split).
//!
//! `App` holds all state and has no dependency on ratatui's rendering types
//! or crossterm's event types — it doesn't know how it's drawn or how input
//! arrives. See `update.rs` for how input mutates this state and `ui.rs` for
//! how it's rendered. See `../ARCHITECTURE.md` for the full picture.

use std::path::{Path, PathBuf};

use crate::desktop;
use crate::disks::{self, DiskInfo};
use crate::scanner::{self, Enter, Scanner};

/// The highlighted button in a Yes/No confirmation popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Yes,
    No,
}

/// An entry the user asked to move to the Trash, awaiting confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteRequest {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub choice: Choice,
}

/// A jump of the highlighted row in the list being shown. Unlike stepping
/// with `j`/`k`, jumps stop at either end instead of wrapping around.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jump {
    First,
    Last,
    PageUp,
    PageDown,
    HalfPageUp,
    HalfPageDown,
}

/// Moves a file or directory to the Trash (or the platform equivalent).
pub type TrashFn = fn(&Path) -> Result<(), String>;

/// Shows a path in the platform's file manager.
pub type RevealFn = fn(&Path) -> Result<(), String>;

/// Copies text to the clipboard.
pub type CopyFn = fn(&str) -> Result<(), String>;

pub struct App {
    pub should_quit: bool,
    /// Whether to show the quit confirmation popup, and which option is selected.
    pub quit_confirmation: Option<Choice>,
    /// The entry awaiting confirmation to be moved to the Trash, if any.
    pub delete_confirmation: Option<DeleteRequest>,
    /// A message to show in a popup until the next key press (e.g. a
    /// failed delete).
    pub notice: Option<String>,
    /// Whether the key help popup (`?`) is open.
    pub show_help: bool,
    /// A short confirmation shown in place of the key hints until the next
    /// key press (e.g. after copying a path).
    pub status: Option<String>,
    /// How entries are moved to the Trash, shown in the file manager, and
    /// copied. Swappable so tests never touch the real desktop.
    pub trash: TrashFn,
    pub reveal: RevealFn,
    pub copy: CopyFn,
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
    /// Number of list rows visible on screen, as of the last render. Sets
    /// how far page jumps move.
    pub page_size: usize,
    /// Whether the user is typing a name filter (after `/`); keys then edit
    /// the query instead of navigating.
    pub filter_input: bool,
    /// Whether the previous key was a lone `g`, so a second `g` jumps to
    /// the first row (vim's `gg`).
    pub pending_g: bool,
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
            delete_confirmation: None,
            notice: None,
            show_help: false,
            status: None,
            trash: desktop::move_to_trash,
            reveal: desktop::reveal,
            copy: desktop::copy_to_clipboard,
            disks: disks::list(),
            selected: 0,
            disks_table_state: ratatui::widgets::TableState::default().with_selected(Some(0)),
            scanner: None,
            scanner_history: Vec::new(),
            page_size: 10,
            filter_input: false,
            pending_g: false,
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

    /// Moves the highlighted row of the list being shown — the current
    /// directory's entries once a scan has finished, or the disk list —
    /// by `jump`. No-op while scanning or on an empty list.
    pub fn jump(&mut self, jump: Jump) {
        let page = self.page_size.max(1);
        let half = (page / 2).max(1);
        let target = |selected: usize, count: usize| -> Option<usize> {
            let last = count.checked_sub(1)?;
            Some(match jump {
                Jump::First => 0,
                Jump::Last => last,
                Jump::PageUp => selected.saturating_sub(page),
                Jump::PageDown => (selected + page).min(last),
                Jump::HalfPageUp => selected.saturating_sub(half),
                Jump::HalfPageDown => (selected + half).min(last),
            })
        };
        match &mut self.scanner {
            Some(scanner) if scanner.finished => {
                if let Some(index) = target(scanner.selected, scanner.entry_count()) {
                    scanner.select(index);
                }
            }
            Some(_) => {}
            None => {
                if let Some(index) = target(self.selected, self.disks.len()) {
                    self.selected = index;
                    self.disks_table_state.select(Some(index));
                }
            }
        }
    }

    /// Starts typing a name filter for the current directory of a finished
    /// scan, keeping any existing query to edit. No-op otherwise.
    pub fn start_filter(&mut self) {
        let Some(scanner) = self.scanner.as_mut().filter(|s| s.finished) else {
            return;
        };
        if scanner.filter_query().is_none() {
            scanner.set_filter("");
        }
        self.filter_input = true;
    }

    /// Appends `c` to the filter being typed.
    pub fn filter_push(&mut self, c: char) {
        self.edit_filter(|query| query.push(c));
    }

    /// Deletes the last character of the filter being typed, or cancels the
    /// filter if it's already empty.
    pub fn filter_pop(&mut self) {
        let empty = self
            .scanner
            .as_ref()
            .and_then(Scanner::filter_query)
            .is_none_or(str::is_empty);
        if empty {
            self.cancel_filter();
        } else {
            self.edit_filter(|query| {
                query.pop();
            });
        }
    }

    /// Stops typing and keeps the filter (an empty one is dropped).
    pub fn accept_filter(&mut self) {
        self.filter_input = false;
        if let Some(scanner) = &mut self.scanner
            && scanner.filter_query() == Some("")
        {
            scanner.clear_filter();
        }
    }

    /// Stops typing and removes the filter.
    pub fn cancel_filter(&mut self) {
        self.filter_input = false;
        if let Some(scanner) = &mut self.scanner {
            scanner.clear_filter();
        }
    }

    fn edit_filter(&mut self, edit: impl FnOnce(&mut String)) {
        let Some(scanner) = &mut self.scanner else {
            return;
        };
        let mut query = scanner.filter_query().unwrap_or_default().to_string();
        edit(&mut query);
        scanner.set_filter(&query);
    }

    /// The path of the highlighted row: an entry of a finished scan, or a
    /// disk's mount point on the disk list. `None` while scanning or with
    /// nothing selected.
    fn selected_path(&self) -> Option<PathBuf> {
        match &self.scanner {
            Some(scanner) => scanner.selected_entry().map(|(_, path)| path),
            None => self
                .selected_disk()
                .map(|disk| PathBuf::from(&disk.mount_point)),
        }
    }

    /// Shows the highlighted row in the file manager.
    pub fn reveal_selected(&mut self) {
        let Some(path) = self.selected_path() else {
            return;
        };
        match (self.reveal)(&path) {
            // Only the launch is known to have worked, hence "Opening".
            Ok(()) => self.status = Some(format!("Opening {} in the file manager", path.display())),
            Err(err) => {
                self.notice = Some(format!(
                    "Couldn't show {} in the file manager: {err}",
                    path.display()
                ));
            }
        }
    }

    /// Copies the highlighted row's full path to the clipboard.
    pub fn copy_selected_path(&mut self) {
        let Some(path) = self.selected_path() else {
            return;
        };
        let text = path.to_string_lossy();
        match (self.copy)(&text) {
            Ok(()) => self.status = Some(format!("Copied {text}")),
            Err(err) => self.notice = Some(format!("Couldn't copy the path: {err}")),
        }
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

    /// Asks for confirmation to move the highlighted entry of a finished
    /// scan to the Trash. No-op on the disk list, while scanning, or with
    /// nothing selected.
    pub fn request_delete(&mut self) {
        let Some((node, path)) = self.scanner.as_ref().and_then(Scanner::selected_entry) else {
            return;
        };
        self.delete_confirmation = Some(DeleteRequest {
            name: node.name.to_string(),
            size: node.size,
            is_dir: node.is_dir(),
            path,
            choice: Choice::No,
        });
    }

    /// Acts on the delete confirmation popup: with `Choice::Yes`, moves the
    /// entry to the Trash and removes it from every scan's tree (so totals
    /// stay right when backing out); on failure, shows a notice instead.
    /// Either way the popup closes.
    pub fn confirm_delete(&mut self) {
        let Some(request) = self.delete_confirmation.take() else {
            return;
        };
        if request.choice != Choice::Yes {
            return;
        }
        match (self.trash)(&request.path) {
            Ok(()) => {
                for scanner in self.scanner.iter_mut().chain(&mut self.scanner_history) {
                    scanner.forget(&request.path, request.size);
                }
            }
            Err(err) => {
                self.notice = Some(format!(
                    "Couldn't move {} to the Trash: {err}",
                    request.path.display()
                ));
            }
        }
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

    /// Stands in for the real Trash in tests: deletes the path for real
    /// (it's always inside a temp directory).
    fn fake_trash(path: &Path) -> Result<(), String> {
        let result = if path.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        result.map_err(|err| err.to_string())
    }

    fn failing_trash(_: &Path) -> Result<(), String> {
        Err("permission denied".to_string())
    }

    fn root_size(app: &App) -> u64 {
        let tree = &app.scanner.as_ref().unwrap().tree;
        tree.get(crate::model::Tree::ROOT).unwrap().size
    }

    fn entry_names(app: &App) -> Vec<String> {
        app.scanner
            .as_ref()
            .unwrap()
            .entries()
            .map(|e| e.name.to_string())
            .collect()
    }

    /// root/
    /// ├── big.bin (100 000 bytes)
    /// └── small.txt
    fn delete_test_app(name: &str) -> (App, PathBuf) {
        let dir = std::env::temp_dir().join(format!("duv-app-{name}-{}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("big.bin"), [1u8; 100_000]).unwrap();
        fs::write(dir.join("small.txt"), b"hi").unwrap();

        let mut app = App::with_start_path(dir.clone(), scanner::DEFAULT_MEMORY_BUDGET);
        app.trash = fake_trash;
        wait_until_finished(&mut app);
        (app, dir)
    }

    #[test]
    fn delete_asks_first_and_defaults_to_no() {
        let (mut app, dir) = delete_test_app("delete-no");

        app.request_delete();
        let request = app.delete_confirmation.clone().unwrap();
        assert_eq!(request.path, dir.join("big.bin"));
        assert_eq!(request.choice, Choice::No);
        assert!(!request.is_dir);

        app.confirm_delete();
        assert!(app.delete_confirmation.is_none());
        assert!(dir.join("big.bin").exists(), "No keeps the file");
        assert_eq!(entry_names(&app), ["big.bin", "small.txt"]);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn confirmed_delete_trashes_the_entry_and_updates_totals() {
        let (mut app, dir) = delete_test_app("delete-yes");
        let small = app
            .scanner
            .as_ref()
            .unwrap()
            .entries()
            .find(|e| &*e.name == "small.txt")
            .unwrap()
            .size;

        app.request_delete();
        app.delete_confirmation.as_mut().unwrap().choice = Choice::Yes;
        app.confirm_delete();

        assert!(!dir.join("big.bin").exists());
        assert_eq!(entry_names(&app), ["small.txt"]);
        assert_eq!(root_size(&app), small);
        assert_eq!(app.scanner.as_ref().unwrap().selected, 0);
        assert!(app.notice.is_none());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn failed_delete_shows_a_notice_and_keeps_the_entry() {
        let (mut app, dir) = delete_test_app("delete-fail");
        app.trash = failing_trash;
        let before = root_size(&app);

        app.request_delete();
        app.delete_confirmation.as_mut().unwrap().choice = Choice::Yes;
        app.confirm_delete();

        assert!(app.notice.as_ref().unwrap().contains("permission denied"));
        assert_eq!(entry_names(&app), ["big.bin", "small.txt"]);
        assert_eq!(root_size(&app), before);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_is_ignored_on_the_disk_list_and_while_scanning() {
        let mut app = App::new(scanner::DEFAULT_MEMORY_BUDGET);
        app.request_delete();
        assert!(app.delete_confirmation.is_none());

        let dir = std::env::temp_dir().join(format!("duv-app-busy-{}", std::process::id()));
        fs::create_dir_all(dir.join("sub")).unwrap();
        let mut app = App::with_start_path(dir.clone(), scanner::DEFAULT_MEMORY_BUDGET);
        assert!(!app.scanner.as_ref().unwrap().finished);
        app.request_delete();
        assert!(app.delete_confirmation.is_none());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_also_updates_earlier_scans_in_history() {
        let dir =
            std::env::temp_dir().join(format!("duv-app-delete-history-{}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub").join("a.bin"), [1u8; 50_000]).unwrap();

        let mut app = App::with_start_path(dir.clone(), scanner::DEFAULT_MEMORY_BUDGET);
        app.trash = fake_trash;
        wait_until_finished(&mut app);

        // Open `sub` and rescan it, so the first scan moves to history.
        app.enter_selected();
        app.start_scan();
        wait_until_finished(&mut app);
        assert_eq!(app.scanner_history.len(), 1);

        app.request_delete();
        app.delete_confirmation.as_mut().unwrap().choice = Choice::Yes;
        app.confirm_delete();
        assert!(entry_names(&app).is_empty());

        // Back in the first scan, `sub` and the root no longer count it.
        app.go_back();
        assert_eq!(current_path(&app), dir);
        assert_eq!(root_size(&app), 0);
        let sub = app.scanner.as_ref().unwrap().entries().next().unwrap();
        assert_eq!(sub.size, 0);

        fs::remove_dir_all(&dir).ok();
    }

    thread_local! {
        /// What the fake reveal/copy functions were called with, per test
        /// thread (each test runs on its own thread).
        static CALLS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    fn calls() -> Vec<String> {
        CALLS.with(|calls| calls.borrow().clone())
    }

    fn fake_reveal(path: &Path) -> Result<(), String> {
        CALLS.with(|calls| {
            calls
                .borrow_mut()
                .push(format!("reveal {}", path.display()))
        });
        Ok(())
    }

    fn fake_copy(text: &str) -> Result<(), String> {
        CALLS.with(|calls| calls.borrow_mut().push(format!("copy {text}")));
        Ok(())
    }

    fn failing_desktop(_: &str) -> Result<(), String> {
        Err("no clipboard tool".to_string())
    }

    #[test]
    fn copy_and_reveal_act_on_the_highlighted_entry() {
        let (mut app, dir) = delete_test_app("copy-reveal");
        app.reveal = fake_reveal;
        app.copy = fake_copy;

        app.copy_selected_path();
        app.reveal_selected();
        let big = dir.join("big.bin");
        assert_eq!(
            calls(),
            [
                format!("copy {}", big.display()),
                format!("reveal {}", big.display())
            ]
        );
        assert!(app.status.as_ref().unwrap().contains("file manager"));
        assert!(app.notice.is_none());

        fs::remove_dir_all(&dir).ok();
    }

    fn no_desktop(_: &Path) -> Result<(), String> {
        Err(
            "there's no desktop here (this is an SSH session). Press y to copy the path instead."
                .to_string(),
        )
    }

    #[test]
    fn reveal_without_a_desktop_explains_why() {
        let (mut app, dir) = delete_test_app("reveal-headless");
        app.reveal = no_desktop;
        app.reveal_selected();

        assert!(app.status.is_none());
        let notice = app.notice.as_ref().unwrap();
        assert!(notice.contains(&dir.join("big.bin").display().to_string()));
        assert!(notice.contains("no desktop here"));
        assert!(notice.contains("Press y"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn copy_reports_success_and_failure() {
        let (mut app, dir) = delete_test_app("copy-status");
        app.copy = fake_copy;
        app.copy_selected_path();
        assert_eq!(
            app.status.as_deref(),
            Some(format!("Copied {}", dir.join("big.bin").display()).as_str())
        );

        app.status = None;
        app.copy = failing_desktop;
        app.copy_selected_path();
        assert!(app.status.is_none());
        assert!(app.notice.as_ref().unwrap().contains("no clipboard tool"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn copy_on_the_disk_list_uses_the_mount_point() {
        let mut app = App::new(scanner::DEFAULT_MEMORY_BUDGET);
        app.copy = fake_copy;
        app.disks = vec![DiskInfo {
            name: "Data".to_string(),
            mount_point: "/mnt/data".to_string(),
            file_system: "ext4".to_string(),
            total_space: 100,
            available_space: 50,
            is_removable: false,
            kind: disks::DiskKind::Ssd,
        }];
        app.copy_selected_path();
        assert_eq!(calls(), ["copy /mnt/data"]);
    }

    #[test]
    fn copy_and_reveal_are_ignored_while_scanning() {
        let dir = std::env::temp_dir().join(format!("duv-app-copy-busy-{}", std::process::id()));
        fs::create_dir_all(dir.join("sub")).unwrap();
        let mut app = App::with_start_path(dir.clone(), scanner::DEFAULT_MEMORY_BUDGET);
        app.reveal = fake_reveal;
        app.copy = fake_copy;
        assert!(!app.scanner.as_ref().unwrap().finished);

        app.copy_selected_path();
        app.reveal_selected();
        assert!(calls().is_empty());

        fs::remove_dir_all(&dir).ok();
    }
}
