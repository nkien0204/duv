//! Disk/volume enumeration.
//!
//! Wraps [`sysinfo`]'s `Disks` API behind a plain, crate-local [`DiskInfo`]
//! type so the rest of the app (in particular `app.rs`) never has to depend
//! on `sysinfo`'s types directly. This keeps `App` free to add fields like a
//! `selected` index without leaking a third-party API into the model.

use std::path::Path;

use sysinfo::Disks;

#[cfg(target_os = "macos")]
const MACOS_SYSTEM_VOLUMES_PREFIX: &str = "/System/Volumes/";

/// A single mounted disk/volume, and the subset of its metadata `duv` cares
/// about for picking a scan target.
#[derive(Debug, Clone)]
pub struct DiskInfo {
    pub name: String,
    pub mount_point: String,
    pub file_system: String,
    pub total_space: u64,
    /// Bytes an ordinary user can still write.
    pub available_space: u64,
    /// Free bytes including any reserved for the superuser (e.g. ext4's
    /// 5% root reserve). Equal to `available_space` where nothing is
    /// reserved.
    pub free_space: u64,
    pub is_removable: bool,
    pub kind: DiskKind,
}

impl DiskInfo {
    /// Bytes currently in use on this disk: total minus free, like `df`'s
    /// Used column. Space reserved for the superuser isn't counted as used
    /// (it's neither used nor available), so on Linux Used + Available can
    /// be less than Total, as in `df`.
    pub fn used_space(&self) -> u64 {
        self.total_space.saturating_sub(self.free_space)
    }

    /// Fraction of the disk in use, in `0.0..=1.0`. Returns `0.0` for a
    /// zero-sized disk rather than dividing by zero.
    pub fn used_fraction(&self) -> f64 {
        if self.total_space == 0 {
            0.0
        } else {
            self.used_space() as f64 / self.total_space as f64
        }
    }
}

/// The kind of underlying storage medium, mirroring [`sysinfo::DiskKind`]
/// so callers outside this module never need to depend on `sysinfo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskKind {
    Hdd,
    Ssd,
    Unknown,
}

impl std::fmt::Display for DiskKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DiskKind::Hdd => "HDD",
            DiskKind::Ssd => "SSD",
            DiskKind::Unknown => "Unknown",
        })
    }
}

impl From<sysinfo::DiskKind> for DiskKind {
    fn from(kind: sysinfo::DiskKind) -> Self {
        match kind {
            sysinfo::DiskKind::HDD => DiskKind::Hdd,
            sysinfo::DiskKind::SSD => DiskKind::Ssd,
            sysinfo::DiskKind::Unknown(_) => DiskKind::Unknown,
        }
    }
}

/// Lists every mounted disk/volume currently visible to the OS.
///
/// Returns disks in whatever order `sysinfo` reports them; callers that
/// need a stable order (e.g. for display) should sort explicitly.
pub fn list() -> Vec<DiskInfo> {
    let disks: Vec<DiskInfo> = Disks::new_with_refreshed_list()
        .list()
        .iter()
        .map(|disk| {
            let available_space = disk.available_space();
            DiskInfo {
                name: disk.name().to_string_lossy().into_owned(),
                mount_point: disk.mount_point().to_string_lossy().into_owned(),
                file_system: disk.file_system().to_string_lossy().into_owned(),
                total_space: disk.total_space(),
                available_space,
                // `sysinfo` only reports available space, so ask the OS;
                // free space is never less than what's available.
                free_space: free_space(disk.mount_point())
                    .map_or(available_space, |free| free.max(available_space)),
                is_removable: disk.is_removable(),
                kind: disk.kind().into(),
            }
        })
        .collect();

    #[cfg(target_os = "macos")]
    let disks = disks
        .into_iter()
        .filter(|disk| !disk.mount_point.starts_with(MACOS_SYSTEM_VOLUMES_PREFIX))
        .collect();

    // todo: filter out other system volumes on other platforms if needed

    disks
}

/// Free bytes on the filesystem mounted at `mount_point`, including blocks
/// reserved for the superuser (`statvfs`'s `f_bfree`), as `df` uses for
/// its Used column. `None` if it can't be read.
#[cfg(unix)]
// The `statvfs` field types vary by platform (e.g. 32-bit block counts on
// macOS), so the `u64::from` conversions are needed even where a field is
// already a `u64`.
#[allow(clippy::useless_conversion)]
fn free_space(mount_point: &Path) -> Option<u64> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let path = CString::new(mount_point.as_os_str().as_bytes()).ok()?;
    // SAFETY: `path` is a valid NUL-terminated string, and `statvfs` only
    // writes into the zero-initialized struct we pass it.
    let stats = unsafe {
        let mut stats: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(path.as_ptr(), &mut stats) != 0 {
            return None;
        }
        stats
    };
    // Block counts are in units of the fragment size (falling back to the
    // block size where a filesystem leaves it unset). Field widths differ
    // between platforms, hence `u64::from`.
    let fragment = match u64::from(stats.f_frsize) {
        0 => u64::from(stats.f_bsize),
        size => size,
    };
    u64::from(stats.f_bfree).checked_mul(fragment)
}

/// Windows reserves nothing for an administrator the way Unix filesystems
/// do, so free and available space are the same.
#[cfg(not(unix))]
fn free_space(_mount_point: &Path) -> Option<u64> {
    None
}

/// Formats a byte count as a human-readable string using binary (1024)
/// units, e.g. `1536` -> `"1.5 KiB"`.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(total: u64, available: u64, free: u64) -> DiskInfo {
        DiskInfo {
            name: "disk".to_string(),
            mount_point: "/".to_string(),
            file_system: "ext4".to_string(),
            total_space: total,
            available_space: available,
            free_space: free,
            is_removable: false,
            kind: DiskKind::Ssd,
        }
    }

    #[test]
    fn list_does_not_panic_and_reports_sane_sizes() {
        for disk in list() {
            assert!(disk.available_space <= disk.total_space || disk.total_space == 0);
            assert!(disk.free_space >= disk.available_space);
            assert_eq!(
                disk.used_space(),
                disk.total_space.saturating_sub(disk.free_space)
            );
        }
    }

    #[test]
    fn used_space_excludes_the_root_reserve_like_df() {
        // Your Ubuntu server's `/`: 9.8G total, 4.2G available, plus about
        // 0.5G reserved for root. `df` says 5.1G used, not 5.6G.
        const TENTH_GIB: u64 = 1024 * 1024 * 1024 / 10;
        let root = disk(98 * TENTH_GIB, 42 * TENTH_GIB, 47 * TENTH_GIB);
        assert_eq!(root.used_space(), 51 * TENTH_GIB);

        // Where nothing is reserved, used is simply total minus available.
        assert_eq!(disk(100, 40, 40).used_space(), 60);
    }

    #[cfg(unix)]
    #[test]
    fn free_space_reads_the_filesystem() {
        let free = free_space(Path::new("/")).expect("statvfs on /");
        assert!(free > 0);
        assert_eq!(free_space(Path::new("/definitely/not/mounted/here")), None);
    }

    #[test]
    fn format_bytes_uses_binary_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(1024 * 1024 * 3), "3.0 MiB");
    }
}
