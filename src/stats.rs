//! Headless scan statistics, behind the hidden `--stats` flag.
//!
//! A developer tool for measuring what a scan costs: it runs a
//! [`Scanner`] to completion without the TUI and prints node counts, the
//! tree's own memory estimate ([`Tree::approx_bytes`]), spare node-list
//! capacity, timing, and the process's peak resident memory as reported
//! by the OS. Comparing the estimate with the peak shows how accurate the
//! estimate is and how much transient memory the scan needs on top of
//! the finished tree.

use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;

use crate::disks::format_bytes;
use crate::model::{Children, Node, NodeKind, Tree};
use crate::scanner::Scanner;

/// Scans `path` to completion with the given memory `budget` (bytes) and
/// prints statistics to stdout.
pub fn run(path: PathBuf, budget: usize) -> Result<()> {
    let startup_rss = peak_rss_bytes();
    let started = Instant::now();

    let mut scanner = Scanner::spawn(path, budget)?;
    while !scanner.finished {
        scanner.poll();
        thread::sleep(Duration::from_millis(5));
    }
    let elapsed = started.elapsed();
    let peak_rss = peak_rss_bytes();

    let tree = &scanner.tree;
    let (mut files, mut loaded, mut unloaded, mut other_fs, mut unreadable) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    for (_, node) in tree.iter() {
        match &node.kind {
            NodeKind::File => files += 1,
            NodeKind::Dir(Children::Loaded(_)) => loaded += 1,
            NodeKind::Dir(Children::Unloaded) => unloaded += 1,
            NodeKind::Dir(Children::OtherFilesystem) => other_fs += 1,
            NodeKind::Dir(Children::Unreadable) => unreadable += 1,
            NodeKind::Free => {}
        }
    }
    let nodes = tree.node_count();
    let largest_branch = largest_top_level_branch(tree);
    let estimate = tree.approx_bytes();
    let spare = (tree.node_capacity() - nodes) * std::mem::size_of::<Node>();
    let total_size = tree.get(Tree::ROOT).map_or(0, |root| root.size);

    println!("path:            {}", tree.root_path().display());
    println!("scan time:       {:.2} s", elapsed.as_secs_f64());
    println!("total size:      {}", format_bytes(total_size));
    println!("nodes:           {nodes} (files {files}, loaded dirs {loaded})");
    println!(
        "dirs not loaded: {unloaded} over budget, {other_fs} other filesystem, {unreadable} unreadable"
    );
    println!("memory budget:   {}", format_bytes(budget as u64));
    println!(
        "tree estimate:   {} ({} B/node)",
        format_bytes(estimate as u64),
        estimate / nodes.max(1)
    );
    println!(
        "node list:       capacity {} for {nodes} nodes (+{} unused)",
        tree.node_capacity(),
        format_bytes(spare as u64)
    );
    if let Some((name, count)) = largest_branch {
        println!(
            "largest branch:  {name} ({count} nodes, {:.0}% of tree)",
            count as f64 * 100.0 / nodes.max(1) as f64
        );
    }
    match (startup_rss, peak_rss) {
        (Some(startup), Some(peak)) => println!(
            "peak RSS:        {} (startup {}, scan +{})",
            format_bytes(peak),
            format_bytes(startup),
            format_bytes(peak.saturating_sub(startup))
        ),
        _ => println!("peak RSS:        unavailable on this platform"),
    }
    Ok(())
}

/// The first-level child with the most nodes under it (itself included),
/// with that count, showing how evenly the tree is spread.
fn largest_top_level_branch(tree: &Tree) -> Option<(String, usize)> {
    let mut counts = vec![0usize; tree.node_capacity()];
    for (id, _) in tree.iter() {
        let mut current = id;
        while let Some(parent) = tree.get(current).and_then(|node| node.parent) {
            if parent == Tree::ROOT {
                counts[current as usize] += 1;
                break;
            }
            current = parent;
        }
    }
    let (id, &count) = counts.iter().enumerate().max_by_key(|&(_, count)| count)?;
    let name = tree.get(id as u32)?.name.to_string();
    Some((name, count))
}

/// The process's peak resident set size so far, in bytes.
#[cfg(unix)]
fn peak_rss_bytes() -> Option<u64> {
    // SAFETY: `getrusage` only writes into the zero-initialized struct we
    // pass it, and `RUSAGE_SELF` is always a valid target.
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) != 0 {
            return None;
        }
        usage
    };
    let max_rss = u64::try_from(usage.ru_maxrss).ok()?;
    // macOS reports bytes; Linux and the BSDs report kilobytes.
    if cfg!(target_os = "macos") {
        Some(max_rss)
    } else {
        Some(max_rss * 1024)
    }
}

#[cfg(not(unix))]
fn peak_rss_bytes() -> Option<u64> {
    None
}
