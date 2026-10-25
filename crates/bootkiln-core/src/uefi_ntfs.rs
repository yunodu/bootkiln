//! Partition layout for NTFS drives: an NTFS data partition plus a small EFI system
//! partition holding UEFI:NTFS, which lets UEFI firmware boot from the NTFS partition.
//!
//! bootkiln writes only the partition table and the UEFI:NTFS partition; the platform
//! formats the NTFS partition and the files are copied through the file system.

use std::io;

use crate::error::Error;
use crate::mbr::{self, PARTITION_ALIGN};
use crate::plan::{Plan, Source};

/// UEFI:NTFS FAT partition image from Rufus; see `assets/README.md`.
pub const IMAGE: &[u8] = include_bytes!("../assets/uefi-ntfs.img");
/// The image is a FAT volume built with 512-byte sectors.
const IMAGE_SECTOR: u64 = 512;
/// Partition number (1-based, as Windows counts) of the NTFS partition.
pub const NTFS_PARTITION_NUMBER: u32 = 1;
/// Zeroed at the start of the NTFS partition so no stale file system is mounted before
/// the platform formats it.
const STALE_WIPE: u64 = 1024 * 1024;
/// The smallest NTFS partition we create; anything smaller is not a real USB drive.
const MIN_NTFS_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct NtfsLayout {
    /// Partition table, the UEFI:NTFS partition and wipes of stale metadata.
    pub plan: Plan,
    pub ntfs_offset: u64,
    pub ntfs_len: u64,
}

pub fn layout(disk_size: u64, sector_size: u32, disk_signature: u32) -> Result<NtfsLayout, Error> {
    let sector = u64::from(sector_size);
    if sector != IMAGE_SECTOR {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("the UEFI:NTFS partition image needs 512-byte sectors; this drive has {sector}"),
        )
        .into());
    }
    let esp_len = IMAGE.len() as u64;
    // MBR fields are 32-bit sector counts; keep everything below 2 TiB.
    let usable = (disk_size / sector * sector).min(u64::from(u32::MAX) * sector);
    let esp_offset = (usable - usable % PARTITION_ALIGN).saturating_sub(esp_len);
    let ntfs_offset = PARTITION_ALIGN;
    let ntfs_len = esp_offset.saturating_sub(ntfs_offset);
    if ntfs_len < MIN_NTFS_BYTES {
        return Err(Error::DriveTooSmall { needed: MIN_NTFS_BYTES + esp_len + PARTITION_ALIGN, available: disk_size });
    }

    let lba = |bytes: u64| (bytes / sector) as u32;
    let partitions = [
        mbr::Partition { kind: mbr::TYPE_NTFS, active: true, start_lba: lba(ntfs_offset), sectors: lba(ntfs_len) },
        mbr::Partition { kind: mbr::TYPE_EFI_SYSTEM, active: false, start_lba: lba(esp_offset), sectors: lba(esp_len) },
    ];
    let disk_end = disk_size / sector * sector;
    let esp_end = esp_offset + esp_len;
    let plan = Plan::new(vec![
        (0, Source::Bytes(mbr::sector(sector, disk_signature, &partitions))),
        (sector, Source::Zeros(ntfs_offset - sector)),
        (ntfs_offset, Source::Zeros(STALE_WIPE.min(ntfs_len))),
        (esp_offset, Source::Bytes(IMAGE.to_vec())),
        // Clears a stale backup GPT header in the last sector.
        (esp_end, Source::Zeros(disk_end - esp_end)),
    ]);
    Ok(NtfsLayout { plan, ntfs_offset, ntfs_len })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::write_plan;
    use crate::testutil::MemDisk;
    use std::io::Cursor;

    fn entry(disk: &[u8], i: usize) -> (u8, u8, u32, u32) {
        let e = &disk[446 + i * 16..462 + i * 16];
        let le = |at: usize| u32::from_le_bytes(e[at..at + 4].try_into().unwrap());
        (e[0], e[4], le(8), le(12))
    }

    #[test]
    fn ntfs_then_uefi_ntfs_partition_at_the_aligned_end() {
        // Not a MiB multiple, so the tail past the last aligned MiB must be wiped too.
        let size = 300 * 1024 * 1024 + 512 * 7;
        let layout = layout(size as u64, 512, 0xABCD).unwrap();
        let mut disk = MemDisk::new(size, 512);
        write_plan(&mut Cursor::new(Vec::new()), &layout.plan, &mut disk, &mut |_| {}).unwrap();

        let esp_offset = 299 * 1024 * 1024;
        assert_eq!(layout.ntfs_offset, 1024 * 1024);
        assert_eq!(layout.ntfs_offset + layout.ntfs_len, esp_offset as u64);
        assert_eq!(entry(&disk.data, 0), (0x80, mbr::TYPE_NTFS, 2048, (layout.ntfs_len / 512) as u32));
        assert_eq!(entry(&disk.data, 1), (0, mbr::TYPE_EFI_SYSTEM, (esp_offset / 512) as u32, 2048));
        assert_eq!(&disk.data[esp_offset..esp_offset + IMAGE.len()], IMAGE);
        assert!(disk.data[esp_offset + IMAGE.len()..].iter().all(|&b| b == 0), "tail wiped");
        assert!(disk.data[512..2 * 1024 * 1024].iter().all(|&b| b == 0), "gap and stale NTFS start wiped");
    }

    #[test]
    fn non_512_byte_sectors_are_refused() {
        let err = layout(1 << 30, 4096, 1).unwrap_err();
        assert!(matches!(&err, Error::Io(e) if e.kind() == io::ErrorKind::Unsupported), "{err:?}");
    }
}
