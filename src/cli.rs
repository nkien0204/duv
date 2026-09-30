//! Command-line argument parsing.
//!
//! `duv [path]` — with a path (`duv .` for the current directory), `duv`
//! starts by scanning it; without one, it opens on the disk list. Validation
//! of `path` happens here, before the terminal enters raw mode, so mistakes
//! surface as a normal shell error instead of inside the TUI.

use std::path::{Path, PathBuf};

use clap::Parser;

/// A fast terminal-based disk usage manager and monitor.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    /// Directory to scan on startup (use `.` for the current directory).
    /// Opens the disk list if omitted.
    pub path: Option<PathBuf>,
}

impl Cli {
    /// Returns the start path as an absolute, symlink-resolved directory,
    /// or an error if it doesn't exist or isn't a directory.
    pub fn start_path(&self) -> Result<Option<PathBuf>, String> {
        self.path.as_deref().map(resolve_dir).transpose()
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
