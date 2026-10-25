//! Read-only UDF directory tree (revisions 1.02-2.01 on physical partitions), the file
//! system of Windows installer ISOs.

use std::collections::HashSet;
use std::io::{Read, Seek};

use crate::disc::{DiscEntry, DiscKind, DiscTree, Extent, SECTOR, read_exact_at};
use crate::error::Error;

const FIRST_RECOGNITION_SECTOR: u64 = 16;
/// Bounds the volume recognition scan on garbage input.
const MAX_RECOGNITION_SECTORS: u64 = 64;
const ANCHOR_SECTOR: u64 = 256;
const MAX_DEPTH: usize = 64;
const MAX_DIR_BYTES: u64 = 16 * 1024 * 1024;
/// Volume descriptor sequences are a few sectors; more than this is corrupt.
const MAX_DESCRIPTORS: u32 = 256;
const MAX_ALLOCATION_CONTINUATIONS: usize = 1024;

const TAG_PRIMARY_VOLUME: u16 = 1;
const TAG_ANCHOR: u16 = 2;
const TAG_PARTITION: u16 = 5;
const TAG_LOGICAL_VOLUME: u16 = 6;
const TAG_TERMINATOR: u16 = 8;
const TAG_FILE_SET: u16 = 256;
const TAG_FILE_IDENTIFIER: u16 = 257;
const TAG_ALLOCATION_EXTENT: u16 = 258;
const TAG_FILE_ENTRY: u16 = 261;
const TAG_EXTENDED_FILE_ENTRY: u16 = 266;

const FILE_TYPE_DIRECTORY: u8 = 4;
const FILE_TYPE_REGULAR: u8 = 5;
const FILE_TYPE_SYMLINK: u8 = 12;
const CHAR_DELETED: u8 = 0x04;
const CHAR_PARENT: u8 = 0x08;

const AD_SHORT: u16 = 0;
const AD_LONG: u16 = 1;
const AD_EMBEDDED: u16 = 3;
const EXTENT_RECORDED: u32 = 0;
const EXTENT_CONTINUATION: u32 = 3;
const EXTENT_LENGTH_MASK: u32 = 0x3FFF_FFFF;

/// Whether the volume recognition sequence contains an NSR (UDF) descriptor.
pub fn is_udf<R: Read + Seek>(image: &mut R) -> Result<bool, Error> {
    for n in FIRST_RECOGNITION_SECTOR..FIRST_RECOGNITION_SECTOR + MAX_RECOGNITION_SECTORS {
        let mut id = [0u8; 6];
        if !read_exact_at(image, n * SECTOR, &mut id)? {
            return Ok(false);
        }
        match &id[1..6] {
            b"NSR02" | b"NSR03" => return Ok(true),
            b"CD001" | b"BEA01" | b"BOOT2" | b"CDW02" => {}
            _ => return Ok(false),
        }
    }
    Ok(false)
}

/// Long allocation descriptor target: logical block in a partition (by map index).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct BlockAddress {
    block: u32,
    partition: u16,
}

pub fn read_tree<R: Read + Seek>(image: &mut R) -> Result<DiscTree, Error> {
    let anchor = read_block(image, ANCHOR_SECTOR)?;
    if tag(&anchor) != TAG_ANCHOR {
        return Err(invalid("UDF anchor volume descriptor missing at sector 256".into()));
    }
    let (sequence_len, sequence_at) = (le32(&anchor, 16), le32(&anchor, 20));

    let mut partitions: Vec<(u16, u32)> = Vec::new(); // (partition number, start sector)
    let mut logical_volume = None;
    let mut volume_id = String::new();
    for i in 0..(sequence_len / SECTOR as u32).min(MAX_DESCRIPTORS) {
        let d = read_block(image, u64::from(sequence_at + i))?;
        match tag(&d) {
            TAG_PRIMARY_VOLUME => volume_id = dstring(&d[24..56]),
            TAG_PARTITION => partitions.push((le16(&d, 22), le32(&d, 188))),
            TAG_LOGICAL_VOLUME => logical_volume = Some(d),
            TAG_TERMINATOR => break,
            _ => {}
        }
    }
    let lvd = logical_volume.ok_or_else(|| invalid("UDF logical volume descriptor missing".into()))?;
    let block_size = le32(&lvd, 212);
    if u64::from(block_size) != SECTOR {
        return Err(invalid(format!("UDF block size {block_size} (only 2048 is supported)")));
    }

    // Partition maps translate a long_ad partition reference into a physical start sector.
    let map_count = le32(&lvd, 268);
    let mut starts = Vec::new();
    let mut pos = 440;
    for _ in 0..map_count {
        let (kind, len) = (lvd[pos], usize::from(lvd[pos + 1]));
        if kind != 1 || len != 6 {
            return Err(invalid("UDF virtual, sparable or metadata partitions are not supported".into()));
        }
        let number = le16(&lvd, pos + 4);
        let start = partitions
            .iter()
            .find(|(n, _)| *n == number)
            .map(|(_, s)| *s)
            .ok_or_else(|| invalid(format!("UDF partition {number} has no partition descriptor")))?;
        starts.push(start);
        pos += len;
        if pos + 2 > lvd.len() {
            break;
        }
    }

    let label = match dstring(&lvd[84..212]) {
        id if id.is_empty() => volume_id,
        id => id,
    };
    let mut reader = Reader { image, starts, visited: HashSet::new() };
    let file_set = reader.read_at(long_ad(&lvd, 248))?;
    if tag(&file_set) != TAG_FILE_SET {
        return Err(invalid("UDF file set descriptor missing".into()));
    }
    let root_entry = reader.read_entry(long_ad(&file_set, 400))?;
    let root = reader.read_dir(&root_entry, 0)?;
    Ok(DiscTree { label, created: root_entry.modified, root })
}

struct Reader<'a, R> {
    image: &'a mut R,
    /// Physical start sector per partition map index.
    starts: Vec<u32>,
    visited: HashSet<BlockAddress>,
}

/// The parts of a (extended) file entry we use.
struct Entry {
    file_type: u8,
    modified: [u8; 7],
    extents: Vec<Extent>,
}

impl<R: Read + Seek> Reader<'_, R> {
    fn byte_offset(&self, at: BlockAddress) -> Result<u64, Error> {
        let start = self
            .starts
            .get(usize::from(at.partition))
            .ok_or_else(|| invalid(format!("UDF partition reference {} out of range", at.partition)))?;
        Ok((u64::from(*start) + u64::from(at.block)) * SECTOR)
    }

    fn read_at(&mut self, at: BlockAddress) -> Result<Vec<u8>, Error> {
        let offset = self.byte_offset(at)?;
        read_block(self.image, offset / SECTOR)
    }

    fn read_entry(&mut self, at: BlockAddress) -> Result<Entry, Error> {
        let block = self.read_at(at)?;
        let (ea_len_at, base, modified_at) = match tag(&block) {
            TAG_FILE_ENTRY => (168, 176, 84),
            TAG_EXTENDED_FILE_ENTRY => (208, 216, 92),
            other => return Err(invalid(format!("UDF block {} is not a file entry (tag {other})", at.block))),
        };
        let file_type = block[16 + 11];
        let ad_type = le16(&block, 16 + 18) & 7;
        let info_len = le64(&block, 56);
        let ads_start = base + le32(&block, ea_len_at) as usize;
        let ads_len = le32(&block, ea_len_at + 4) as usize;
        let ads = block
            .get(ads_start..ads_start + ads_len)
            .ok_or_else(|| invalid(format!("UDF file entry at block {} overruns its block", at.block)))?
            .to_vec();

        let mut extents = match ad_type {
            AD_EMBEDDED => {
                let offset = self.byte_offset(at)? + ads_start as u64;
                vec![Extent { offset, len: info_len.min(ads_len as u64) }]
            }
            AD_SHORT | AD_LONG => self.allocation_extents(ads, ad_type, at.partition)?,
            _ => return Err(invalid("UDF extended allocation descriptors are not supported".into())),
        };
        trim(&mut extents, info_len);
        Ok(Entry { file_type, modified: timestamp(&block[modified_at..modified_at + 12]), extents })
    }

    fn allocation_extents(&mut self, mut ads: Vec<u8>, ad_type: u16, partition: u16) -> Result<Vec<Extent>, Error> {
        let size = if ad_type == AD_SHORT { 8 } else { 16 };
        let mut extents = Vec::new();
        for _ in 0..MAX_ALLOCATION_CONTINUATIONS {
            let mut continuation = None;
            for ad in ads.chunks_exact(size) {
                let raw = le32(ad, 0);
                let len = raw & EXTENT_LENGTH_MASK;
                if len == 0 {
                    break;
                }
                let at = match ad_type {
                    AD_SHORT => BlockAddress { block: le32(ad, 4), partition },
                    _ => long_ad(ad, 0),
                };
                match raw >> 30 {
                    EXTENT_RECORDED => extents.push(Extent { offset: self.byte_offset(at)?, len: u64::from(len) }),
                    EXTENT_CONTINUATION => continuation = Some(at),
                    _ => return Err(invalid("UDF sparse (unrecorded) extents are not supported".into())),
                }
            }
            let Some(at) = continuation else { return Ok(extents) };
            let block = self.read_at(at)?;
            if tag(&block) != TAG_ALLOCATION_EXTENT {
                return Err(invalid(format!("UDF allocation extent missing at block {}", at.block)));
            }
            let len = le32(&block, 20) as usize;
            ads = block.get(24..24 + len).ok_or_else(|| invalid("UDF allocation extent overruns its block".into()))?.to_vec();
        }
        Err(invalid("UDF allocation extent chain too long".into()))
    }

    fn read_dir(&mut self, dir: &Entry, depth: usize) -> Result<Vec<DiscEntry>, Error> {
        if depth > MAX_DEPTH {
            return Err(invalid("directories nested too deeply".into()));
        }
        let size: u64 = dir.extents.iter().map(|e| e.len).sum();
        if size > MAX_DIR_BYTES {
            return Err(invalid(format!("UDF directory claims {size} bytes")));
        }
        let mut data = Vec::with_capacity(size as usize);
        for extent in &dir.extents {
            let start = data.len();
            data.resize(start + extent.len as usize, 0);
            if !read_exact_at(self.image, extent.offset, &mut data[start..])? {
                return Err(invalid("UDF directory lies past the end of the image".into()));
            }
        }

        let mut entries = Vec::new();
        let mut pos = 0;
        while pos + 38 <= data.len() {
            let fid = &data[pos..];
            if tag(fid) != TAG_FILE_IDENTIFIER {
                return Err(invalid("UDF directory holds a non-file-identifier record".into()));
            }
            let characteristics = fid[18];
            let name_len = usize::from(fid[19]);
            let implementation_len = usize::from(le16(fid, 36));
            let name_start = 38 + implementation_len;
            let record_len = (name_start + name_len).next_multiple_of(4);
            let Some(raw_name) = fid.get(name_start..name_start + name_len) else {
                return Err(invalid("UDF file identifier overruns its directory".into()));
            };
            let at = long_ad(fid, 20);
            pos += record_len;
            if characteristics & (CHAR_DELETED | CHAR_PARENT) != 0 {
                continue;
            }
            let name = cs0(raw_name);
            if !self.visited.insert(at) {
                return Err(invalid(format!("UDF entry {name} is reachable twice")));
            }
            let entry = self.read_entry(at)?;
            let kind = match entry.file_type {
                FILE_TYPE_DIRECTORY => DiscKind::Dir(self.read_dir(&entry, depth + 1)?),
                FILE_TYPE_REGULAR => DiscKind::File(entry.extents),
                FILE_TYPE_SYMLINK => DiscKind::Symlink,
                _ => continue, // Streams, devices and other special files have no data to copy.
            };
            entries.push(DiscEntry { name, recorded: entry.modified, kind });
        }
        Ok(entries)
    }
}

/// Drop bytes past the file's information length (allocations round up to blocks).
fn trim(extents: &mut Vec<Extent>, info_len: u64) {
    let mut remaining = info_len;
    extents.retain_mut(|e| {
        e.len = e.len.min(remaining);
        remaining -= e.len;
        e.len > 0
    });
}

fn read_block<R: Read + Seek>(image: &mut R, sector: u64) -> Result<Vec<u8>, Error> {
    let mut block = vec![0u8; SECTOR as usize];
    if !read_exact_at(image, sector * SECTOR, &mut block)? {
        return Err(invalid(format!("UDF block at sector {sector} lies past the end of the image")));
    }
    Ok(block)
}

fn tag(d: &[u8]) -> u16 {
    le16(d, 0)
}

fn long_ad(d: &[u8], offset: usize) -> BlockAddress {
    BlockAddress { block: le32(d, offset + 4), partition: le16(d, offset + 8) }
}

/// OSTA compressed Unicode: compression id 8 (one byte per char) or 16 (UTF-16BE).
fn cs0(raw: &[u8]) -> String {
    match raw.split_first() {
        Some((16, rest)) => {
            let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|&c| u16::from_be_bytes(c)).collect();
            String::from_utf16_lossy(&units)
        }
        Some((_, rest)) => rest.iter().map(|&b| char::from(b)).collect(),
        None => String::new(),
    }
}

/// Fixed-size dstring: CS0 text followed by its used length in the last byte.
fn dstring(field: &[u8]) -> String {
    let used = usize::from(*field.last().unwrap_or(&0)).min(field.len() - 1);
    cs0(&field[..used])
}

/// UDF timestamp (type/zone, year, month, day, hour, minute, second, ...) in
/// directory-record form.
fn timestamp(t: &[u8]) -> [u8; 7] {
    let year = le16(t, 2);
    [year.saturating_sub(1900).min(255) as u8, t[4], t[5], t[6], t[7], t[8], 0]
}

fn le16(d: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([d[offset], d[offset + 1]])
}

fn le32(d: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(d[offset..offset + 4].try_into().expect("4-byte slice"))
}

fn le64(d: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(d[offset..offset + 8].try_into().expect("8-byte slice"))
}

fn invalid(message: String) -> Error {
    Error::InvalidIso(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disc;
    use crate::testutil::{IsoBuilder, IsoNaming, UdfBuilder, UdfFileOpts, pattern};
    use std::io::Cursor;

    fn bytes(img: &[u8], entry: &DiscEntry) -> Vec<u8> {
        let DiscKind::File(extents) = &entry.kind else { panic!("{} is not a file", entry.name) };
        extents.iter().flat_map(|e| img[e.offset as usize..(e.offset + e.len) as usize].to_vec()).collect()
    }

    #[test]
    fn reads_every_entry_and_allocation_form() {
        let mut udf = UdfBuilder::new("CCCOMA_X64FRE_EN-US_DV9");
        let sources = udf.dir(None, "sources");
        let efi = udf.dir(None, "efi");
        let boot = udf.dir(Some(efi), "boot");
        udf.file(Some(boot), "bootx64.efi", pattern(5000), UdfFileOpts::default());
        let long = UdfFileOpts { extended: true, long_ad: true, extent_blocks: Some(2), ..Default::default() };
        udf.file(Some(sources), "install.wim", pattern(20_000), long);
        udf.file(None, "autorun.inf", b"[AutoRun]".to_vec(), UdfFileOpts { embedded: true, ..Default::default() });
        udf.file(None, "Ünïcode ✓.txt", b"x".to_vec(), UdfFileOpts::default());
        udf.deleted(None, "gone.txt");
        let img = udf.build();

        assert!(is_udf(&mut Cursor::new(&img)).unwrap());
        let tree = disc::read_tree(&mut Cursor::new(&img)).unwrap();
        assert_eq!(tree.label, "CCCOMA_X64FRE_EN-US_DV9");
        let names: Vec<&str> = tree.root.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["sources", "efi", "autorun.inf", "Ünïcode ✓.txt"]);

        let wim = disc::find(&tree.root, &["sources", "install.wim"]).unwrap();
        assert!(matches!(&wim.kind, DiscKind::File(extents) if extents.len() == 5));
        assert_eq!(bytes(&img, wim), pattern(20_000));
        assert_eq!(bytes(&img, disc::find(&tree.root, &["EFI", "BOOT", "BOOTX64.EFI"]).unwrap()), pattern(5000));
        assert_eq!(bytes(&img, &tree.root[2]), b"[AutoRun]");
        assert_eq!(bytes(&img, &tree.root[3]), b"x");
        assert_eq!(wim.recorded, [125, 1, 2, 3, 4, 5, 0]);
    }

    #[test]
    fn iso9660_only_images_are_not_udf() {
        let iso = IsoBuilder::new("X", IsoNaming::RockRidge).build();
        assert!(!is_udf(&mut Cursor::new(&iso)).unwrap());
        assert!(!is_udf(&mut Cursor::new(vec![0u8; 64 * 1024])).unwrap());
    }
}
