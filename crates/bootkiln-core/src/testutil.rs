//! Test doubles shared by the unit tests.

use std::io;

use crate::BUFFER_ALIGN;
use crate::device::BlockDevice;

/// In-memory disk that enforces the `BlockDevice` alignment contract.
pub struct MemDisk {
    pub data: Vec<u8>,
    pub sector: u32,
    pub synced: bool,
}

impl MemDisk {
    /// Filled with 0xEE so untouched bytes are recognisable.
    pub fn new(size: usize, sector: u32) -> Self {
        Self { data: vec![0xEE; size], sector, synced: false }
    }

    fn check(&self, offset: u64, ptr: *const u8, len: usize) -> std::ops::Range<usize> {
        let s = self.sector as u64;
        assert_eq!(offset % s, 0, "unaligned offset");
        assert_eq!(len as u64 % s, 0, "unaligned length");
        assert_eq!(ptr as usize % BUFFER_ALIGN, 0, "unaligned buffer");
        offset as usize..offset as usize + len
    }
}

impl BlockDevice for MemDisk {
    fn sector_size(&self) -> u32 {
        self.sector
    }
    fn size(&self) -> u64 {
        self.data.len() as u64
    }
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()> {
        let r = self.check(offset, buf.as_ptr(), buf.len());
        self.data[r].copy_from_slice(buf);
        Ok(())
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let r = self.check(offset, buf.as_ptr(), buf.len());
        buf.copy_from_slice(&self.data[r]);
        Ok(())
    }
    fn sync(&mut self) -> io::Result<()> {
        self.synced = true;
        Ok(())
    }
}

/// Deterministic non-repeating-looking bytes.
pub fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + i / 251) as u8).collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IsoNaming {
    RockRidge,
    /// Upper-case ISO 9660 names in the primary tree, original names in a Joliet tree.
    Joliet,
    Plain,
}

enum IsoNode {
    Dir { name: String, parent: Option<usize> },
    File { name: String, parent: Option<usize>, parts: Vec<Vec<u8>> },
    Symlink { name: String, parent: Option<usize> },
}

impl IsoNode {
    fn parent(&self) -> Option<usize> {
        match self {
            IsoNode::Dir { parent, .. } | IsoNode::File { parent, .. } | IsoNode::Symlink { parent, .. } => *parent,
        }
    }
}

/// Minimal ISO 9660 writer for tests; every directory must fit in one 2048-byte sector.
pub struct IsoBuilder {
    label: String,
    naming: IsoNaming,
    nodes: Vec<IsoNode>,
}

const ISO_SECTOR: usize = 2048;
const ISO_DATE: [u8; 7] = [125, 1, 2, 3, 4, 5, 0];

impl IsoBuilder {
    pub fn new(label: &str, naming: IsoNaming) -> Self {
        Self { label: label.to_owned(), naming, nodes: Vec::new() }
    }

    /// Returns the directory's handle for use as `parent`; `None` is the root.
    pub fn dir(&mut self, parent: Option<usize>, name: &str) -> usize {
        self.nodes.push(IsoNode::Dir { name: name.to_owned(), parent });
        self.nodes.len() - 1
    }

    pub fn file(&mut self, parent: Option<usize>, name: &str, data: Vec<u8>) {
        self.nodes.push(IsoNode::File { name: name.to_owned(), parent, parts: vec![data] });
    }

    /// All parts but the last must be whole sectors.
    pub fn multi_extent_file(&mut self, parent: Option<usize>, name: &str, parts: &[Vec<u8>]) {
        self.nodes.push(IsoNode::File { name: name.to_owned(), parent, parts: parts.to_vec() });
    }

    pub fn symlink(&mut self, parent: Option<usize>, name: &str) {
        assert!(self.naming == IsoNaming::RockRidge, "symlinks need Rock Ridge");
        self.nodes.push(IsoNode::Symlink { name: name.to_owned(), parent });
    }

    pub fn build(&self) -> Vec<u8> {
        // Sectors: 16 PVD, 17 Joliet SVD or terminator, 18 terminator, then directories, then files.
        let dirs: Vec<Option<usize>> =
            std::iter::once(None).chain((0..self.nodes.len()).filter(|&i| matches!(self.nodes[i], IsoNode::Dir { .. })).map(Some)).collect();
        let trees = if self.naming == IsoNaming::Joliet { 2 } else { 1 };
        let first_dir = 19;
        let mut next = first_dir + dirs.len() * trees;
        let mut part_lba: Vec<Vec<usize>> = Vec::new();
        for node in &self.nodes {
            let mut lbas = Vec::new();
            if let IsoNode::File { parts, .. } = node {
                for part in parts {
                    lbas.push(next);
                    next += part.len().div_ceil(ISO_SECTOR).max(1);
                }
            }
            part_lba.push(lbas);
        }
        let mut img = vec![0u8; next * ISO_SECTOR];

        let dir_lba = |tree: usize, dir: Option<usize>| first_dir + tree * dirs.len() + dirs.iter().position(|d| *d == dir).unwrap();
        for tree in 0..trees {
            let joliet = tree == 1;
            let rock_ridge = self.naming == IsoNaming::RockRidge;
            for &dir in &dirs {
                let lba = dir_lba(tree, dir);
                let parent = dir.and_then(|d| self.nodes[d].parent());
                let mut sector = Vec::new();
                let dot_su = if rock_ridge && dir.is_none() { vec![b'S', b'P', 7, 1, 0xBE, 0xEF, 0] } else { Vec::new() };
                sector.extend(iso_record(&[0], lba, ISO_SECTOR, 2, &dot_su));
                sector.extend(iso_record(&[1], dir_lba(tree, parent), ISO_SECTOR, 2, &[]));
                for (i, node) in self.nodes.iter().enumerate().filter(|(_, n)| n.parent() == dir) {
                    let (name, is_dir) = match node {
                        IsoNode::Dir { name, .. } => (name, true),
                        IsoNode::File { name, .. } | IsoNode::Symlink { name, .. } => (name, false),
                    };
                    let raw: Vec<u8> = if joliet {
                        name.encode_utf16().flat_map(u16::to_be_bytes).collect()
                    } else if is_dir {
                        name.to_uppercase().into_bytes()
                    } else {
                        format!("{};1", name.to_uppercase()).into_bytes()
                    };
                    let mut su = Vec::new();
                    if rock_ridge {
                        su.extend([b'N', b'M', 5 + name.len() as u8, 1, 0]);
                        su.extend(name.as_bytes());
                    }
                    match node {
                        IsoNode::Dir { .. } => sector.extend(iso_record(&raw, dir_lba(tree, Some(i)), ISO_SECTOR, 2, &su)),
                        IsoNode::Symlink { .. } => {
                            let mut px = vec![b'P', b'X', 36, 1];
                            px.extend(0o120777u32.to_le_bytes());
                            px.extend(0o120777u32.to_be_bytes());
                            px.resize(36, 0);
                            su.extend(px);
                            sector.extend(iso_record(&raw, 0, 0, 0, &su));
                        }
                        IsoNode::File { parts, .. } => {
                            for (n, part) in parts.iter().enumerate() {
                                let flags = if n + 1 < parts.len() { 0x80 } else { 0 };
                                sector.extend(iso_record(&raw, part_lba[i][n], part.len(), flags, &su));
                            }
                        }
                    }
                }
                assert!(sector.len() <= ISO_SECTOR, "test directory exceeds one sector");
                img[lba * ISO_SECTOR..lba * ISO_SECTOR + sector.len()].copy_from_slice(&sector);
            }
        }
        for (i, node) in self.nodes.iter().enumerate() {
            if let IsoNode::File { parts, .. } = node {
                for (n, part) in parts.iter().enumerate() {
                    let at = part_lba[i][n] * ISO_SECTOR;
                    img[at..at + part.len()].copy_from_slice(part);
                }
            }
        }

        let descriptor = |kind: u8| {
            let mut d = vec![0u8; ISO_SECTOR];
            d[0] = kind;
            d[1..6].copy_from_slice(b"CD001");
            d[6] = 1;
            d
        };
        let mut pvd = descriptor(1);
        pvd[40..72].fill(b' ');
        pvd[40..40 + self.label.len()].copy_from_slice(self.label.as_bytes());
        pvd[128..130].copy_from_slice(&(ISO_SECTOR as u16).to_le_bytes());
        pvd[156..190].copy_from_slice(&iso_record(&[0], dir_lba(0, None), ISO_SECTOR, 2, &[]));
        img[16 * ISO_SECTOR..17 * ISO_SECTOR].copy_from_slice(&pvd);
        let mut terminator_at = 17;
        if trees == 2 {
            let mut svd = descriptor(2);
            svd[88..91].copy_from_slice(b"%/E");
            svd[128..130].copy_from_slice(&(ISO_SECTOR as u16).to_le_bytes());
            svd[156..190].copy_from_slice(&iso_record(&[0], dir_lba(1, None), ISO_SECTOR, 2, &[]));
            img[17 * ISO_SECTOR..18 * ISO_SECTOR].copy_from_slice(&svd);
            terminator_at = 18;
        }
        img[terminator_at * ISO_SECTOR..(terminator_at + 1) * ISO_SECTOR].copy_from_slice(&descriptor(255));
        img
    }
}

fn iso_record(name: &[u8], lba: usize, size: usize, flags: u8, su: &[u8]) -> Vec<u8> {
    let mut r = vec![0u8; 33];
    r[2..6].copy_from_slice(&(lba as u32).to_le_bytes());
    r[6..10].copy_from_slice(&(lba as u32).to_be_bytes());
    r[10..14].copy_from_slice(&(size as u32).to_le_bytes());
    r[14..18].copy_from_slice(&(size as u32).to_be_bytes());
    r[18..25].copy_from_slice(&ISO_DATE);
    r[25] = flags;
    r[28] = 1;
    r[31] = 1;
    r[32] = name.len() as u8;
    r.extend(name);
    if name.len().is_multiple_of(2) {
        r.push(0);
    }
    r.extend(su);
    if r.len() % 2 == 1 {
        r.push(0);
    }
    r[0] = r.len() as u8;
    r
}
