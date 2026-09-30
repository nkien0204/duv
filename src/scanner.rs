//! Directory-size scanning and in-scan navigation.
//!
//! Given a root path (a disk's mount point, or any directory the user
//! opened), walks everything under it — recursing into subdirectories
//! with `rayon` so multiple branches are measured in parallel — and
//! records every file and directory in a [`Tree`] (`Scanner::tree`). Runs
//! on a background thread: each first-level child is walked into a
//! [`Subtree`], sent through an `mpsc` channel, and grafted into the tree
//! by `poll`, which `App::tick()` calls on every `Event::Tick` so the UI
//! thread is never blocked waiting on I/O.
//!
//! Once finished, drilling into a subdirectory and backing out
//! (`Scanner::enter_selected`/`Scanner::go_up`) just move around the tree,
//! with no further I/O. Only a directory whose contents aren't in the tree
//! (on another filesystem, or unreadable) needs a separate [`Scanner`]
//! rooted there — see `App::enter_selected` and `App::scanner_history`.
//!
//! Recursion never crosses filesystem boundaries (like `du -x`/
//! `--one-file-system`): a subdirectory that's actually the mount point of
//! a different filesystem contributes `0` rather than being summed in,
//! since its space doesn't belong to the filesystem being measured. This
//! matters in practice — e.g. on macOS, walking `/` would otherwise also
//! sum in `/System/Volumes/Data`, anything under `/Volumes`, network
//! mounts, etc., wildly inflating totals beyond the disk's actual size.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use rayon::iter::{IntoParallelIterator, ParallelBridge, ParallelIterator};

use crate::model::{Children, Node, NodeId, NodeKind, Subtree, Tree};

/// A directory-size scan in progress (or finished) on a background thread,
/// plus the user's position within the resulting tree.
pub struct Scanner {
    /// Everything measured so far under the scan root, as a tree.
    /// First-level children appear as their measurement arrives.
    pub tree: Tree,
    /// Number of first-level children being measured.
    pub total: usize,
    /// Number of first-level children measured so far.
    pub measured: usize,
    /// Whether every entry has been measured (or the scan thread has
    /// otherwise finished / disconnected).
    pub finished: bool,
    /// Index into the current directory's (size-sorted) entries of the
    /// highlighted row, once `finished`.
    pub selected: usize,
    /// Scroll/selection state for the results `Table`, kept here (rather
    /// than recreated each frame) so ratatui can track the scroll offset
    /// across renders and keep `selected` in view as the list scrolls.
    pub table_state: ratatui::widgets::TableState,
    /// The directory currently shown; starts at [`Tree::ROOT`].
    current: NodeId,
    /// Directories above `current` that were drilled through, with their
    /// selection and scroll state, innermost last.
    parents: Vec<SavedView>,
    rx: mpsc::Receiver<Subtree>,
}

/// A directory view to restore when going back up.
struct SavedView {
    dir: NodeId,
    selected: usize,
    table_state: ratatui::widgets::TableState,
}

/// Outcome of [`Scanner::enter_selected`].
#[derive(Debug, PartialEq, Eq)]
pub enum Enter {
    /// Moved into the selected directory, straight from the tree.
    Entered,
    /// The selected directory's contents aren't in the tree (another
    /// filesystem, or it couldn't be read); it needs its own scan.
    NeedsScan(PathBuf),
    /// Nothing to enter: the scan isn't finished, nothing is selected, or
    /// the selection is a file.
    Ignored,
}

impl Scanner {
    /// Spawns a background scan of `root`'s immediate children.
    ///
    /// Reading `root`'s own listing happens synchronously (it's a single,
    /// fast `read_dir` call) so the returned [`Scanner`] immediately knows
    /// `total`; the expensive recursive sizing of each child happens on a
    /// background thread pool.
    pub fn spawn(root: PathBuf) -> io::Result<Self> {
        let children = immediate_children(&root)?;
        let total = children.len();
        let root_dev = device_id(&root);
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            children.into_par_iter().for_each_with(tx, |tx, child| {
                let subtree = if child.is_dir {
                    dir_subtree(child.name, &child.path, root_dev)
                } else {
                    Subtree::file(child.name, child.size)
                };
                let _ = tx.send(subtree);
            });
        });

        let mut tree = Tree::new(root);
        tree.mark_loaded(Tree::ROOT);

        Ok(Self {
            tree,
            total,
            measured: 0,
            finished: total == 0,
            selected: 0,
            table_state: ratatui::widgets::TableState::default().with_selected(Some(0)),
            current: Tree::ROOT,
            parents: Vec::new(),
            rx,
        })
    }

    /// Drains any results the background thread has produced so far,
    /// grafting each into `tree`, and sorts the tree's children by size
    /// descending once every entry has arrived.
    ///
    /// Non-blocking: intended to be called periodically (e.g. once per
    /// `Event::Tick`) rather than awaited.
    pub fn poll(&mut self) {
        let was_finished = self.finished;
        loop {
            match self.rx.try_recv() {
                Ok(subtree) => {
                    self.tree.attach(Tree::ROOT, subtree);
                    self.measured += 1;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.finished = true;
                    break;
                }
            }
        }
        if self.measured >= self.total {
            self.finished = true;
        }
        // Sort once, on the tick the scan finishes, rather than every tick.
        if self.finished && !was_finished {
            self.tree.sort_children_by_size();
        }
    }

    /// Fraction of first-level children measured so far, in `0.0..=1.0`.
    pub fn progress_fraction(&self) -> f64 {
        if self.total == 0 {
            1.0
        } else {
            (self.measured as f64 / self.total as f64).min(1.0)
        }
    }

    /// The path this scan was started from.
    pub fn root(&self) -> &Path {
        self.tree.root_path()
    }

    /// The path of the directory currently shown.
    pub fn current_path(&self) -> PathBuf {
        self.tree
            .path(self.current)
            .unwrap_or_else(|| self.root().to_path_buf())
    }

    /// Entries of the directory currently shown, largest first once the
    /// scan is finished.
    pub fn entries(&self) -> impl Iterator<Item = &Node> {
        self.current_children()
            .iter()
            .filter_map(|&id| self.tree.get(id))
    }

    /// Number of entries in the directory currently shown.
    pub fn entry_count(&self) -> usize {
        self.current_children().len()
    }

    fn current_children(&self) -> &[NodeId] {
        self.tree.children(self.current).unwrap_or(&[])
    }

    /// Moves the selection to the next entry, wrapping at the end.
    pub fn select_next(&mut self) {
        let count = self.entry_count();
        if count == 0 {
            return;
        }
        self.selected = (self.selected + 1) % count;
        self.table_state.select(Some(self.selected));
    }

    /// Moves the selection to the previous entry, wrapping at the start.
    pub fn select_previous(&mut self) {
        let count = self.entry_count();
        if count == 0 {
            return;
        }
        self.selected = self.selected.checked_sub(1).unwrap_or(count - 1);
        self.table_state.select(Some(self.selected));
    }

    /// Drills into the selected entry if it's a directory whose contents
    /// are already in the tree, remembering the current view so
    /// [`Scanner::go_up`] can restore it.
    pub fn enter_selected(&mut self) -> Enter {
        if !self.finished {
            return Enter::Ignored;
        }
        let Some(&id) = self.current_children().get(self.selected) else {
            return Enter::Ignored;
        };
        let Some(node) = self.tree.get(id) else {
            return Enter::Ignored;
        };
        match &node.kind {
            NodeKind::File => Enter::Ignored,
            NodeKind::Dir(Children::Unloaded) => match self.tree.path(id) {
                Some(path) => Enter::NeedsScan(path),
                None => Enter::Ignored,
            },
            NodeKind::Dir(Children::Loaded(_)) => {
                let table_state = std::mem::replace(
                    &mut self.table_state,
                    ratatui::widgets::TableState::default().with_selected(Some(0)),
                );
                self.parents.push(SavedView {
                    dir: self.current,
                    selected: self.selected,
                    table_state,
                });
                self.current = id;
                self.selected = 0;
                Enter::Entered
            }
        }
    }

    /// Goes back to the parent directory within this scan, restoring its
    /// selection and scroll position. Returns `false` (and does nothing)
    /// if already at the scan root.
    pub fn go_up(&mut self) -> bool {
        let Some(view) = self.parents.pop() else {
            return false;
        };
        self.current = view.dir;
        self.selected = view.selected;
        self.table_state = view.table_state;
        true
    }
}

/// A first-level child discovered by the initial (synchronous) listing,
/// before its size has been computed.
struct PendingChild {
    name: String,
    path: PathBuf,
    is_dir: bool,
    /// Already-known size for plain files; ignored (and recomputed) for
    /// directories.
    size: u64,
}

fn immediate_children(root: &Path) -> io::Result<Vec<PendingChild>> {
    let mut children = Vec::new();
    for entry in fs::read_dir(root)? {
        let Ok(entry) = entry else { continue };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // Don't follow symlinks: avoids cycles and double-counting space
        // that's already accounted for elsewhere.
        if file_type.is_symlink() {
            continue;
        }
        let is_dir = file_type.is_dir();
        let size = if is_dir {
            0
        } else {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                entry.metadata().map(|m| m.blocks() * 512).unwrap_or(0)
            }
            #[cfg(not(unix))]
            {
                entry.metadata().map(|m| m.len()).unwrap_or(0)
            }
        };
        children.push(PendingChild {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: entry.path(),
            is_dir,
            size,
        });
    }
    Ok(children)
}

/// Recursively walks the directory at `path` into a [`Subtree`] named
/// `name`, parallelizing across subdirectories with `rayon`. Its size is
/// the sum of everything under it.
///
/// Skips entries it can't read (permission errors, races) and symlinks
/// (to avoid cycles/double counting) rather than failing the whole scan.
/// A directory that can't be listed at all becomes an unloaded,
/// zero-sized node, so opening it attempts a fresh scan that reports the
/// error. Also
/// stops at filesystem boundaries (like `du -x`/`--one-file-system`): a
/// subdirectory that's the mount point of a different filesystem than
/// `root_dev` becomes an unloaded, zero-sized node, since its space isn't
/// part of the disk being measured (this is what prevents e.g.
/// `/System/Volumes/Data`, `/Volumes/*`, or network mounts reachable from
/// `/` from being summed into the size of the root filesystem).
fn dir_subtree(name: String, path: &Path, root_dev: Option<u64>) -> Subtree {
    if crosses_filesystem_boundary(path, root_dev) {
        return Subtree::unloaded_dir(name, 0);
    }
    let Ok(entries) = fs::read_dir(path) else {
        return Subtree::unloaded_dir(name, 0);
    };
    let children = entries
        .par_bridge()
        .filter_map(Result::ok)
        .filter_map(|entry| entry_subtree(&entry, root_dev))
        .collect();
    Subtree::dir(name, children)
}

/// The [`Subtree`] for one directory entry, or `None` for entries that
/// are skipped (symlinks, and entries whose type can't be read).
fn entry_subtree(entry: &fs::DirEntry, root_dev: Option<u64>) -> Option<Subtree> {
    let file_type = entry.file_type().ok()?;
    if file_type.is_symlink() {
        return None;
    }
    let name = entry.file_name().to_string_lossy().into_owned();
    if file_type.is_dir() {
        Some(dir_subtree(name, &entry.path(), root_dev))
    } else {
        #[cfg(unix)]
        let size = {
            use std::os::unix::fs::MetadataExt;
            entry.metadata().map(|m| m.blocks() * 512).unwrap_or(0)
        };
        #[cfg(not(unix))]
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        Some(Subtree::file(name, size))
    }
}

/// The device/filesystem id of `path`, if it can be determined. `None` on
/// platforms where this isn't supported (currently: everything but Unix),
/// in which case filesystem-boundary checks are simply skipped.
#[cfg(unix)]
fn device_id(path: &Path) -> Option<u64> {
    fs::metadata(path).ok().map(|m| m.dev())
}

#[cfg(not(unix))]
fn device_id(_path: &Path) -> Option<u64> {
    None
}

/// Whether `path` lives on a different filesystem than `root_dev`.
/// Always `false` if `root_dev` is `None` (unknown / unsupported
/// platform), which preserves the old behavior there.
fn crosses_filesystem_boundary(path: &Path, root_dev: Option<u64>) -> bool {
    match root_dev {
        Some(root_dev) => device_id(path).is_some_and(|dev| dev != root_dev),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{Duration, Instant};

    fn write_file(path: &Path, bytes: &[u8]) {
        let mut f = fs::File::create(path).unwrap();
        f.write_all(bytes).unwrap();
    }

    fn wait_until_finished(scanner: &mut Scanner) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !scanner.finished && Instant::now() < deadline {
            scanner.poll();
            thread::sleep(Duration::from_millis(10));
        }
        assert!(scanner.finished, "scan did not finish in time");
    }

    #[test]
    fn builds_a_tree_of_everything_under_root() {
        let dir =
            std::env::temp_dir().join(format!("duv-scanner-tree-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("sub").join("deeper")).unwrap();
        fs::create_dir_all(dir.join("empty")).unwrap();
        write_file(&dir.join("top.txt"), b"hello");
        write_file(&dir.join("sub").join("a.txt"), b"world!");
        write_file(
            &dir.join("sub").join("deeper").join("c.txt"),
            &[0u8; 10_000],
        );

        let mut scanner = Scanner::spawn(dir.clone()).unwrap();
        wait_until_finished(&mut scanner);
        let tree = &scanner.tree;

        // root, top.txt, sub, empty, a.txt, deeper, c.txt
        assert_eq!(tree.node_count(), 7);

        // Root total matches the sum of the first-level entries.
        let entries_total: u64 = scanner.entries().map(|e| e.size).sum();
        assert_eq!(tree.get(Tree::ROOT).unwrap().size, entries_total);

        // Top-level entries are sorted largest first.
        let sizes: Vec<u64> = scanner.entries().map(|e| e.size).collect();
        assert!(sizes.windows(2).all(|pair| pair[0] >= pair[1]));
        assert_eq!(&*scanner.entries().next().unwrap().name, "sub");

        // Nested nodes are present, with paths rebuilt from the root.
        let find = |parent, name: &str| {
            tree.children(parent)
                .unwrap()
                .iter()
                .copied()
                .find(|&id| &*tree.get(id).unwrap().name == name)
                .unwrap()
        };
        let sub = find(Tree::ROOT, "sub");
        let deeper = find(sub, "deeper");
        let c = find(deeper, "c.txt");
        assert_eq!(
            tree.path(c).unwrap(),
            dir.join("sub").join("deeper").join("c.txt")
        );
        assert!(tree.get(c).unwrap().size >= 10_000);
        assert_eq!(tree.get(deeper).unwrap().size, tree.get(c).unwrap().size);

        // An empty directory is loaded with no children, not unloaded.
        let empty = find(Tree::ROOT, "empty");
        assert_eq!(tree.children(empty), Some(&[][..]));

        fs::remove_dir_all(&dir).ok();
    }

    fn select_by_name(scanner: &mut Scanner, name: &str) {
        let index = scanner.entries().position(|e| &*e.name == name).unwrap();
        while scanner.selected != index {
            scanner.select_next();
        }
    }

    fn entry_names(scanner: &Scanner) -> Vec<String> {
        scanner.entries().map(|e| e.name.to_string()).collect()
    }

    #[test]
    fn navigates_the_tree_without_rescanning() {
        let dir = std::env::temp_dir().join(format!("duv-scanner-nav-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("sub").join("deeper")).unwrap();
        write_file(&dir.join("top.txt"), b"hello");
        write_file(&dir.join("sub").join("a.txt"), b"world!");
        write_file(
            &dir.join("sub").join("deeper").join("c.txt"),
            &[0u8; 10_000],
        );

        let mut scanner = Scanner::spawn(dir.clone()).unwrap();
        wait_until_finished(&mut scanner);

        // Remove the files: navigating must still show them, proving it
        // reads from the tree rather than the filesystem.
        fs::remove_dir_all(&dir).unwrap();

        select_by_name(&mut scanner, "top.txt");
        assert_eq!(scanner.enter_selected(), Enter::Ignored);
        let top_selected = scanner.selected;

        select_by_name(&mut scanner, "sub");
        let sub_selected = scanner.selected;
        assert_eq!(scanner.enter_selected(), Enter::Entered);
        assert_eq!(scanner.current_path(), dir.join("sub"));
        assert_eq!(entry_names(&scanner), ["deeper", "a.txt"]);
        assert_eq!(scanner.selected, 0);

        assert_eq!(scanner.enter_selected(), Enter::Entered);
        assert_eq!(scanner.current_path(), dir.join("sub").join("deeper"));
        assert_eq!(entry_names(&scanner), ["c.txt"]);

        assert!(scanner.go_up());
        assert_eq!(scanner.current_path(), dir.join("sub"));
        assert!(scanner.go_up());
        assert_eq!(scanner.current_path(), dir);
        assert_eq!(scanner.selected, sub_selected);
        assert_ne!(scanner.selected, top_selected);
        assert!(!scanner.go_up());
    }

    #[test]
    fn enter_is_ignored_while_scanning() {
        let dir =
            std::env::temp_dir().join(format!("duv-scanner-busy-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("sub")).unwrap();

        let mut scanner = Scanner::spawn(dir.clone()).unwrap();
        assert!(!scanner.finished);
        assert_eq!(scanner.enter_selected(), Enter::Ignored);

        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_directory_needs_its_own_scan() {
        use std::os::unix::fs::PermissionsExt;

        let dir =
            std::env::temp_dir().join(format!("duv-scanner-locked-test-{}", std::process::id()));
        let locked = dir.join("locked");
        fs::create_dir_all(&locked).unwrap();
        write_file(&locked.join("secret.txt"), b"x");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        // Running as root can read it anyway; nothing to test then.
        if fs::read_dir(&locked).is_err() {
            let mut scanner = Scanner::spawn(dir.clone()).unwrap();
            wait_until_finished(&mut scanner);
            select_by_name(&mut scanner, "locked");
            assert_eq!(scanner.enter_selected(), Enter::NeedsScan(locked.clone()));
        }

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scans_first_level_children_with_correct_sizes() {
        let dir = std::env::temp_dir().join(format!("duv-scanner-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("sub")).unwrap();
        write_file(&dir.join("top.txt"), b"hello"); // 5 bytes
        write_file(&dir.join("sub").join("a.txt"), b"world!"); // 6 bytes
        write_file(&dir.join("sub").join("b.txt"), b"!!"); // 2 bytes

        let mut scanner = Scanner::spawn(dir.clone()).unwrap();
        assert_eq!(scanner.total, 2); // "top.txt" and "sub"

        wait_until_finished(&mut scanner);

        let file_entry = scanner.entries().find(|e| &*e.name == "top.txt").unwrap();
        assert!(file_entry.size >= 5);
        assert!(!file_entry.is_dir());

        let dir_entry = scanner.entries().find(|e| &*e.name == "sub").unwrap();
        assert!(dir_entry.size >= 8); // 6 + 2
        assert!(dir_entry.is_dir());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn handles_sparse_files_correctly() {
        let dir = std::env::temp_dir().join(format!("duv-sparse-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        let file_path = dir.join("sparse.txt");
        let mut file = fs::File::create(&file_path).unwrap();

        // Create a "1 GiB" sparse file by seeking to 1GiB and writing 1 byte
        use std::io::{Seek, SeekFrom, Write};
        file.seek(SeekFrom::Start(1024 * 1024 * 1024)).unwrap();
        file.write_all(b"a").unwrap();

        let mut scanner = Scanner::spawn(dir.clone()).unwrap();
        wait_until_finished(&mut scanner);

        let entry = scanner
            .entries()
            .find(|e| &*e.name == "sparse.txt")
            .expect("Sparse file not found in scan");

        // The logical size is > 1GiB, but the physical size should be very small (usually a few KiB)
        assert!(
            entry.size < 1024 * 1024,
            "Sparse file reported as too large: {} bytes (expected < 1MiB)",
            entry.size
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn same_filesystem_paths_share_a_device_id() {
        let dir =
            std::env::temp_dir().join(format!("duv-scanner-devid-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        write_file(&dir.join("a.txt"), b"a");

        let dir_dev = device_id(&dir).unwrap();
        let file_dev = device_id(&dir.join("a.txt")).unwrap();
        assert_eq!(dir_dev, file_dev);
        assert!(!crosses_filesystem_boundary(&dir, Some(dir_dev)));

        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn crosses_filesystem_boundary_flags_a_mismatched_device() {
        let dir =
            std::env::temp_dir().join(format!("duv-scanner-devid-test2-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        let real_dev = device_id(&dir).unwrap();
        let fake_dev = real_dev.wrapping_add(1);
        assert!(crosses_filesystem_boundary(&dir, Some(fake_dev)));

        fs::remove_dir_all(&dir).ok();
    }
}
