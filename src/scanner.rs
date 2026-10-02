//! Directory-size scanning and in-scan navigation.
//!
//! Given a root path (a disk's mount point, or any directory the user
//! opened), walks everything under it — recursing into subdirectories
//! with `rayon` so multiple branches are measured in parallel — and
//! records what it finds in a [`Tree`] (`Scanner::tree`). The walk runs on
//! a background thread and sends one message per directory listed (its
//! direct entries) through an `mpsc` channel; `poll`, called by
//! `App::tick()` on every `Event::Tick`, adds them to the tree, so the UI
//! thread is never blocked waiting on I/O and sizes fill in live.
//!
//! **Memory budget.** The tree is kept within `Scanner::budget` bytes (as
//! estimated by [`Tree::approx_bytes`]):
//!
//! - While scanning, once the tree is over budget, a newly listed
//!   directory's entries aren't stored: only their total is added to the
//!   directory, which stays [`Children::Unloaded`]. Totals stay exact.
//! - Opening an unloaded directory scans it into the same tree ("loading
//!   in place"). Before that, if the tree is above three quarters of the
//!   budget, the entries of directories off the current path are evicted,
//!   least recently visited first (a visit to a directory counts as a
//!   visit to all of its ancestors), so the load has room.
//!
//! Once a scan finishes, drilling into loaded subdirectories and backing
//! out (`Scanner::enter_selected`/`Scanner::go_up`) just move around the
//! tree, with no further I/O. Directories on another filesystem, and ones
//! that can't be read, need a separate [`Scanner`] rooted there — see
//! `App::enter_selected` and `App::scanner_history`.
//!
//! A file with several hard links is counted once, like `du` does: the
//! first link seen in a scan counts its full size, the others count `0`
//! (which link is "first" depends on thread timing).
//!
//! Recursion never crosses filesystem boundaries (like `du -x`/
//! `--one-file-system`): a subdirectory that's actually the mount point of
//! a different filesystem contributes `0` rather than being summed in,
//! since its space doesn't belong to the filesystem being measured. This
//! matters in practice — e.g. on macOS, walking `/` would otherwise also
//! sum in `/System/Volumes/Data`, anything under `/Volumes`, network
//! mounts, etc., wildly inflating totals beyond the disk's actual size.

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use rayon::iter::{IntoParallelIterator, ParallelBridge, ParallelIterator};

use crate::model::{Children, Node, NodeId, NodeKind, Tree};

/// Default memory budget for a scan's tree, in bytes of
/// [`Tree::approx_bytes`].
pub const DEFAULT_MEMORY_BUDGET: usize = 256 * 1024 * 1024;

/// A directory-size scan in progress (or finished) on a background thread,
/// plus the user's position within the resulting tree.
pub struct Scanner {
    /// Everything measured so far under the scan root, as a tree.
    pub tree: Tree,
    /// Target upper bound for `tree.approx_bytes()`, in bytes.
    pub budget: usize,
    /// Number of subdirectories of the directory being scanned (or
    /// loaded) whose walk has to finish.
    pub total: usize,
    /// How many of those `total` subdirectories have been fully walked.
    pub measured: usize,
    /// Whether no scan or load is running.
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
    /// The directory the running (or last) scan job is filling in.
    job_dir: NodeId,
    /// Device id of the scan root, for filesystem-boundary checks.
    root_dev: Option<u64>,
    /// Directories the walk has announced but not listed yet, by token.
    pending: HashMap<u64, Pending>,
    /// Source of unique directory tokens for the walk.
    next_token: Arc<AtomicU64>,
    /// When each visited directory (or one below it) was last shown, as a
    /// value of `clock`. Used to pick eviction victims.
    last_visit: HashMap<NodeId, u64>,
    clock: u64,
    rx: Option<mpsc::Receiver<Msg>>,
}

/// A directory view to restore when going back up.
struct SavedView {
    dir: NodeId,
    selected: usize,
    table_state: ratatui::widgets::TableState,
}

/// What to do with a pending directory's listing when it arrives.
#[derive(Debug, Clone, Copy)]
enum Pending {
    /// Store its entries under this node (unless the tree is over budget).
    Load(NodeId),
    /// Only add its entries' sizes to this (unloaded) ancestor.
    Fold(NodeId),
}

/// A message from the background walk.
enum Msg {
    /// A directory's direct entries.
    Listed { token: u64, entries: Vec<Entry> },
    /// A directory couldn't be listed.
    Unreadable { token: u64 },
    /// One of the job directory's subdirectories has been fully walked.
    BranchDone,
}

/// One directory entry found by the walk.
struct Entry {
    name: Box<str>,
    kind: EntryKind,
}

/// A subdirectory still to be walked: its token and path.
type Subdir = (u64, PathBuf);

enum EntryKind {
    File(u64),
    /// A subdirectory, whose own listing will arrive under this token.
    Dir(u64),
    OtherFilesystem,
}

/// Outcome of [`Scanner::enter_selected`].
#[derive(Debug, PartialEq, Eq)]
pub enum Enter {
    /// Moved into the selected directory, straight from the tree.
    Entered,
    /// Moved into the selected directory, whose entries weren't in memory;
    /// they're now being scanned into the tree (`finished` is `false`).
    Loading,
    /// The selected directory is on another filesystem or couldn't be
    /// read; it needs its own scan.
    NeedsScan(PathBuf),
    /// Nothing to enter: a scan is running, nothing is selected, or the
    /// selection is a file.
    Ignored,
}

impl Scanner {
    /// Starts a background scan of everything under `root`, keeping the
    /// tree within `budget` bytes.
    ///
    /// Listing `root` itself happens synchronously (it's a single, fast
    /// `read_dir` call), so a failure to read `root` is returned here and
    /// its entries are in the tree immediately; the expensive recursive
    /// walk of its subdirectories happens on a background thread pool.
    pub fn spawn(root: PathBuf, budget: usize) -> io::Result<Self> {
        let mut scanner = Self {
            root_dev: device_id(&root),
            tree: Tree::new(root.clone()),
            budget,
            total: 0,
            measured: 0,
            finished: true,
            selected: 0,
            table_state: ratatui::widgets::TableState::default().with_selected(Some(0)),
            current: Tree::ROOT,
            parents: Vec::new(),
            job_dir: Tree::ROOT,
            pending: HashMap::new(),
            next_token: Arc::new(AtomicU64::new(0)),
            last_visit: HashMap::new(),
            clock: 0,
            rx: None,
        };
        scanner.start_job(Tree::ROOT, &root)?;
        scanner.record_visit(Tree::ROOT);
        Ok(scanner)
    }

    /// Adds whatever the background walk has produced so far to the tree,
    /// and sorts the scanned directory's subtree by size once the walk is
    /// done.
    ///
    /// Non-blocking: intended to be called periodically (e.g. once per
    /// `Event::Tick`) rather than awaited.
    pub fn poll(&mut self) {
        let Some(rx) = self.rx.take() else {
            return;
        };
        let mut disconnected = false;
        loop {
            match rx.try_recv() {
                Ok(msg) => self.handle(msg),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if disconnected || self.measured >= self.total {
            self.finish_job();
        } else {
            self.rx = Some(rx);
        }
    }

    /// Fraction of the scanned directory's subdirectories fully walked so
    /// far, in `0.0..=1.0`.
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

    /// Drills into the selected entry if it's a directory, remembering the
    /// current view so [`Scanner::go_up`] can restore it. A directory
    /// whose entries aren't in memory is loaded into the tree in place,
    /// evicting least recently visited directories first if needed.
    pub fn enter_selected(&mut self) -> Enter {
        if !self.finished {
            return Enter::Ignored;
        }
        let Some(&id) = self.current_children().get(self.selected) else {
            return Enter::Ignored;
        };
        let (Some(node), Some(path)) = (self.tree.get(id), self.tree.path(id)) else {
            return Enter::Ignored;
        };
        match &node.kind {
            NodeKind::Dir(Children::Loaded(_)) => {
                self.push_view(id);
                Enter::Entered
            }
            NodeKind::Dir(Children::Unloaded) => {
                self.push_view(id);
                self.evict_for_room();
                match self.start_job(id, &path) {
                    Ok(()) => Enter::Loading,
                    Err(_) => {
                        // Let a separate scan report the error.
                        self.tree.set_children(id, Children::Unreadable);
                        self.go_up();
                        Enter::NeedsScan(path)
                    }
                }
            }
            NodeKind::Dir(Children::OtherFilesystem | Children::Unreadable) => {
                Enter::NeedsScan(path)
            }
            NodeKind::File | NodeKind::Free => Enter::Ignored,
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
        self.record_visit(self.current);
        true
    }

    fn push_view(&mut self, dir: NodeId) {
        let table_state = std::mem::replace(
            &mut self.table_state,
            ratatui::widgets::TableState::default().with_selected(Some(0)),
        );
        self.parents.push(SavedView {
            dir: self.current,
            selected: self.selected,
            table_state,
        });
        self.current = dir;
        self.selected = 0;
        self.record_visit(dir);
    }

    /// Marks `dir` and all of its ancestors as visited now.
    fn record_visit(&mut self, dir: NodeId) {
        self.clock += 1;
        let mut current = Some(dir);
        while let Some(id) = current {
            self.last_visit.insert(id, self.clock);
            current = self.tree.get(id).and_then(|node| node.parent);
        }
    }

    /// If the tree is above three quarters of the budget, evicts the
    /// entries of loaded directories hanging off the current path (never
    /// the path itself), least recently visited first, until it's back
    /// under that mark or there's nothing left to evict.
    fn evict_for_room(&mut self) {
        let target = self.budget / 4 * 3;
        if self.tree.approx_bytes() <= target {
            return;
        }
        let mut path = vec![self.current];
        while let Some(parent) = path.last().and_then(|&id| self.tree.get(id)?.parent) {
            path.push(parent);
        }
        let mut candidates: Vec<(u64, NodeId)> = path
            .iter()
            .filter_map(|&dir| self.tree.children(dir))
            .flatten()
            .copied()
            .filter(|id| !path.contains(id) && self.tree.children(*id).is_some())
            .map(|id| (self.last_visit.get(&id).copied().unwrap_or(0), id))
            .collect();
        candidates.sort_unstable();

        for (_, id) in candidates {
            if self.tree.approx_bytes() <= target {
                break;
            }
            let last_visit = &mut self.last_visit;
            self.tree.evict_children(id, |freed| {
                last_visit.remove(&freed);
            });
        }
    }

    /// Lists `path` (the directory `dir`) synchronously, stores its
    /// entries, and walks its subdirectories in the background.
    fn start_job(&mut self, dir: NodeId, path: &Path) -> io::Result<()> {
        let ctx = Arc::new(WalkCtx::new(self.root_dev, Arc::clone(&self.next_token)));
        let (entries, subdirs) = list_dir(path, &ctx)?;
        self.pending.clear();
        self.tree.clear_size(dir);
        self.store_listing(dir, entries);

        self.job_dir = dir;
        self.total = subdirs.len();
        self.measured = 0;
        self.finished = false;

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            subdirs
                .into_par_iter()
                .for_each_with(tx, |tx, (token, path)| {
                    walk(token, &path, &ctx, tx);
                    let _ = tx.send(Msg::BranchDone);
                });
        });
        self.rx = Some(rx);
        if self.total == 0 {
            self.finish_job();
        }
        Ok(())
    }

    fn finish_job(&mut self) {
        self.finished = true;
        self.rx = None;
        self.pending.clear();
        self.pending.shrink_to_fit();
        self.tree.sort_subtree_by_size(self.job_dir);
        self.tree.shrink_to_fit();
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Listed { token, entries } => match self.pending.remove(&token) {
                Some(Pending::Load(dir)) if self.tree.approx_bytes() <= self.budget => {
                    self.store_listing(dir, entries);
                }
                Some(Pending::Load(dir) | Pending::Fold(dir)) => self.fold_listing(dir, entries),
                None => {}
            },
            Msg::Unreadable { token } => {
                if let Some(Pending::Load(dir)) = self.pending.remove(&token) {
                    self.tree.set_children(dir, Children::Unreadable);
                }
            }
            Msg::BranchDone => self.measured += 1,
        }
    }

    /// Adds `entries` to the tree as `dir`'s children.
    fn store_listing(&mut self, dir: NodeId, entries: Vec<Entry>) {
        self.tree.mark_loaded(dir);
        for Entry { name, kind } in entries {
            match kind {
                EntryKind::File(size) => {
                    self.tree.add_child(dir, name, size, NodeKind::File);
                }
                EntryKind::Dir(token) => {
                    let child =
                        self.tree
                            .add_child(dir, name, 0, NodeKind::Dir(Children::Unloaded));
                    // If the tree can't take another node, keep the size.
                    let pending = child.map_or(Pending::Fold(dir), Pending::Load);
                    self.pending.insert(token, pending);
                }
                EntryKind::OtherFilesystem => {
                    self.tree
                        .add_child(dir, name, 0, NodeKind::Dir(Children::OtherFilesystem));
                }
            }
        }
    }

    /// Adds only the total size of `entries` to unloaded directory `dir`,
    /// and routes the listings of its subdirectories there too.
    fn fold_listing(&mut self, dir: NodeId, entries: Vec<Entry>) {
        let mut size = 0;
        for entry in entries {
            match entry.kind {
                EntryKind::File(file_size) => size += file_size,
                EntryKind::Dir(token) => {
                    self.pending.insert(token, Pending::Fold(dir));
                }
                EntryKind::OtherFilesystem => {}
            }
        }
        self.tree.add_size(dir, size);
    }
}

/// State shared by every thread of one scan job's walk.
struct WalkCtx {
    /// Device id of the scan root; directories on other devices aren't
    /// walked.
    root_dev: Option<u64>,
    /// Source of unique directory tokens.
    next_token: Arc<AtomicU64>,
    /// `(device, inode)` of every multiply-linked file already counted in
    /// this job, so each hard-linked file is counted once (like `du`).
    seen_links: Mutex<HashSet<(u64, u64)>>,
}

impl WalkCtx {
    fn new(root_dev: Option<u64>, next_token: Arc<AtomicU64>) -> Self {
        Self {
            root_dev,
            next_token,
            seen_links: Mutex::new(HashSet::new()),
        }
    }

    /// Records the file `(dev, ino)` and returns whether this is the first
    /// time it's been seen in this job.
    #[cfg(unix)]
    fn first_link(&self, dev: u64, ino: u64) -> bool {
        self.seen_links
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((dev, ino))
    }
}

/// Lists the directory at `token`/`path`, sends its entries, and then
/// walks its subdirectories in parallel.
fn walk(token: u64, path: &Path, ctx: &WalkCtx, tx: &mpsc::Sender<Msg>) {
    match list_dir(path, ctx) {
        Err(_) => {
            let _ = tx.send(Msg::Unreadable { token });
        }
        Ok((entries, subdirs)) => {
            let _ = tx.send(Msg::Listed { token, entries });
            subdirs.into_par_iter().for_each(|(token, path)| {
                walk(token, &path, ctx, tx);
            });
        }
    }
}

/// Lists the direct entries of `path`, reading their metadata in
/// parallel, and returns them along with the subdirectories to walk next
/// (each with a fresh token from `ctx`).
///
/// Symlinks are skipped (avoids cycles and double counting), as are
/// entries whose type can't be read. Subdirectories on another filesystem
/// than the scan root are returned as [`EntryKind::OtherFilesystem`] and
/// not walked. A hard-linked file counts for its full size only the first
/// time any of its links is seen in the job; later links count as `0`.
fn list_dir(path: &Path, ctx: &WalkCtx) -> io::Result<(Vec<Entry>, Vec<Subdir>)> {
    let listed: Vec<(Entry, Option<Subdir>)> = fs::read_dir(path)?
        .par_bridge()
        .filter_map(Result::ok)
        .filter_map(|entry| list_entry(&entry, ctx))
        .collect();
    let mut subdirs = Vec::new();
    let entries = listed
        .into_iter()
        .map(|(entry, subdir)| {
            subdirs.extend(subdir);
            entry
        })
        .collect();
    Ok((entries, subdirs))
}

fn list_entry(entry: &fs::DirEntry, ctx: &WalkCtx) -> Option<(Entry, Option<Subdir>)> {
    let file_type = entry.file_type().ok()?;
    if file_type.is_symlink() {
        return None;
    }
    let name = entry.file_name().to_string_lossy().into();
    if file_type.is_dir() {
        let path = entry.path();
        if crosses_filesystem_boundary(&path, ctx.root_dev) {
            let kind = EntryKind::OtherFilesystem;
            return Some((Entry { name, kind }, None));
        }
        let token = ctx.next_token.fetch_add(1, Ordering::Relaxed);
        let kind = EntryKind::Dir(token);
        return Some((Entry { name, kind }, Some((token, path))));
    }
    // On-disk size (allocated blocks), so sparse files aren't overcounted;
    // extra links to a file already counted add nothing.
    #[cfg(unix)]
    let size = match entry.metadata() {
        Ok(m) if m.nlink() > 1 && !ctx.first_link(m.dev(), m.ino()) => 0,
        Ok(m) => m.blocks() * 512,
        Err(_) => 0,
    };
    #[cfg(not(unix))]
    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
    let kind = EntryKind::File(size);
    Some((Entry { name, kind }, None))
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

    const NO_LIMIT: usize = usize::MAX;

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("duv-scanner-{name}-{}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).unwrap();
        dir
    }

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

    fn scan(dir: &Path, budget: usize) -> Scanner {
        let mut scanner = Scanner::spawn(dir.to_path_buf(), budget).unwrap();
        wait_until_finished(&mut scanner);
        scanner
    }

    fn find(tree: &Tree, parent: NodeId, name: &str) -> NodeId {
        tree.children(parent)
            .unwrap()
            .iter()
            .copied()
            .find(|&id| &*tree.get(id).unwrap().name == name)
            .unwrap()
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

    fn entry_kind<'a>(scanner: &'a Scanner, name: &str) -> &'a NodeKind {
        &scanner.entries().find(|e| &*e.name == name).unwrap().kind
    }

    fn is_loaded(scanner: &Scanner, name: &str) -> bool {
        matches!(
            entry_kind(scanner, name),
            NodeKind::Dir(Children::Loaded(_))
        )
    }

    /// root/
    /// ├── top.txt
    /// ├── empty/
    /// └── sub/
    ///     ├── a.txt
    ///     └── deeper/
    ///         └── c.txt (10 000 bytes)
    fn sample_dir(name: &str) -> PathBuf {
        let dir = test_dir(name);
        fs::create_dir_all(dir.join("sub").join("deeper")).unwrap();
        fs::create_dir_all(dir.join("empty")).unwrap();
        write_file(&dir.join("top.txt"), b"hello");
        write_file(&dir.join("sub").join("a.txt"), b"world!");
        write_file(
            &dir.join("sub").join("deeper").join("c.txt"),
            &[0u8; 10_000],
        );
        dir
    }

    #[test]
    fn builds_a_tree_of_everything_under_root() {
        let dir = sample_dir("tree");
        let scanner = scan(&dir, NO_LIMIT);
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
        let sub = find(tree, Tree::ROOT, "sub");
        let deeper = find(tree, sub, "deeper");
        let c = find(tree, deeper, "c.txt");
        assert_eq!(
            tree.path(c).unwrap(),
            dir.join("sub").join("deeper").join("c.txt")
        );
        assert!(tree.get(c).unwrap().size >= 10_000);
        assert_eq!(tree.get(deeper).unwrap().size, tree.get(c).unwrap().size);

        // An empty directory is loaded with no children, not unloaded.
        let empty = find(tree, Tree::ROOT, "empty");
        assert_eq!(tree.children(empty), Some(&[][..]));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scans_first_level_children_with_correct_sizes() {
        let dir = test_dir("sizes");
        fs::create_dir_all(dir.join("sub")).unwrap();
        write_file(&dir.join("top.txt"), b"hello"); // 5 bytes
        write_file(&dir.join("sub").join("a.txt"), b"world!"); // 6 bytes
        write_file(&dir.join("sub").join("b.txt"), b"!!"); // 2 bytes

        let mut scanner = Scanner::spawn(dir.clone(), NO_LIMIT).unwrap();
        assert_eq!(scanner.entry_count(), 2); // "top.txt" and "sub", immediately
        assert_eq!(scanner.total, 1); // one subdirectory to walk
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
        let dir = test_dir("sparse");

        let file_path = dir.join("sparse.txt");
        let mut file = fs::File::create(&file_path).unwrap();

        // Create a "1 GiB" sparse file by seeking to 1GiB and writing 1 byte
        use std::io::{Seek, SeekFrom, Write};
        file.seek(SeekFrom::Start(1024 * 1024 * 1024)).unwrap();
        file.write_all(b"a").unwrap();

        let scanner = scan(&dir, NO_LIMIT);
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

    #[test]
    fn navigates_the_tree_without_rescanning() {
        let dir = sample_dir("nav");
        let mut scanner = scan(&dir, NO_LIMIT);

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
        let dir = test_dir("busy");
        fs::create_dir_all(dir.join("sub")).unwrap();

        let mut scanner = Scanner::spawn(dir.clone(), NO_LIMIT).unwrap();
        assert!(!scanner.finished);
        assert_eq!(scanner.enter_selected(), Enter::Ignored);

        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_directory_needs_its_own_scan() {
        use std::os::unix::fs::PermissionsExt;

        let dir = test_dir("locked");
        let locked = dir.join("locked");
        fs::create_dir_all(&locked).unwrap();
        write_file(&locked.join("secret.txt"), b"x");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        // Running as root can read it anyway; nothing to test then.
        if fs::read_dir(&locked).is_err() {
            let mut scanner = scan(&dir, NO_LIMIT);
            assert_eq!(
                entry_kind(&scanner, "locked"),
                &NodeKind::Dir(Children::Unreadable)
            );
            select_by_name(&mut scanner, "locked");
            assert_eq!(scanner.enter_selected(), Enter::NeedsScan(locked.clone()));
        }

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn hard_linked_files_are_counted_once() {
        let dir = test_dir("hardlink");
        fs::create_dir_all(dir.join("a")).unwrap();
        fs::create_dir_all(dir.join("b")).unwrap();
        write_file(&dir.join("a").join("big.bin"), &[1u8; 100_000]);
        fs::hard_link(dir.join("a").join("big.bin"), dir.join("b").join("link1")).unwrap();
        fs::hard_link(dir.join("a").join("big.bin"), dir.join("link2")).unwrap();
        write_file(&dir.join("other.bin"), &[2u8; 50_000]);

        let single = {
            let solo = test_dir("hardlink-solo");
            write_file(&solo.join("big.bin"), &[1u8; 100_000]);
            write_file(&solo.join("other.bin"), &[2u8; 50_000]);
            let scanner = scan(&solo, NO_LIMIT);
            let size = scanner.tree.get(Tree::ROOT).unwrap().size;
            fs::remove_dir_all(&solo).ok();
            size
        };

        let scanner = scan(&dir, NO_LIMIT);
        let total = scanner.tree.get(Tree::ROOT).unwrap().size;
        assert_eq!(total, single, "three links to one file count once");

        // All links are still listed in the tree; only one carries the size.
        let tree = &scanner.tree;
        let linked: Vec<u64> = [
            find(tree, find(tree, Tree::ROOT, "a"), "big.bin"),
            find(tree, find(tree, Tree::ROOT, "b"), "link1"),
            find(tree, Tree::ROOT, "link2"),
        ]
        .iter()
        .map(|&id| tree.get(id).unwrap().size)
        .collect();
        assert_eq!(linked.iter().filter(|&&size| size > 0).count(), 1);
        assert_eq!(tree.node_count(), 1 + 2 + 3 + 1); // root, a, b, 3 links, other

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn over_budget_directories_are_folded_with_exact_totals() {
        let dir = sample_dir("fold");
        let full = scan(&dir, NO_LIMIT);
        let folded = scan(&dir, 1);

        // The root's own entries are always stored...
        assert_eq!(entry_names(&folded), entry_names(&full));
        // ...but deeper listings were only summed, not stored.
        assert_eq!(
            entry_kind(&folded, "sub"),
            &NodeKind::Dir(Children::Unloaded)
        );
        assert_eq!(folded.tree.node_count(), 4); // root, top.txt, sub, empty

        let sizes = |scanner: &Scanner| -> Vec<u64> { scanner.entries().map(|e| e.size).collect() };
        assert_eq!(sizes(&folded), sizes(&full));
        assert_eq!(
            folded.tree.get(Tree::ROOT).unwrap().size,
            full.tree.get(Tree::ROOT).unwrap().size
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn opening_a_folded_directory_loads_it_in_place() {
        let dir = sample_dir("load");
        let mut scanner = scan(&dir, 1);
        let root_size = scanner.tree.get(Tree::ROOT).unwrap().size;

        select_by_name(&mut scanner, "sub");
        assert_eq!(scanner.enter_selected(), Enter::Loading);
        assert_eq!(scanner.current_path(), dir.join("sub"));
        wait_until_finished(&mut scanner);

        assert_eq!(entry_names(&scanner), ["deeper", "a.txt"]);
        // Same data on disk, so the reloaded totals match.
        assert_eq!(scanner.tree.get(Tree::ROOT).unwrap().size, root_size);

        assert!(scanner.go_up());
        assert!(is_loaded(&scanner, "sub"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn loading_evicts_the_least_recently_visited_branch() {
        // root/
        // ├── a/{0..19}.txt
        // ├── b/{0..19}.txt
        // └── c/{0..19}.txt
        let dir = test_dir("evict");
        for sub in ["a", "b", "c"] {
            fs::create_dir_all(dir.join(sub)).unwrap();
            for i in 0..20 {
                write_file(&dir.join(sub).join(format!("{i}.txt")), b"x");
            }
        }
        // Start with only the root's own entries loaded.
        let mut scanner = scan(&dir, 1);
        let root_size = scanner.tree.get(Tree::ROOT).unwrap().size;

        let open = |scanner: &mut Scanner, name: &str| {
            select_by_name(scanner, name);
            assert_eq!(scanner.enter_selected(), Enter::Loading);
            wait_until_finished(scanner);
            assert_eq!(scanner.entry_count(), 20);
            assert!(scanner.go_up());
        };

        scanner.budget = NO_LIMIT;
        open(&mut scanner, "a");
        open(&mut scanner, "b");

        // Room for about two loaded directories: the eviction mark
        // (3/4 of the budget) sits just under the current size.
        let two_loaded = scanner.tree.approx_bytes();
        scanner.budget = (two_loaded - 100) / 3 * 4;

        // `a` is the least recently visited, so it's evicted.
        open(&mut scanner, "c");
        assert!(!is_loaded(&scanner, "a"));
        assert!(is_loaded(&scanner, "b"));
        assert!(is_loaded(&scanner, "c"));

        // Now `b` is: reopening `a` evicts `b` and loads `a` again.
        open(&mut scanner, "a");
        assert!(is_loaded(&scanner, "a"));
        assert!(!is_loaded(&scanner, "b"));
        assert!(is_loaded(&scanner, "c"));

        // Evicted nodes' slots were reused, and totals never changed.
        assert_eq!(scanner.tree.node_count(), 1 + 3 + 20 + 20);
        assert_eq!(scanner.tree.node_capacity(), 1 + 3 + 20 + 20);
        assert_eq!(scanner.tree.get(Tree::ROOT).unwrap().size, root_size);

        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn same_filesystem_paths_share_a_device_id() {
        let dir = test_dir("devid");
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
        let dir = test_dir("devid2");

        let real_dev = device_id(&dir).unwrap();
        let fake_dev = real_dev.wrapping_add(1);
        assert!(crosses_filesystem_boundary(&dir, Some(fake_dev)));

        fs::remove_dir_all(&dir).ok();
    }
}
