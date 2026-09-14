//! Disk/volume enumeration.
//!
//! Wraps [`sysinfo`]'s `Disks` API behind a plain, crate-local [`DiskInfo`]
//! type so the rest of the app (in particular `app.rs`) never has to depend
//! on `sysinfo`'s types directly. This keeps `App` free to add fields like a
//! `selected` index without leaking a third-party API into the model.

use sysinfo::Disks;

const MACOS_SYSTEM_VOLUMES_PREFIX: &str = "/System/Volumes/";

/// A single mounted disk/volume, and the subset of its metadata `duv` cares
/// about for picking a scan target.
#[derive(Debug, Clone)]
pub struct DiskInfo {
    pub name: String,
    pub mount_point: String,
    pub file_system: String,
    pub total_space: u64,
    pub available_space: u64,
    pub is_removable: bool,
    pub kind: DiskKind,
}

impl DiskInfo {
    /// Bytes currently in use on this disk (total minus available).
    pub fn used_space(&self) -> u64 {
        self.total_space.saturating_sub(self.available_space)
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
        .map(|disk| DiskInfo {
            name: disk.name().to_string_lossy().into_owned(),
            mount_point: disk.mount_point().to_string_lossy().into_owned(),
            file_system: disk.file_system().to_string_lossy().into_owned(),
            total_space: disk.total_space(),
            available_space: disk.available_space(),
            is_removable: disk.is_removable(),
            kind: disk.kind().into(),
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

    #[test]
    fn list_does_not_panic_and_reports_sane_sizes() {
        for disk in list() {
            assert!(disk.available_space <= disk.total_space || disk.total_space == 0);
        }
    }

    #[test]
    fn format_bytes_uses_binary_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(1024 * 1024 * 3), "3.0 MiB");
    }
}
