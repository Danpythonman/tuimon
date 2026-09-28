//! Physical disks and the filesystems mounted on them.
//!
//! Disks and their I/O counters are read from `/sys/block`, which only
//! exists on Linux; filesystem space comes from `sysinfo`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use sysinfo::{DiskRefreshKind, Disks};

const SYS_BLOCK: &str = "/sys/block";
const SYS_CLASS_BLOCK: &str = "/sys/class/block";
/// `/sys/block/*/stat` and `size` count 512-byte sectors regardless of the
/// device's real sector size.
const SECTOR_SIZE: u64 = 512;
/// Device-mapper and RAID devices can be stacked (e.g. LVM on LUKS), but
/// never deeply.
const MAX_STACK_DEPTH: usize = 8;

/// A physical disk with its current I/O rates.
#[derive(Debug, Clone, Default)]
pub struct DiskStats {
    /// Kernel name, e.g. `nvme0n1` or `sda`.
    pub name: String,
    pub model: Option<String>,
    /// Capacity in bytes.
    pub size: u64,
    pub kind: DiskKind,
    /// Bytes per second.
    pub read_rate: f64,
    pub write_rate: f64,
    /// Operations per second.
    pub read_ops: f64,
    pub write_ops: f64,
    /// Bytes since boot.
    pub total_read: u64,
    pub total_written: u64,
    /// Share of time the disk had I/O in flight, like `iostat`'s `%util`.
    pub busy_percent: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiskKind {
    Nvme,
    #[default]
    Ssd,
    Hdd,
    Removable,
}

impl DiskKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Nvme => "NVMe",
            Self::Ssd => "SSD",
            Self::Hdd => "HDD",
            Self::Removable => "Removable",
        }
    }
}

/// A mounted filesystem backed by a block device.
#[derive(Debug, Clone)]
pub struct Filesystem {
    /// Device path, e.g. `/dev/nvme0n1p2`.
    pub device: String,
    pub mount_point: String,
    pub fs_type: String,
    pub total: u64,
    pub available: u64,
    /// The physical disk this lives on, if it could be traced back to one.
    pub disk: Option<String>,
}

impl Filesystem {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }

    pub fn percent(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.used() as f64 / self.total as f64 * 100.0
        }
    }
}

/// Cumulative counters from `/sys/block/<disk>/stat`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Counters {
    reads: u64,
    read_sectors: u64,
    writes: u64,
    write_sectors: u64,
    /// Milliseconds spent with I/O in flight.
    busy_ms: u64,
}

impl Counters {
    /// Parses a `stat` line; see the kernel's `Documentation/block/stat.rst`.
    fn parse(line: &str) -> Option<Self> {
        let fields: Vec<u64> = line
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()?;
        Some(Self {
            reads: *fields.first()?,
            read_sectors: *fields.get(2)?,
            writes: *fields.get(4)?,
            write_sectors: *fields.get(6)?,
            busy_ms: *fields.get(9)?,
        })
    }
}

/// Tracks disks between refreshes so rates can be computed from deltas.
pub struct DiskMonitor {
    disks: Disks,
    previous: HashMap<String, Counters>,
    /// Maps a filesystem's device path to its physical disk. Resolving it
    /// walks sysfs, and devices don't move between disks.
    owners: HashMap<String, Option<String>>,
}

impl DiskMonitor {
    pub fn new() -> Self {
        let mut monitor = Self {
            disks: Disks::new_with_refreshed_list_specifics(refresh_kind()),
            previous: HashMap::new(),
            owners: HashMap::new(),
        };
        // Prime the counters so the first refresh reports real rates.
        monitor.refresh_disks(1.0);
        monitor
    }

    /// Returns the physical disks, sorted by name, and the filesystems on
    /// block devices, sorted by mount point.
    pub fn refresh(&mut self, elapsed_secs: f64) -> (Vec<DiskStats>, Vec<Filesystem>) {
        (self.refresh_disks(elapsed_secs), self.refresh_filesystems())
    }

    fn refresh_disks(&mut self, elapsed_secs: f64) -> Vec<DiskStats> {
        let mut disks = Vec::new();
        let mut counters = HashMap::new();
        for name in physical_disk_names() {
            let dir = Path::new(SYS_BLOCK).join(&name);
            let Some(current) = read_trimmed(&dir.join("stat")).and_then(|s| Counters::parse(&s))
            else {
                continue;
            };
            // Before the first sample, or if the counters reset (e.g. the
            // disk was replugged), there's no meaningful rate yet.
            let prev = self
                .previous
                .get(&name)
                .copied()
                .filter(|p| p.read_sectors <= current.read_sectors)
                .unwrap_or(current);
            let per_sec = |now: u64, then: u64| now.saturating_sub(then) as f64 / elapsed_secs;
            disks.push(DiskStats {
                model: read_trimmed(&dir.join("device/model")).filter(|m| !m.is_empty()),
                size: read_u64(&dir.join("size")).unwrap_or(0) * SECTOR_SIZE,
                kind: disk_kind(&name, &dir),
                read_rate: per_sec(current.read_sectors, prev.read_sectors) * SECTOR_SIZE as f64,
                write_rate: per_sec(current.write_sectors, prev.write_sectors) * SECTOR_SIZE as f64,
                read_ops: per_sec(current.reads, prev.reads),
                write_ops: per_sec(current.writes, prev.writes),
                total_read: current.read_sectors * SECTOR_SIZE,
                total_written: current.write_sectors * SECTOR_SIZE,
                busy_percent: (per_sec(current.busy_ms, prev.busy_ms) / 10.0).min(100.0),
                name: name.clone(),
            });
            counters.insert(name, current);
        }
        self.previous = counters;
        disks.sort_by(|a, b| a.name.cmp(&b.name));
        disks
    }

    fn refresh_filesystems(&mut self) -> Vec<Filesystem> {
        self.disks.refresh_specifics(true, refresh_kind());
        let mut filesystems: Vec<Filesystem> = self
            .disks
            .list()
            .iter()
            .filter_map(|d| {
                let device = d.name().to_str()?.to_owned();
                // Skip overlays, FUSE and other filesystems without a device.
                if !device.starts_with("/dev/") {
                    return None;
                }
                let disk = self
                    .owners
                    .entry(device.clone())
                    .or_insert_with(|| owning_disk(&device))
                    .clone();
                Some(Filesystem {
                    device,
                    mount_point: d.mount_point().to_string_lossy().into_owned(),
                    fs_type: d.file_system().to_string_lossy().into_owned(),
                    total: d.total_space(),
                    available: d.available_space(),
                    disk,
                })
            })
            .collect();
        filesystems.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
        filesystems
    }
}

fn refresh_kind() -> DiskRefreshKind {
    DiskRefreshKind::nothing().with_storage()
}

/// Block devices backed by hardware. Virtual ones (loop, device-mapper,
/// RAID, zram) have no `device` link, and empty card readers report size 0.
fn physical_disk_names() -> Vec<String> {
    let Ok(entries) = fs::read_dir(SYS_BLOCK) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|name| {
            let dir = Path::new(SYS_BLOCK).join(name);
            dir.join("device").exists() && read_u64(&dir.join("size")).unwrap_or(0) > 0
        })
        .collect()
}

fn disk_kind(name: &str, dir: &Path) -> DiskKind {
    if read_u64(&dir.join("removable")) == Some(1) {
        DiskKind::Removable
    } else if name.starts_with("nvme") {
        DiskKind::Nvme
    } else if read_u64(&dir.join("queue/rotational")) == Some(1) {
        DiskKind::Hdd
    } else {
        DiskKind::Ssd
    }
}

/// Traces a device path like `/dev/mapper/root` or `/dev/sda2` back to the
/// physical disk it lives on.
fn owning_disk(device: &str) -> Option<String> {
    let resolved = fs::canonicalize(device).ok()?;
    let name = resolved.file_name()?.to_str()?;
    disk_of(name, 0)
}

fn disk_of(name: &str, depth: usize) -> Option<String> {
    let sys: PathBuf = fs::canonicalize(Path::new(SYS_CLASS_BLOCK).join(name)).ok()?;
    if sys.join("partition").exists() {
        // Partitions live in their disk's directory.
        return Some(sys.parent()?.file_name()?.to_str()?.to_owned());
    }
    // Device-mapper and RAID devices list what they're built on in
    // `slaves`; follow the first.
    let slave = fs::read_dir(sys.join("slaves"))
        .ok()
        .and_then(|mut entries| entries.next()?.ok()?.file_name().into_string().ok());
    match slave {
        Some(slave) if depth < MAX_STACK_DEPTH => disk_of(&slave, depth + 1),
        Some(_) => None,
        None => Some(name.to_owned()),
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}

fn read_u64(path: &Path) -> Option<u64> {
    read_trimmed(path)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stat() {
        let line = "  201889   102637 10727314    56609   163198   286970 18401562   378740        0    25743   439222        0        0        0        0    14623     3872";
        assert_eq!(
            Counters::parse(line),
            Some(Counters {
                reads: 201_889,
                read_sectors: 10_727_314,
                writes: 163_198,
                write_sectors: 18_401_562,
                busy_ms: 25_743,
            })
        );
        assert_eq!(Counters::parse("1 2 3"), None);
        assert_eq!(Counters::parse("x 2 3 4 5 6 7 8 9 10"), None);
    }

    #[test]
    fn filesystem_usage() {
        let fs = Filesystem {
            device: "/dev/sda1".into(),
            mount_point: "/".into(),
            fs_type: "ext4".into(),
            total: 400,
            available: 100,
            disk: Some("sda".into()),
        };
        assert_eq!(fs.used(), 300);
        assert!((fs.percent() - 75.0).abs() < f64::EPSILON);
    }
}
