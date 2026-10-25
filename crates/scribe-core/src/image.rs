use std::io::{self, Read, Seek, SeekFrom};

/// How an image file can be put on a USB drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    /// Carries an MBR/GPT boot record (hybrid ISO or raw disk image): a raw copy boots.
    Hybrid,
    /// Hybrid ISO whose boot record only serves legacy BIOS: no GPT and no EFI system
    /// partition. A raw copy boots only on PCs with legacy boot (CSM) enabled.
    BiosOnly,
    /// Plain ISO 9660 / UDF optical image (e.g. Windows installers): a raw copy will not
    /// boot from USB; the files must be copied onto a formatted drive instead.
    OpticalOnly,
    /// Neither a boot record nor an ISO 9660 descriptor was found.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageInfo {
    pub len: u64,
    pub kind: ImageKind,
}

const MBR_SIGNATURE_OFFSET: u64 = 510;
const MBR_PARTITION_TABLE_OFFSET: u64 = 446;
const MBR_PARTITION_ENTRY_LEN: usize = 16;
/// Partition type byte inside an MBR partition entry.
const MBR_PARTITION_TYPE_OFFSET: usize = 4;
const MBR_TYPE_GPT_PROTECTIVE: u8 = 0xEE;
const MBR_TYPE_EFI_SYSTEM: u8 = 0xEF;
/// GPT header lives at LBA 1; hybrid ISOs use 512-byte LBAs.
const GPT_HEADER_OFFSET: u64 = 512;
const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";
/// ISO 9660 volume descriptors start at sector 16 (2048-byte sectors); bytes 1..6 are "CD001".
const ISO9660_DESCRIPTOR_OFFSET: u64 = 16 * 2048;

pub fn inspect<R: Read + Seek>(image: &mut R) -> io::Result<ImageInfo> {
    let len = image.seek(SeekFrom::End(0))?;

    let mut signature = [0u8; 2];
    let has_boot_record = read_at(image, MBR_SIGNATURE_OFFSET, &mut signature)? && signature == [0x55, 0xAA];

    let mut descriptor = [0u8; 6];
    let has_iso9660 = read_at(image, ISO9660_DESCRIPTOR_OFFSET, &mut descriptor)? && &descriptor[1..] == b"CD001";

    let kind = match (has_boot_record, has_iso9660) {
        // Only judged for hybrid ISOs: raw disk images may carry an ESP under an ordinary
        // FAT partition type, so their firmware support can't be read from the MBR alone.
        (true, true) if !has_uefi_boot_path(image)? => ImageKind::BiosOnly,
        (true, _) => ImageKind::Hybrid,
        (false, true) => ImageKind::OpticalOnly,
        (false, false) => ImageKind::Unknown,
    };
    image.seek(SeekFrom::Start(0))?;
    Ok(ImageInfo { len, kind })
}

/// UEFI firmware boots a USB stick from an EFI system partition, found via a GPT or an
/// MBR entry of type 0xEF.
fn has_uefi_boot_path<R: Read + Seek>(image: &mut R) -> io::Result<bool> {
    let mut gpt_signature = [0u8; 8];
    if read_at(image, GPT_HEADER_OFFSET, &mut gpt_signature)? && &gpt_signature == GPT_SIGNATURE {
        return Ok(true);
    }
    let mut table = [0u8; 4 * MBR_PARTITION_ENTRY_LEN];
    if !read_at(image, MBR_PARTITION_TABLE_OFFSET, &mut table)? {
        return Ok(false);
    }
    let (entries, _) = table.as_chunks::<MBR_PARTITION_ENTRY_LEN>();
    Ok(entries
        .iter()
        .any(|entry| matches!(entry[MBR_PARTITION_TYPE_OFFSET], MBR_TYPE_GPT_PROTECTIVE | MBR_TYPE_EFI_SYSTEM)))
}

/// Fills `buf` from `offset`; `false` if the image is too short.
fn read_at<R: Read + Seek>(image: &mut R, offset: u64, buf: &mut [u8]) -> io::Result<bool> {
    image.seek(SeekFrom::Start(offset))?;
    match image.read_exact(buf) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn image(boot_record: bool, iso9660: bool) -> Cursor<Vec<u8>> {
        let mut data = vec![0u8; 64 * 1024];
        if boot_record {
            data[510] = 0x55;
            data[511] = 0xAA;
        }
        if iso9660 {
            data[0x8000] = 1;
            data[0x8001..0x8006].copy_from_slice(b"CD001");
        }
        Cursor::new(data)
    }

    fn with_partition_type(mut img: Cursor<Vec<u8>>, slot: usize, part_type: u8) -> Cursor<Vec<u8>> {
        img.get_mut()[446 + slot * 16 + 4] = part_type;
        img
    }

    #[test]
    fn classifies_by_boot_record_then_iso9660() {
        assert_eq!(inspect(&mut image(true, false)).unwrap().kind, ImageKind::Hybrid);
        assert_eq!(inspect(&mut image(false, true)).unwrap().kind, ImageKind::OpticalOnly);
        assert_eq!(inspect(&mut image(false, false)).unwrap().kind, ImageKind::Unknown);
    }

    #[test]
    fn hybrid_iso_needs_gpt_or_esp_for_uefi() {
        // isohybrid without --uefi (e.g. Tiny Core): one 0x17 partition.
        let bios_only = with_partition_type(image(true, true), 0, 0x17);
        assert_eq!(inspect(&mut bios_only.clone()).unwrap().kind, ImageKind::BiosOnly);

        // ESP in any MBR slot (isohybrid --uefi puts it in slot 2).
        let mut esp = with_partition_type(bios_only.clone(), 1, 0xEF);
        assert_eq!(inspect(&mut esp).unwrap().kind, ImageKind::Hybrid);

        let mut protective = with_partition_type(image(true, true), 0, 0xEE);
        assert_eq!(inspect(&mut protective).unwrap().kind, ImageKind::Hybrid);

        // GPT header without a protective MBR entry (hybrid MBR layouts).
        let mut gpt = bios_only;
        gpt.get_mut()[512..520].copy_from_slice(b"EFI PART");
        assert_eq!(inspect(&mut gpt).unwrap().kind, ImageKind::Hybrid);
    }

    #[test]
    fn raw_disk_image_is_not_judged_by_partition_types() {
        let mut fat32 = with_partition_type(image(true, false), 0, 0x0C);
        assert_eq!(inspect(&mut fat32).unwrap().kind, ImageKind::Hybrid);
    }

    #[test]
    fn short_file_is_unknown_and_stream_is_rewound() {
        let mut img = Cursor::new(vec![0u8; 100]);
        let info = inspect(&mut img).unwrap();
        assert_eq!(info, ImageInfo { len: 100, kind: ImageKind::Unknown });
        assert_eq!(img.position(), 0);
    }
}
