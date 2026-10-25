//! Master boot record partition tables.

/// Partition start; 1 MiB keeps flash erase blocks and every cluster size aligned.
pub const PARTITION_ALIGN: u64 = 1024 * 1024;
/// `int 18h` (try the next boot device), then halt forever: legacy BIOS boot falls through.
pub const BOOT_STUB: [u8; 5] = [0xCD, 0x18, 0xF4, 0xEB, 0xFD];
pub const TYPE_NTFS: u8 = 0x07;
pub const TYPE_FAT32_LBA: u8 = 0x0C;
pub const TYPE_EFI_SYSTEM: u8 = 0xEF;
const ACTIVE: u8 = 0x80;
/// CHS placeholder for LBA-addressed partitions.
const CHS_LBA: [u8; 3] = [0xFE, 0xFF, 0xFF];
const TABLE_OFFSET: usize = 446;
const ENTRY_LEN: usize = 16;
pub const MAX_PARTITIONS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Partition {
    pub kind: u8,
    pub active: bool,
    pub start_lba: u32,
    pub sectors: u32,
}

/// Sector 0 with the boot stub, `disk_signature` (Windows needs it unique among
/// attached disks) and up to four primary partitions.
pub fn sector(sector_size: u64, disk_signature: u32, partitions: &[Partition]) -> Vec<u8> {
    assert!(partitions.len() <= MAX_PARTITIONS, "an MBR holds at most four primary partitions");
    let mut s = vec![0u8; sector_size as usize];
    s[..BOOT_STUB.len()].copy_from_slice(&BOOT_STUB);
    s[440..444].copy_from_slice(&disk_signature.to_le_bytes());
    for (i, p) in partitions.iter().enumerate() {
        let entry = &mut s[TABLE_OFFSET + i * ENTRY_LEN..TABLE_OFFSET + (i + 1) * ENTRY_LEN];
        entry[0] = if p.active { ACTIVE } else { 0 };
        entry[1..4].copy_from_slice(&CHS_LBA);
        entry[4] = p.kind;
        entry[5..8].copy_from_slice(&CHS_LBA);
        entry[8..12].copy_from_slice(&p.start_lba.to_le_bytes());
        entry[12..16].copy_from_slice(&p.sectors.to_le_bytes());
    }
    s[510] = 0x55;
    s[511] = 0xAA;
    s
}
