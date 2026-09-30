//! Command-line argument parsing.
//!
//! `duv [path]` — with a path (`duv .` for the current directory), `duv`
//! starts by scanning it; without one, it opens on the disk list. Validation
//! of `path` happens here, before the terminal enters raw mode, so mistakes
//! surface as a normal shell error instead of inside the TUI.

use std::path::{Path, PathBuf};

use clap::Parser;

use crate::scanner::DEFAULT_MEMORY_BUDGET;

const MIB: usize = 1024 * 1024;

/// A fast terminal-based disk usage manager and monitor.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    /// Directory to scan on startup (use `.` for the current directory).
    /// Opens the disk list if omitted.
    pub path: Option<PathBuf>,

    /// Memory budget for the scanned tree, in MiB. Directories beyond it
    /// are kept as totals only and loaded when opened.
    #[arg(
        long,
        value_name = "MIB",
        default_value_t = (DEFAULT_MEMORY_BUDGET / MIB) as u32,
        value_parser = clap::value_parser!(u32).range(1..),
    )]
    pub memory_budget: u32,

    /// Developer tool: scan `path` without the TUI, print node counts,
    /// memory and timing, then exit.
    #[arg(long, hide = true, requires = "path")]
    pub stats: bool,
}

impl Cli {
    /// Returns the start path as an absolute, symlink-resolved directory,
    /// or an error if it doesn't exist or isn't a directory.
    pub fn start_path(&self) -> Result<Option<PathBuf>, String> {
        self.path.as_deref().map(resolve_dir).transpose()
    }

    /// The memory budget in bytes.
    pub fn memory_budget_bytes(&self) -> usize {
        self.memory_budget as usize * MIB
    }
}

fn resolve_dir(path: &Path) -> Result<PathBuf, String> {
    let resolved = path
        .canonicalize()
        .map_err(|err| format!("cannot access '{}': {err}", path.display()))?;
    if !resolved.is_dir() {
        return Err(format!("'{}' is not a directory", path.display()));
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_path_means_disk_list() {
        let cli = Cli::try_parse_from(["duv"]).unwrap();
        assert_eq!(cli.start_path().unwrap(), None);
    }

    #[test]
    fn dot_means_current_directory() {
        let cli = Cli::try_parse_from(["duv", "."]).unwrap();
        assert_eq!(
            cli.start_path().unwrap(),
            Some(std::env::current_dir().unwrap().canonicalize().unwrap())
        );
    }

    #[test]
    fn existing_directory_is_resolved() {
        let dir = std::env::temp_dir();
        let cli = Cli::try_parse_from(["duv", dir.to_str().unwrap()]).unwrap();
        assert_eq!(cli.start_path().unwrap(), Some(dir.canonicalize().unwrap()));
    }

    #[test]
    fn stats_requires_a_path() {
        assert!(Cli::try_parse_from(["duv", "--stats"]).is_err());
        let cli = Cli::try_parse_from(["duv", "--stats", "."]).unwrap();
        assert!(cli.stats);
    }

    #[test]
    fn memory_budget_defaults_and_parses_in_mib() {
        let cli = Cli::try_parse_from(["duv"]).unwrap();
        assert_eq!(cli.memory_budget_bytes(), DEFAULT_MEMORY_BUDGET);

        let cli = Cli::try_parse_from(["duv", "--memory-budget", "64"]).unwrap();
        assert_eq!(cli.memory_budget_bytes(), 64 * MIB);

        assert!(Cli::try_parse_from(["duv", "--memory-budget", "0"]).is_err());
        assert!(Cli::try_parse_from(["duv", "--memory-budget", "lots"]).is_err());
    }

    #[test]
    fn missing_path_is_rejected() {
        let cli = Cli::try_parse_from(["duv", "/definitely/not/a/real/path"]).unwrap();
        assert!(cli.start_path().unwrap_err().contains("cannot access"));
    }

    #[test]
    fn file_path_is_rejected() {
        let file = std::env::temp_dir().join(format!("duv-cli-test-{}", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let cli = Cli::try_parse_from(["duv", file.to_str().unwrap()]).unwrap();
        let err = cli.start_path().unwrap_err();
        std::fs::remove_file(&file).unwrap();
        assert!(err.contains("not a directory"));
    }
}
