use std::io;
use std::path::{Path, PathBuf};

/// Bus a physical drive is attached through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusKind {
    Usb,
    /// SD / MMC card reader.
    Sd,
    Other,
}

/// A physical disk as reported by the platform backend.
#[derive(Debug, Clone)]
pub struct Drive {
    /// OS disk number (`N` in `\\.\PhysicalDriveN`).
    pub number: u32,
    /// Path used to open the raw device.
    pub path: String,
    pub model: String,
    pub size_bytes: u64,
    pub sector_size: u32,
    pub bus: BusKind,
    pub removable_media: bool,
    /// Mount points (drive letters / folders) of volumes on this disk.
    pub mount_points: Vec<String>,
    /// Hosts the running operating system.
    pub is_system: bool,
}

impl Drive {
    /// Only removable USB / SD drives that do not host the OS may be written.
    pub fn is_safe_target(&self) -> bool {
        !self.is_system && (self.removable_media || matches!(self.bus, BusKind::Usb | BusKind::Sd))
    }
}

/// Exclusive raw access to a disk.
///
/// Callers MUST pass offsets and lengths that are multiples of [`BlockDevice::sector_size`]
/// and buffers aligned to [`crate::BUFFER_ALIGN`].
pub trait BlockDevice {
    fn sector_size(&self) -> u32;
    fn size(&self) -> u64;
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()>;
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()>;
    /// Flush every written byte to the medium.
    fn sync(&mut self) -> io::Result<()>;
}

/// OS-specific disk access.
pub trait DiskBackend {
    /// Whether the process has the privileges raw disk access requires.
    fn is_elevated(&self) -> bool;
    /// All physical disks with media present.
    fn list_drives(&self) -> io::Result<Vec<Drive>>;
    /// Disk numbers backing the volume that contains `path` (empty for network paths).
    fn disk_numbers_for_path(&self, path: &Path) -> io::Result<Vec<u32>>;
    /// Unmount every volume on `drive` and open it for raw read/write.
    /// Volumes stay locked until the returned device is dropped.
    fn open_exclusive(&self, drive: &Drive) -> io::Result<Box<dyn BlockDevice>>;
    /// Format the partition starting at byte `partition_offset` of `drive` as NTFS with
    /// `label`, after a partition table naming it was written with [`Self::open_exclusive`].
    /// Returns a path to the mounted volume's root.
    fn format_ntfs(&self, drive: &Drive, partition_offset: u64, label: &str) -> io::Result<PathBuf>;
    /// Flush and unmount the volume mounted at `root`, so the next access reads the
    /// medium instead of cached data.
    fn dismount_volume(&self, root: &Path) -> io::Result<()>;
}
