//! First-level directory-size scanning.
//!
//! Given a root path (a disk's mount point, or any directory the user has
//! drilled into), walks its immediate children and computes the total
//! on-disk size of each one — recursing into subdirectories with `rayon`
//! so multiple branches of the tree are measured in parallel. Runs on a
//! background thread and reports progress through an `mpsc` channel,
//! polled from `App::tick()` (driven by `Event::Tick`) so the UI thread is
//! never blocked waiting on I/O.
//!
//! A [`Scanner`] only ever measures one level of a directory tree at a
//! time. Drilling further into a subdirectory means spawning another
//! [`Scanner`] rooted there — see `App::enter_selected` and
//! `App::scanner_history`, which keep completed parent scans around so
//! backing out doesn't require re-scanning them.
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

/// One first-level child of the scanned root, with its total size.
///
/// For a directory, `size` is the recursive sum of everything under it.
/// For a plain file, it's just the file's own length.
#[derive(Debug, Clone)]
pub struct ScanEntry {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub is_dir: bool,
}

/// A directory-size scan in progress (or finished) on a background thread.
pub struct Scanner {
    /// The path whose immediate children are being scanned.
    pub root: PathBuf,
    /// Number of first-level children being measured.
    pub total: usize,
    /// Results received from the background thread so far. Sorted by size
    /// descending once `finished` is set, so `selected` indexes into a
    /// stable, display-ready order.
    pub entries: Vec<ScanEntry>,
    /// Whether every entry has been measured (or the scan thread has
    /// otherwise finished / disconnected).
    pub finished: bool,
    /// Index into `entries` of the currently highlighted row, once
    /// `finished`.
    pub selected: usize,
    /// Scroll/selection state for the results `Table`, kept here (rather
    /// than recreated each frame) so ratatui can track the scroll offset
    /// across renders and keep `selected` in view as the list scrolls.
    pub table_state: ratatui::widgets::TableState,
    rx: mpsc::Receiver<ScanEntry>,
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
                let size = if child.is_dir {
                    if crosses_filesystem_boundary(&child.path, root_dev) {
                        0
                    } else {
                        dir_size(&child.path, root_dev)
                    }
                } else {
                    child.size
                };
                let _ = tx.send(ScanEntry {
                    name: child.name,
                    path: child.path,
                    size,
                    is_dir: child.is_dir,
                });
            });
        });

        Ok(Self {
            root,
            total,
            entries: Vec::with_capacity(total),
            finished: total == 0,
            selected: 0,
            table_state: ratatui::widgets::TableState::default().with_selected(Some(0)),
            rx,
        })
    }

    /// Drains any results the background thread has produced so far, and
    /// sorts `entries` by size descending once every entry has arrived.
    ///
    /// Non-blocking: intended to be called periodically (e.g. once per
    /// `Event::Tick`) rather than awaited.
    pub fn poll(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(entry) => self.entries.push(entry),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.finished = true;
                    break;
                }
            }
        }
        if self.entries.len() >= self.total {
            self.finished = true;
        }
        if self.finished {
            self.entries
                .sort_by_key(|entry| std::cmp::Reverse(entry.size));
        }
    }

    /// Fraction of first-level children measured so far, in `0.0..=1.0`.
    pub fn progress_fraction(&self) -> f64 {
        if self.total == 0 {
            1.0
        } else {
            (self.entries.len() as f64 / self.total as f64).min(1.0)
        }
    }

    /// The currently highlighted entry, if any.
    pub fn selected_entry(&self) -> Option<&ScanEntry> {
        self.entries.get(self.selected)
    }

    /// Moves the selection to the next entry, wrapping at the end.
    pub fn select_next(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.entries.len();
        self.table_state.select(Some(self.selected));
    }

    /// Moves the selection to the previous entry, wrapping at the start.
    pub fn select_previous(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        self.selected = self
            .selected
            .checked_sub(1)
            .unwrap_or(self.entries.len() - 1);
        self.table_state.select(Some(self.selected));
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

/// Recursively sums the size of every file under `path`, parallelizing
/// across subdirectories with `rayon`. Skips entries it can't read
/// (permission errors, races) and symlinks (to avoid cycles/double
/// counting) rather than failing the whole scan. Also stops at
/// filesystem boundaries (like `du -x`/`--one-file-system`): a
/// subdirectory that's the mount point of a different filesystem than
/// `root_dev` contributes `0`, since its space isn't part of the disk
/// being measured (this is what prevents e.g. `/System/Volumes/Data`,
/// `/Volumes/*`, or network mounts reachable from `/` from being summed
/// into the size of the root filesystem).
fn dir_size(path: &Path, root_dev: Option<u64>) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .par_bridge()
        .filter_map(Result::ok)
        .map(|entry| entry_size(&entry, root_dev))
        .sum()
}

fn entry_size(entry: &fs::DirEntry, root_dev: Option<u64>) -> u64 {
    let Ok(file_type) = entry.file_type() else {
        return 0;
    };
    if file_type.is_symlink() {
        0
    } else if file_type.is_dir() {
        let path = entry.path();
        if crosses_filesystem_boundary(&path, root_dev) {
            0
        } else {
            dir_size(&path, root_dev)
        }
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

    #[test]
    fn scans_first_level_children_with_correct_sizes() {
        let dir = std::env::temp_dir().join(format!("duv-scanner-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("sub")).unwrap();
        write_file(&dir.join("top.txt"), b"hello"); // 5 bytes
        write_file(&dir.join("sub").join("a.txt"), b"world!"); // 6 bytes
        write_file(&dir.join("sub").join("b.txt"), b"!!"); // 2 bytes

        let mut scanner = Scanner::spawn(dir.clone()).unwrap();
        assert_eq!(scanner.total, 2); // "top.txt" and "sub"

        let deadline = Instant::now() + Duration::from_secs(5);
        while !scanner.finished && Instant::now() < deadline {
            scanner.poll();
            thread::sleep(Duration::from_millis(10));
        }
        assert!(scanner.finished, "scan did not finish in time");

        let file_entry = scanner
            .entries
            .iter()
            .find(|e| e.name == "top.txt")
            .unwrap();
        assert!(file_entry.size >= 5);
        assert!(!file_entry.is_dir);

        let dir_entry = scanner.entries.iter().find(|e| e.name == "sub").unwrap();
        assert!(dir_entry.size >= 8); // 6 + 2
        assert!(dir_entry.is_dir);

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
        let deadline = Instant::now() + Duration::from_secs(5);
        while !scanner.finished && Instant::now() < deadline {
            scanner.poll();
            thread::sleep(Duration::from_millis(10));
        }

        let entry = scanner
            .entries
            .iter()
            .find(|e| e.name == "sparse.txt")
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
