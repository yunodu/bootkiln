//! Read-only ISO 9660 directory tree, with Rock Ridge or Joliet names when present.

use std::collections::HashSet;
use std::io::{self, Read, Seek, SeekFrom};

use crate::error::Error;

pub const SECTOR: u64 = 2048;
const FIRST_DESCRIPTOR: u64 = 16;
/// Volume descriptor sets are short; this bounds the scan on garbage input.
const MAX_DESCRIPTORS: u64 = 64;
const MAX_DEPTH: usize = 64;
/// Larger directories than this are corrupt rather than real.
const MAX_DIR_BYTES: u32 = 16 * 1024 * 1024;
const MAX_CONTINUATIONS: usize = 32;

const TYPE_PRIMARY: u8 = 1;
const TYPE_SUPPLEMENTARY: u8 = 2;
const TYPE_TERMINATOR: u8 = 255;
const JOLIET_ESCAPES: [&[u8; 3]; 3] = [b"%/@", b"%/C", b"%/E"];

const FLAG_ASSOCIATED: u8 = 0x04;
const FLAG_DIRECTORY: u8 = 0x02;
const FLAG_MULTI_EXTENT: u8 = 0x80;
const S_IFMT: u32 = 0o170000;
const S_IFLNK: u32 = 0o120000;

#[derive(Debug, Clone)]
pub struct IsoTree {
    /// Primary volume identifier: what Linux reports as the filesystem label.
    pub label: String,
    /// Volume creation date in directory-record form (all zero when unset).
    pub created: [u8; 7],
    pub root: Vec<IsoEntry>,
}

#[derive(Debug, Clone)]
pub struct IsoEntry {
    pub name: String,
    /// ISO 9660 recording date: years since 1900, month, day, hour, minute, second, GMT offset.
    pub recorded: [u8; 7],
    pub kind: IsoKind,
}

#[derive(Debug, Clone)]
pub enum IsoKind {
    Dir(Vec<IsoEntry>),
    /// Data extents in file order.
    File(Vec<Extent>),
    Symlink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    /// Byte offset in the image.
    pub offset: u64,
    pub len: u64,
}

impl IsoEntry {
    pub fn file_len(&self) -> u64 {
        match &self.kind {
            IsoKind::File(extents) => extents.iter().map(|e| e.len).sum(),
            _ => 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Naming {
    RockRidge { skip: usize },
    Joliet,
    Plain,
}

/// Read the directory tree; [`Error::NotIso9660`] if there is no primary volume descriptor.
pub fn read_tree<R: Read + Seek>(image: &mut R) -> Result<IsoTree, Error> {
    let mut primary = None;
    let mut joliet_root = None;
    for n in FIRST_DESCRIPTOR..FIRST_DESCRIPTOR + MAX_DESCRIPTORS {
        let mut d = [0u8; SECTOR as usize];
        if !read_exact_at(image, n * SECTOR, &mut d)? || &d[1..6] != b"CD001" {
            break;
        }
        match d[0] {
            TYPE_PRIMARY if primary.is_none() => primary = Some(d),
            TYPE_SUPPLEMENTARY if JOLIET_ESCAPES.iter().any(|e| &d[88..91] == *e) => {
                joliet_root = Some(root_record(&d)?);
            }
            TYPE_TERMINATOR => break,
            _ => {}
        }
    }
    let primary = primary.ok_or(Error::NotIso9660)?;
    let block_size = u16::from_le_bytes([primary[128], primary[129]]);
    if u64::from(block_size) != SECTOR {
        return Err(invalid(format!("logical block size {block_size} (only 2048 is supported)")));
    }
    let label = String::from_utf8_lossy(&primary[40..72]).trim_end_matches([' ', '\0']).to_owned();

    let primary_root = root_record(&primary)?;
    let (root, naming) = match rock_ridge_skip(image, primary_root)? {
        Some(skip) => (primary_root, Naming::RockRidge { skip }),
        None => match joliet_root {
            Some(root) => (root, Naming::Joliet),
            None => (primary_root, Naming::Plain),
        },
    };
    let mut reader = TreeReader { image, naming, visited: HashSet::new() };
    let root = reader.read_dir(root, 0)?;
    Ok(IsoTree { label, created: descriptor_date(&primary[813..830]), root })
}

/// Volume descriptor dates are ASCII "YYYYMMDDHHMMSScc" plus a GMT offset byte.
fn descriptor_date(field: &[u8]) -> [u8; 7] {
    let number = |range: std::ops::Range<usize>| -> Option<u16> { std::str::from_utf8(&field[range]).ok()?.parse().ok() };
    let parts = (number(0..4), number(4..6), number(6..8), number(8..10), number(10..12), number(12..14));
    match parts {
        (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) if year >= 1900 => {
            [(year - 1900).min(255) as u8, month as u8, day as u8, hour as u8, minute as u8, second as u8, 0]
        }
        _ => [0; 7],
    }
}

/// (extent LBA, data length) of a directory.
type DirLocation = (u32, u32);

fn root_record(descriptor: &[u8]) -> Result<DirLocation, Error> {
    let record = &descriptor[156..190];
    if record[25] & FLAG_DIRECTORY == 0 {
        return Err(invalid("root directory record is not a directory".into()));
    }
    Ok((le32(record, 2), le32(record, 10)))
}

/// Rock Ridge is announced by an "SP" entry in the root's "." record.
fn rock_ridge_skip<R: Read + Seek>(image: &mut R, (lba, _): DirLocation) -> Result<Option<usize>, Error> {
    let mut sector = [0u8; SECTOR as usize];
    if !read_exact_at(image, u64::from(lba) * SECTOR, &mut sector)? {
        return Err(invalid("root directory lies past the end of the image".into()));
    }
    let len = usize::from(sector[0]);
    if len < 34 || len > sector.len() {
        return Ok(None);
    }
    let record = &sector[..len];
    let su = system_use(record);
    Ok((su.len() >= 7 && &su[..2] == b"SP" && su[4..6] == [0xBE, 0xEF]).then(|| usize::from(su[6])))
}

struct TreeReader<'a, R> {
    image: &'a mut R,
    naming: Naming,
    visited: HashSet<u32>,
}

/// Rock Ridge facts about one directory record.
#[derive(Default)]
struct RockRidge {
    name: Option<Vec<u8>>,
    symlink: bool,
    relocated: bool,
    child_link: Option<u32>,
}

impl<R: Read + Seek> TreeReader<'_, R> {
    fn read_dir(&mut self, (lba, size): DirLocation, depth: usize) -> Result<Vec<IsoEntry>, Error> {
        if depth > MAX_DEPTH {
            return Err(invalid("directories nested too deeply".into()));
        }
        if size > MAX_DIR_BYTES {
            return Err(invalid(format!("directory at block {lba} claims {size} bytes")));
        }
        if !self.visited.insert(lba) {
            return Err(invalid(format!("directory at block {lba} is reachable twice")));
        }
        let mut data = vec![0u8; size as usize];
        if !read_exact_at(self.image, u64::from(lba) * SECTOR, &mut data)? {
            return Err(invalid(format!("directory at block {lba} lies past the end of the image")));
        }

        let mut entries = Vec::new();
        // Extents of a multi-extent file whose final record has not been seen yet.
        let mut pending: Option<(Vec<u8>, Vec<Extent>)> = None;
        let mut pos = 0usize;
        while pos < data.len() {
            let len = usize::from(data[pos]);
            if len == 0 {
                // Records never cross sector boundaries; the rest of this sector is padding.
                pos = (pos / SECTOR as usize + 1) * SECTOR as usize;
                continue;
            }
            let Some(record) = data.get(pos..pos + len).filter(|r| r.len() >= 34) else {
                return Err(invalid(format!("truncated directory record in block {lba}")));
            };
            pos += len;

            let name_len = usize::from(record[32]);
            let Some(raw_name) = record.get(33..33 + name_len) else {
                return Err(invalid(format!("directory record name overruns its record in block {lba}")));
            };
            if raw_name == [0] || raw_name == [1] {
                continue; // "." and ".."
            }
            let flags = record[25];
            if flags & FLAG_ASSOCIATED != 0 {
                continue;
            }
            if record[26] != 0 || record[27] != 0 {
                return Err(invalid("interleaved files are not supported".into()));
            }
            let rr = match self.naming {
                Naming::RockRidge { skip } => self.rock_ridge(record, skip)?,
                _ => RockRidge::default(),
            };
            if rr.relocated {
                continue; // Reachable through its child-link placeholder instead.
            }
            let name = match (&rr.name, self.naming) {
                (Some(name), _) => String::from_utf8_lossy(name).into_owned(),
                (None, Naming::Joliet) => plain_name(&decode_ucs2(raw_name)),
                (None, _) => plain_name(&String::from_utf8_lossy(raw_name)),
            };
            let recorded: [u8; 7] = record[18..25].try_into().expect("7-byte slice");
            let location = (le32(record, 2), le32(record, 10));

            if rr.symlink {
                entries.push(IsoEntry { name, recorded, kind: IsoKind::Symlink });
                continue;
            }
            if let Some(child_lba) = rr.child_link {
                let size = self.dot_record_size(child_lba)?;
                let children = self.read_dir((child_lba, size), depth + 1)?;
                entries.push(IsoEntry { name, recorded, kind: IsoKind::Dir(children) });
                continue;
            }
            if flags & FLAG_DIRECTORY != 0 {
                let children = self.read_dir(location, depth + 1)?;
                entries.push(IsoEntry { name, recorded, kind: IsoKind::Dir(children) });
                continue;
            }

            let extent = Extent { offset: u64::from(location.0) * SECTOR, len: u64::from(location.1) };
            let mut extents = match pending.take() {
                Some((pending_name, extents)) if pending_name == raw_name => extents,
                Some(_) => return Err(invalid(format!("multi-extent file in block {lba} is missing its last part"))),
                None => Vec::new(),
            };
            if extent.len > 0 {
                extents.push(extent);
            }
            if flags & FLAG_MULTI_EXTENT != 0 {
                pending = Some((raw_name.to_vec(), extents));
            } else {
                entries.push(IsoEntry { name, recorded, kind: IsoKind::File(extents) });
            }
        }
        if pending.is_some() {
            return Err(invalid(format!("multi-extent file in block {lba} is missing its last part")));
        }
        Ok(entries)
    }

    /// Size of the directory at `lba`, from its "." record.
    fn dot_record_size(&mut self, lba: u32) -> Result<u32, Error> {
        let mut record = [0u8; 34];
        if !read_exact_at(self.image, u64::from(lba) * SECTOR, &mut record)? {
            return Err(invalid(format!("relocated directory at block {lba} lies past the end of the image")));
        }
        Ok(le32(&record, 10))
    }

    fn rock_ridge(&mut self, record: &[u8], skip: usize) -> Result<RockRidge, Error> {
        let mut rr = RockRidge::default();
        let mut area = system_use(record).get(skip..).unwrap_or_default().to_vec();
        for _ in 0..MAX_CONTINUATIONS {
            let mut continuation = None;
            let mut pos = 0;
            while pos + 4 <= area.len() {
                let len = usize::from(area[pos + 2]);
                if len < 4 || pos + len > area.len() {
                    break;
                }
                let entry = &area[pos..pos + len];
                match &entry[..2] {
                    b"NM" if len >= 5 => rr.name.get_or_insert_with(Vec::new).extend_from_slice(&entry[5..]),
                    b"PX" if len >= 12 => rr.symlink |= le32(entry, 4) & S_IFMT == S_IFLNK,
                    b"SL" => rr.symlink = true,
                    b"RE" => rr.relocated = true,
                    b"CL" if len >= 8 => rr.child_link = Some(le32(entry, 4)),
                    b"CE" if len >= 28 => {
                        continuation = Some((le32(entry, 4), le32(entry, 12), le32(entry, 20)));
                    }
                    b"ST" => break,
                    _ => {}
                }
                pos += len;
            }
            let Some((block, offset, len)) = continuation else { return Ok(rr) };
            area = vec![0u8; len as usize];
            if !read_exact_at(self.image, u64::from(block) * SECTOR + u64::from(offset), &mut area)? {
                return Err(invalid(format!("Rock Ridge continuation at block {block} lies past the end of the image")));
            }
        }
        Err(invalid("Rock Ridge continuation chain too long".into()))
    }
}

/// System use area of a directory record (after the name and its padding byte).
fn system_use(record: &[u8]) -> &[u8] {
    let name_len = usize::from(record[32]);
    let start = 33 + name_len + usize::from(name_len.is_multiple_of(2));
    record.get(start..).unwrap_or_default()
}

/// Strip the ";1" version suffix and the trailing dot of extension-less names.
fn plain_name(name: &str) -> String {
    let name = name.split_once(';').map_or(name, |(base, _)| base);
    name.strip_suffix('.').unwrap_or(name).to_owned()
}

fn decode_ucs2(raw: &[u8]) -> String {
    let units: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|&c| u16::from_be_bytes(c)).collect();
    String::from_utf16_lossy(&units)
}

fn le32(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(buf[offset..offset + 4].try_into().expect("4-byte slice"))
}

fn invalid(message: String) -> Error {
    Error::InvalidIso(message)
}

/// Fills `buf` from `offset`; `false` if the image is too short.
fn read_exact_at<R: Read + Seek>(image: &mut R, offset: u64, buf: &mut [u8]) -> io::Result<bool> {
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
    use crate::testutil::{IsoBuilder, IsoNaming};
    use std::io::Cursor;

    fn names(entries: &[IsoEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    fn sample(naming: IsoNaming) -> Vec<u8> {
        let mut iso = IsoBuilder::new("My-Label", naming);
        let efi = iso.dir(None, "EFI");
        let boot = iso.dir(Some(efi), "BOOT");
        iso.file(Some(boot), "bootx64.efi", b"shim".to_vec());
        iso.file(None, "README.txt", vec![b'r'; 3000]);
        iso.file(None, "empty", Vec::new());
        iso.build()
    }

    fn file_bytes(img: &[u8], entry: &IsoEntry) -> Vec<u8> {
        let IsoKind::File(extents) = &entry.kind else { panic!("{} is not a file", entry.name) };
        extents.iter().flat_map(|e| img[e.offset as usize..(e.offset + e.len) as usize].to_vec()).collect()
    }

    #[test]
    fn reads_rock_ridge_names_and_file_extents() {
        let img = sample(IsoNaming::RockRidge);
        let tree = read_tree(&mut Cursor::new(&img)).unwrap();
        assert_eq!(tree.label, "My-Label");
        assert_eq!(names(&tree.root), ["EFI", "README.txt", "empty"]);

        let IsoKind::Dir(efi) = &tree.root[0].kind else { panic!("EFI is not a directory") };
        let IsoKind::Dir(boot) = &efi[0].kind else { panic!("BOOT is not a directory") };
        assert_eq!(names(boot), ["bootx64.efi"]);
        assert_eq!(file_bytes(&img, &boot[0]), b"shim");
        assert_eq!(file_bytes(&img, &tree.root[1]), vec![b'r'; 3000]);
        assert!(matches!(&tree.root[2].kind, IsoKind::File(e) if e.is_empty()));
    }

    #[test]
    fn falls_back_to_joliet_then_plain_names() {
        let tree = read_tree(&mut Cursor::new(sample(IsoNaming::Joliet))).unwrap();
        assert_eq!(names(&tree.root), ["EFI", "README.txt", "empty"]);

        // Plain ISO 9660 names are upper case 8.3 with a ";1" version.
        let tree = read_tree(&mut Cursor::new(sample(IsoNaming::Plain))).unwrap();
        assert_eq!(names(&tree.root), ["EFI", "README.TXT", "EMPTY"]);
    }

    #[test]
    fn joins_multi_extent_files() {
        let mut iso = IsoBuilder::new("X", IsoNaming::RockRidge);
        iso.multi_extent_file(None, "big.img", &[vec![1; 2048], vec![2; 2048], vec![3; 100]]);
        let img = iso.build();
        let tree = read_tree(&mut Cursor::new(&img)).unwrap();
        assert_eq!(names(&tree.root), ["big.img"]);
        assert_eq!(tree.root[0].file_len(), 4196);
        let expected: Vec<u8> = [vec![1; 2048], vec![2; 2048], vec![3; 100]].concat();
        assert_eq!(file_bytes(&img, &tree.root[0]), expected);
    }

    #[test]
    fn rock_ridge_symlinks_are_reported_not_followed() {
        let mut iso = IsoBuilder::new("X", IsoNaming::RockRidge);
        iso.symlink(None, "ubuntu");
        let tree = read_tree(&mut Cursor::new(iso.build())).unwrap();
        assert!(matches!(tree.root[0].kind, IsoKind::Symlink));
    }

    #[test]
    fn non_iso_input_is_not_iso9660() {
        let err = read_tree(&mut Cursor::new(vec![0u8; 64 * 1024])).unwrap_err();
        assert!(matches!(err, Error::NotIso9660), "{err:?}");
    }
}
