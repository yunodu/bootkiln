//! Directory tree of an optical disc image, read through UDF when the image has it
//! (Windows installers) and ISO 9660 otherwise.

use std::io::{self, Read, Seek, SeekFrom};

use crate::error::Error;
use crate::{iso9660, udf};

/// Logical block size of every optical image we read.
pub const SECTOR: u64 = 2048;

#[derive(Debug, Clone)]
pub struct DiscTree {
    /// Volume label as Linux reports it (ISO 9660 primary volume id / UDF logical volume id).
    pub label: String,
    /// Volume date in directory-record form (all zero when unset).
    pub created: [u8; 7],
    pub root: Vec<DiscEntry>,
}

#[derive(Debug, Clone)]
pub struct DiscEntry {
    pub name: String,
    /// Recording date: years since 1900, month, day, hour, minute, second, GMT offset.
    pub recorded: [u8; 7],
    pub kind: DiscKind,
}

#[derive(Debug, Clone)]
pub enum DiscKind {
    Dir(Vec<DiscEntry>),
    /// Data extents in file order.
    File(Vec<Extent>),
    /// Symbolic link, with its target path when the image records one (Rock Ridge).
    Symlink(Option<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    /// Byte offset in the image.
    pub offset: u64,
    pub len: u64,
}

impl DiscEntry {
    pub fn file_len(&self) -> u64 {
        match &self.kind {
            DiscKind::File(extents) => extents.iter().map(|e| e.len).sum(),
            _ => 0,
        }
    }
}

/// Look up `path` (components matched case-insensitively) below `entries`.
pub fn find<'a>(entries: &'a [DiscEntry], path: &[&str]) -> Option<&'a DiscEntry> {
    let (first, rest) = path.split_first()?;
    let entry = entries.iter().find(|e| e.name.eq_ignore_ascii_case(first))?;
    match (rest.is_empty(), &entry.kind) {
        (true, _) => Some(entry),
        (false, DiscKind::Dir(children)) => find(children, rest),
        (false, _) => None,
    }
}

/// UDF when the volume recognition sequence announces it, else ISO 9660;
/// [`Error::NotIso9660`] when the image is neither.
pub fn read_tree<R: Read + Seek>(image: &mut R) -> Result<DiscTree, Error> {
    if udf::is_udf(image)? { udf::read_tree(image) } else { iso9660::read_tree(image) }
}

/// Fills `buf` from `offset`; `false` if the image is too short.
pub(crate) fn read_exact_at<R: Read + Seek>(image: &mut R, offset: u64, buf: &mut [u8]) -> io::Result<bool> {
    image.seek(SeekFrom::Start(offset))?;
    match image.read_exact(buf) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e),
    }
}
