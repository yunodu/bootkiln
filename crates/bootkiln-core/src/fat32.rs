//! A FAT32 volume in a single MBR partition, laid out in one pass as a [`Plan`].
//!
//! Every directory and file gets one contiguous cluster run, so the whole drive is written
//! with large sequential I/O and no read-modify-write of the FAT.

use std::collections::HashSet;

use crate::error::Error;
use crate::mbr::{self, BOOT_STUB, PARTITION_ALIGN};
use crate::plan::{Plan, Source};

/// Zeroed at the end of the disk so a stale backup GPT cannot shadow the new MBR.
const TAIL_WIPE: u64 = 1024 * 1024;
const MIN_RESERVED_SECTORS: u64 = 32;
const NUM_FATS: u64 = 2;
/// Fewer clusters than this makes the volume FAT16 by definition.
const MIN_CLUSTERS: u64 = 65_525;
const MAX_CLUSTERS: u64 = 0x0FFF_FFF4;
const FIRST_CLUSTER: u32 = 2;
const END_OF_CHAIN: u32 = 0x0FFF_FFFF;
const MEDIA_FIXED: u8 = 0xF8;
const FSINFO_SECTOR: u16 = 1;
const BACKUP_BOOT_SECTOR: u16 = 6;
const DIR_ENTRY: usize = 32;
const LFN_CHARS: usize = 13;
const LFN_CHAR_OFFSETS: [usize; LFN_CHARS] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
const MAX_LONG_NAME: usize = 255;
const ATTR_VOLUME_ID: u8 = 0x08;
const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_ARCHIVE: u8 = 0x20;
const ATTR_LONG_NAME: u8 = 0x0F;
const SHORT_SPECIALS: &[u8] = b"$%'-_@~`!(){}^#&";
const LONG_FORBIDDEN: &[char] = &['"', '*', '/', ':', '<', '>', '?', '\\', '|'];

/// FAT on-disk date and time (two-second resolution, years 1980-2107).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DosTime {
    pub date: u16,
    pub time: u16,
}

impl DosTime {
    /// Out-of-range fields are clamped, so garbage source dates still give a valid stamp.
    pub fn new(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Self {
        let year = year.clamp(1980, 2107) - 1980;
        let date = (year << 9) | (u16::from(month.clamp(1, 12)) << 5) | u16::from(day.clamp(1, 31));
        let time = (u16::from(hour.min(23)) << 11) | (u16::from(minute.min(59)) << 5) | u16::from(second.min(59) / 2);
        Self { date, time }
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    pub name: String,
    pub modified: DosTime,
    pub kind: NodeKind,
}

#[derive(Debug, Clone)]
pub enum NodeKind {
    Dir(Vec<Node>),
    /// File contents, concatenated.
    File(Vec<Source>),
}

#[derive(Debug, Clone, Copy)]
pub struct Volume<'a> {
    /// At most 11 bytes of valid short-name characters.
    pub label: &'a str,
    pub volume_id: u32,
    /// MBR disk signature; Windows needs it unique among attached disks.
    pub disk_signature: u32,
    pub created: DosTime,
}

/// MBR + FAT32 layout of `disk_size` bytes holding `root`.
pub fn layout(root: &[Node], volume: &Volume, disk_size: u64, sector_size: u32) -> Result<Plan, Error> {
    assert!(is_valid_label(volume.label), "invalid FAT label {:?}", volume.label);
    let geometry = Geometry::new(disk_size, sector_size)?;

    let mut tree = Tree { dirs: vec![Dir::default()], files: Vec::new() };
    tree.collect(root, 0, "")?;
    let fat = tree.allocate(&geometry)?;

    let mut runs = Vec::new();
    let sector = geometry.sector;
    runs.push((0, Source::Bytes(mbr_sector(&geometry, volume.disk_signature))));
    runs.push((sector, Source::Zeros(geometry.partition_offset - sector)));
    runs.push((geometry.partition_offset, Source::Bytes(reserved_region(&geometry, volume, &fat))));

    let mut fat_bytes: Vec<u8> = fat.entries.iter().flat_map(|e| e.to_le_bytes()).collect();
    fat_bytes.resize((fat_bytes.len() as u64).next_multiple_of(sector) as usize, 0);
    let fat_len = geometry.fat_sectors * sector;
    for copy in 0..NUM_FATS {
        let offset = geometry.fat_offset(copy);
        runs.push((offset + fat_bytes.len() as u64, Source::Zeros(fat_len - fat_bytes.len() as u64)));
        runs.push((offset, Source::Bytes(fat_bytes.clone())));
    }

    for (index, dir) in tree.dirs.iter().enumerate() {
        let bytes = tree.dir_bytes(index, volume, geometry.cluster);
        runs.push((geometry.cluster_offset(dir.first_cluster), Source::Bytes(bytes)));
    }
    for file in tree.files.iter().filter(|f| f.first_cluster != 0) {
        let mut offset = geometry.cluster_offset(file.first_cluster);
        for source in file.sources {
            runs.push((offset, source.clone()));
            offset += source.len();
        }
    }

    let disk_end = geometry.disk_sectors * sector;
    let wipe_start = disk_end.saturating_sub(TAIL_WIPE).max(geometry.cluster_offset(fat.next_free));
    if wipe_start < disk_end {
        runs.push((wipe_start, Source::Zeros(disk_end - wipe_start)));
    }
    Ok(Plan::new(runs))
}

/// FAT labels: 1-11 bytes of upper-case short-name characters (spaces excluded).
pub fn is_valid_label(label: &str) -> bool {
    (1..=11).contains(&label.len()) && label.bytes().all(is_short_char)
}

fn is_short_char(c: u8) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit() || SHORT_SPECIALS.contains(&c)
}

/// Sizes in bytes unless named `_sectors`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Geometry {
    pub sector: u64,
    pub disk_sectors: u64,
    pub partition_offset: u64,
    pub partition_sectors: u64,
    pub cluster: u64,
    pub reserved_sectors: u64,
    pub fat_sectors: u64,
    pub clusters: u64,
}

impl Geometry {
    pub fn new(disk_size: u64, sector_size: u32) -> Result<Self, Error> {
        let sector = u64::from(sector_size);
        if !sector.is_power_of_two() || !(512..=4096).contains(&sector) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!("unsupported sector size {sector} for FAT32"),
            )
            .into());
        }
        let too_small = || Error::DriveTooSmall { needed: MIN_CLUSTERS * 512 + PARTITION_ALIGN, available: disk_size };
        let disk_sectors = disk_size / sector;
        let partition_lba = PARTITION_ALIGN / sector;
        // MBR partition fields are 32-bit sector counts.
        let partition_sectors = disk_sectors.checked_sub(partition_lba).ok_or_else(too_small)?.min(u64::from(u32::MAX));
        let cluster = cluster_size(partition_sectors * sector).max(sector);
        let per_cluster = cluster / sector;

        let mut reserved_sectors = MIN_RESERVED_SECTORS;
        let max_clusters = partition_sectors.saturating_sub(reserved_sectors) / per_cluster;
        let fat_sectors = ((max_clusters + 2) * 4).div_ceil(sector);
        // Start the data region on a cluster boundary.
        let metadata = reserved_sectors + NUM_FATS * fat_sectors;
        reserved_sectors += (per_cluster - metadata % per_cluster) % per_cluster;
        let data_sectors = partition_sectors.checked_sub(reserved_sectors + NUM_FATS * fat_sectors).ok_or_else(too_small)?;
        let clusters = (data_sectors / per_cluster).min(MAX_CLUSTERS);
        if clusters < MIN_CLUSTERS {
            return Err(too_small());
        }
        Ok(Self {
            sector,
            disk_sectors,
            partition_offset: PARTITION_ALIGN,
            partition_sectors,
            cluster,
            reserved_sectors,
            fat_sectors,
            clusters,
        })
    }

    fn fat_offset(&self, copy: u64) -> u64 {
        self.partition_offset + (self.reserved_sectors + copy * self.fat_sectors) * self.sector
    }

    pub fn data_offset(&self) -> u64 {
        self.fat_offset(NUM_FATS)
    }

    fn cluster_offset(&self, cluster: u32) -> u64 {
        self.data_offset() + u64::from(cluster - FIRST_CLUSTER) * self.cluster
    }
}

/// Microsoft's default FAT32 cluster sizes.
fn cluster_size(volume_bytes: u64) -> u64 {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    match volume_bytes {
        b if b < 260 * MIB => 512,
        b if b <= 8 * GIB => 4096,
        b if b <= 16 * GIB => 8192,
        b if b <= 32 * GIB => 16384,
        _ => 32768,
    }
}

#[derive(Default)]
struct Dir {
    parent: Option<usize>,
    entries: Vec<Entry>,
    first_cluster: u32,
}

struct Entry {
    short: [u8; 11],
    long: Option<Vec<u16>>,
    modified: DosTime,
    target: Target,
}

enum Target {
    Dir(usize),
    File(usize),
}

struct File<'a> {
    sources: &'a [Source],
    len: u32,
    first_cluster: u32,
}

struct Tree<'a> {
    dirs: Vec<Dir>,
    files: Vec<File<'a>>,
}

struct Fat {
    entries: Vec<u32>,
    next_free: u32,
    free: u32,
}

impl<'a> Tree<'a> {
    /// Depth-first; `dir` is the index of the directory holding `nodes`.
    fn collect(&mut self, nodes: &'a [Node], dir: usize, path: &str) -> Result<(), Error> {
        let mut seen = HashSet::new();
        for node in nodes {
            let path = format!("{path}/{}", node.name);
            if !is_valid_long_name(&node.name) {
                return Err(Error::InvalidName { path });
            }
            if !seen.insert(node.name.to_uppercase()) {
                return Err(Error::NameConflict { path });
            }
        }

        // Names that already are 8.3 claim their short form first so generated aliases avoid them.
        let mut used = HashSet::new();
        let exact: Vec<Option<[u8; 11]>> = nodes.iter().map(|n| exact_short_name(&n.name).filter(|s| used.insert(*s))).collect();
        let mut entries = Vec::with_capacity(nodes.len());
        for (node, exact) in nodes.iter().zip(exact) {
            let node_path = format!("{path}/{}", node.name);
            let (short, long) = match exact {
                Some(short) => (short, None),
                None => (generated_short_name(&node.name, &mut used), Some(node.name.encode_utf16().collect())),
            };
            let target = match &node.kind {
                NodeKind::Dir(children) => {
                    let index = self.dirs.len();
                    self.dirs.push(Dir { parent: Some(dir), ..Dir::default() });
                    self.collect(children, index, &node_path)?;
                    Target::Dir(index)
                }
                NodeKind::File(sources) => {
                    let size: u64 = sources.iter().map(Source::len).sum();
                    let len =
                        u32::try_from(size).map_err(|_| Error::FileTooLarge { path: node_path.clone(), size })?;
                    self.files.push(File { sources, len, first_cluster: 0 });
                    Target::File(self.files.len() - 1)
                }
            };
            entries.push(Entry { short, long, modified: node.modified, target });
        }
        self.dirs[dir].entries = entries;
        Ok(())
    }

    fn allocate(&mut self, geometry: &Geometry) -> Result<Fat, Error> {
        let mut entries = vec![0x0FFF_FF00 | u32::from(MEDIA_FIXED), END_OF_CHAIN];
        let mut chain = |count: u64| -> u32 {
            if count == 0 {
                return 0;
            }
            let first = entries.len() as u32;
            entries.extend((1..count).map(|i| first + i as u32));
            entries.push(END_OF_CHAIN);
            first
        };
        for index in 0..self.dirs.len() {
            let slots = self.dir_slots(index) as u64;
            self.dirs[index].first_cluster = chain((slots * DIR_ENTRY as u64).div_ceil(geometry.cluster).max(1));
        }
        for file in &mut self.files {
            file.first_cluster = chain(u64::from(file.len).div_ceil(geometry.cluster));
        }

        let used = entries.len() as u64 - u64::from(FIRST_CLUSTER);
        if used > geometry.clusters {
            return Err(Error::DriveTooSmall { needed: used * geometry.cluster, available: geometry.clusters * geometry.cluster });
        }
        Ok(Fat {
            next_free: entries.len() as u32,
            free: (geometry.clusters - used) as u32,
            entries,
        })
    }

    /// Directory entries including ".", ".." or the volume label, and long-name slots.
    fn dir_slots(&self, index: usize) -> usize {
        let fixed = if index == 0 { 1 } else { 2 };
        fixed + self.dirs[index].entries.iter().map(|e| 1 + e.long.as_ref().map_or(0, |l| l.len().div_ceil(LFN_CHARS))).sum::<usize>()
    }

    /// Directory contents padded with zeros to whole clusters (zeros end the listing).
    fn dir_bytes(&self, index: usize, volume: &Volume, cluster: u64) -> Vec<u8> {
        let dir = &self.dirs[index];
        let mut out = Vec::with_capacity(self.dir_slots(index) * DIR_ENTRY);
        match dir.parent {
            None => {
                let mut label = [b' '; 11];
                label[..volume.label.len()].copy_from_slice(volume.label.as_bytes());
                out.extend(short_entry(&label, ATTR_VOLUME_ID, 0, 0, volume.created));
            }
            Some(parent) => {
                // ".." of a first-level directory points at cluster 0, meaning the root.
                let parent_cluster = if parent == 0 { 0 } else { self.dirs[parent].first_cluster };
                out.extend(short_entry(b".          ", ATTR_DIRECTORY, dir.first_cluster, 0, volume.created));
                out.extend(short_entry(b"..         ", ATTR_DIRECTORY, parent_cluster, 0, volume.created));
            }
        }
        for entry in &dir.entries {
            if let Some(long) = &entry.long {
                for slot in lfn_slots(long, checksum(&entry.short)) {
                    out.extend(slot);
                }
            }
            let (attr, cluster, size) = match entry.target {
                Target::Dir(i) => (ATTR_DIRECTORY, self.dirs[i].first_cluster, 0),
                Target::File(i) => (ATTR_ARCHIVE, self.files[i].first_cluster, self.files[i].len),
            };
            out.extend(short_entry(&entry.short, attr, cluster, size, entry.modified));
        }
        out.resize((out.len() as u64).next_multiple_of(cluster).max(cluster) as usize, 0);
        out
    }
}

fn is_valid_long_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.ends_with([' ', '.'])
        && !name.chars().any(|c| c < ' ' || LONG_FORBIDDEN.contains(&c))
        && name.encode_utf16().count() <= MAX_LONG_NAME
}

/// The name itself if it is a valid upper-case 8.3 name.
fn exact_short_name(name: &str) -> Option<[u8; 11]> {
    let (base, ext) = name.split_once('.').unwrap_or((name, ""));
    let valid = |part: &str, max: usize| part.len() <= max && part.bytes().all(is_short_char);
    if base.is_empty() || !valid(base, 8) || !valid(ext, 3) || (name.contains('.') && ext.is_empty()) {
        return None;
    }
    let mut short = [b' '; 11];
    short[..base.len()].copy_from_slice(base.as_bytes());
    short[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
    Some(short)
}

/// Windows-style alias: the upper-cased name when that is valid 8.3 and free, else an
/// upper-cased basis with a `~N` tail unique within the directory.
fn generated_short_name(name: &str, used: &mut HashSet<[u8; 11]>) -> [u8; 11] {
    if let Some(upper) = exact_short_name(&name.to_ascii_uppercase()).filter(|s| used.insert(*s)) {
        return upper;
    }
    let trimmed = name.trim_start_matches('.');
    let (base, ext) = match trimmed.rfind('.') {
        Some(dot) => (&trimmed[..dot], &trimmed[dot + 1..]),
        None => (trimmed, ""),
    };
    let clean = |part: &str| -> Vec<u8> {
        part.chars()
            .filter(|c| *c != ' ' && *c != '.')
            .map(|c| {
                let c = c.to_ascii_uppercase();
                if c.is_ascii() && is_short_char(c as u8) { c as u8 } else { b'_' }
            })
            .collect()
    };
    let base = clean(base);
    let ext = clean(ext);
    let mut short = [b' '; 11];
    let ext_len = ext.len().min(3);
    short[8..8 + ext_len].copy_from_slice(&ext[..ext_len]);

    let base = if base.is_empty() { b"_".to_vec() } else { base };
    for n in 1u32.. {
        let tail = format!("~{n}");
        let keep = base.len().min(8 - tail.len());
        let mut candidate = short;
        candidate[..keep].copy_from_slice(&base[..keep]);
        candidate[keep..keep + tail.len()].copy_from_slice(tail.as_bytes());
        if used.insert(candidate) {
            return candidate;
        }
    }
    unreachable!("a directory cannot hold enough entries to exhaust ~N tails")
}

fn checksum(short: &[u8; 11]) -> u8 {
    short.iter().fold(0u8, |sum, &b| sum.rotate_right(1).wrapping_add(b))
}

/// Long-name slots in on-disk order (highest sequence number first).
fn lfn_slots(long: &[u16], checksum: u8) -> Vec<[u8; DIR_ENTRY]> {
    let count = long.len().div_ceil(LFN_CHARS);
    (1..=count)
        .rev()
        .map(|seq| {
            let mut slot = [0u8; DIR_ENTRY];
            slot[0] = seq as u8 | if seq == count { 0x40 } else { 0 };
            slot[11] = ATTR_LONG_NAME;
            slot[13] = checksum;
            for (i, &at) in LFN_CHAR_OFFSETS.iter().enumerate() {
                let index = (seq - 1) * LFN_CHARS + i;
                let unit = match index.cmp(&long.len()) {
                    std::cmp::Ordering::Less => long[index],
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 0xFFFF,
                };
                slot[at..at + 2].copy_from_slice(&unit.to_le_bytes());
            }
            slot
        })
        .collect()
}

fn short_entry(name: &[u8; 11], attr: u8, cluster: u32, size: u32, time: DosTime) -> [u8; DIR_ENTRY] {
    let mut e = [0u8; DIR_ENTRY];
    e[..11].copy_from_slice(name);
    e[11] = attr;
    e[14..16].copy_from_slice(&time.time.to_le_bytes());
    e[16..18].copy_from_slice(&time.date.to_le_bytes());
    e[18..20].copy_from_slice(&time.date.to_le_bytes());
    e[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
    e[22..24].copy_from_slice(&time.time.to_le_bytes());
    e[24..26].copy_from_slice(&time.date.to_le_bytes());
    e[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
    e[28..32].copy_from_slice(&size.to_le_bytes());
    e
}

fn mbr_sector(g: &Geometry, disk_signature: u32) -> Vec<u8> {
    let partition = mbr::Partition {
        kind: mbr::TYPE_FAT32_LBA,
        active: true,
        start_lba: (g.partition_offset / g.sector) as u32,
        sectors: g.partition_sectors as u32,
    };
    mbr::sector(g.sector, disk_signature, &[partition])
}

/// Boot sector, FSInfo and their backups; the rest of the reserved area is zero.
fn reserved_region(g: &Geometry, volume: &Volume, fat: &Fat) -> Vec<u8> {
    let sector = g.sector as usize;
    let mut region = vec![0u8; g.reserved_sectors as usize * sector];
    let boot = boot_sector(g, volume);
    let fsinfo = fsinfo_sector(g, fat);
    let backup = usize::from(BACKUP_BOOT_SECTOR);
    let fsinfo_at = usize::from(FSINFO_SECTOR);
    for (at, data) in [(0, &boot), (fsinfo_at, &fsinfo), (backup, &boot), (backup + fsinfo_at, &fsinfo)] {
        region[at * sector..(at + 1) * sector].copy_from_slice(data);
    }
    region
}

fn boot_sector(g: &Geometry, volume: &Volume) -> Vec<u8> {
    let mut s = vec![0u8; g.sector as usize];
    s[..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    s[3..11].copy_from_slice(b"MSWIN4.1");
    s[11..13].copy_from_slice(&(g.sector as u16).to_le_bytes());
    s[13] = (g.cluster / g.sector) as u8;
    s[14..16].copy_from_slice(&(g.reserved_sectors as u16).to_le_bytes());
    s[16] = NUM_FATS as u8;
    s[21] = MEDIA_FIXED;
    s[24..26].copy_from_slice(&63u16.to_le_bytes());
    s[26..28].copy_from_slice(&255u16.to_le_bytes());
    s[28..32].copy_from_slice(&((g.partition_offset / g.sector) as u32).to_le_bytes());
    s[32..36].copy_from_slice(&(g.partition_sectors as u32).to_le_bytes());
    s[36..40].copy_from_slice(&(g.fat_sectors as u32).to_le_bytes());
    s[44..48].copy_from_slice(&FIRST_CLUSTER.to_le_bytes());
    s[48..50].copy_from_slice(&FSINFO_SECTOR.to_le_bytes());
    s[50..52].copy_from_slice(&BACKUP_BOOT_SECTOR.to_le_bytes());
    s[64] = 0x80;
    s[66] = 0x29;
    s[67..71].copy_from_slice(&volume.volume_id.to_le_bytes());
    s[71..82].fill(b' ');
    s[71..71 + volume.label.len()].copy_from_slice(volume.label.as_bytes());
    s[82..90].copy_from_slice(b"FAT32   ");
    s[90..90 + BOOT_STUB.len()].copy_from_slice(&BOOT_STUB);
    s[510] = 0x55;
    s[511] = 0xAA;
    s
}

fn fsinfo_sector(g: &Geometry, fat: &Fat) -> Vec<u8> {
    let mut s = vec![0u8; g.sector as usize];
    s[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
    s[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
    s[488..492].copy_from_slice(&fat.free.to_le_bytes());
    // 0xFFFFFFFF: no hint (the volume is full).
    let next_free = if fat.free == 0 { u32::MAX } else { fat.next_free };
    s[492..496].copy_from_slice(&next_free.to_le_bytes());
    s[508..512].copy_from_slice(&0xAA55_0000u32.to_le_bytes());
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::write_plan;
    use crate::testutil::{MemDisk, pattern};
    use fatfs::{FileSystem, FsOptions};
    use std::io::{Cursor, Read};

    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    const STAMP: DosTime = DosTime { date: 0x5A21, time: 0x6000 };

    fn file(name: &str, data: Vec<u8>) -> Node {
        Node { name: name.into(), modified: STAMP, kind: NodeKind::File(vec![Source::Bytes(data)]) }
    }

    fn dir(name: &str, children: Vec<Node>) -> Node {
        Node { name: name.into(), modified: STAMP, kind: NodeKind::Dir(children) }
    }

    fn volume() -> Volume<'static> {
        Volume { label: "TEST-VOL", volume_id: 0xDEAD_BEEF, disk_signature: 0x1234_5678, created: STAMP }
    }

    /// Write `root` to an in-memory disk and mount its partition with an independent FAT driver.
    fn mount(root: &[Node], disk_size: usize, sector: u32) -> (MemDisk, FileSystem<Cursor<Vec<u8>>>) {
        let plan = layout(root, &volume(), disk_size as u64, sector).unwrap();
        let mut disk = MemDisk::new(disk_size, sector);
        write_plan(&mut Cursor::new(Vec::new()), &plan, &mut disk, &mut |_| Ok(())).unwrap();
        let partition = disk.data[MIB as usize..].to_vec();
        let fs = FileSystem::new(Cursor::new(partition), FsOptions::new()).unwrap();
        (disk, fs)
    }

    fn read(fs: &FileSystem<Cursor<Vec<u8>>>, path: &str) -> Vec<u8> {
        let mut data = Vec::new();
        fs.root_dir().open_file(path).unwrap().read_to_end(&mut data).unwrap();
        data
    }

    #[test]
    fn geometry_is_valid_fat32_across_sizes_and_sector_sizes() {
        for (size, sector) in [(40 * MIB, 512), (300 * MIB, 4096), (58 * GIB, 512), (64 * GIB, 4096), (3 * 1024 * GIB, 512)] {
            let g = Geometry::new(size, sector).unwrap();
            let ctx = format!("{size} bytes / {sector}-byte sectors");
            assert!(g.clusters >= MIN_CLUSTERS, "{ctx}: FAT32 needs >= 65525 clusters");
            assert!(g.fat_sectors * g.sector / 4 >= g.clusters + 2, "{ctx}: FAT too small");
            assert_eq!(g.data_offset() % g.cluster, 0, "{ctx}: data region not cluster-aligned");
            let partition_end = g.partition_offset + g.partition_sectors * g.sector;
            assert!(partition_end <= size, "{ctx}: partition past disk end");
            assert!(g.data_offset() + g.clusters * g.cluster <= partition_end, "{ctx}: clusters past partition");
            assert!(g.partition_sectors <= u64::from(u32::MAX), "{ctx}: exceeds MBR limit");
        }
        assert_eq!(Geometry::new(58 * GIB, 512).unwrap().cluster, 32 * 1024);
    }

    #[test]
    fn disk_below_fat32_minimum_is_refused() {
        let err = Geometry::new(20 * MIB, 512).unwrap_err();
        assert!(matches!(err, Error::DriveTooSmall { .. }), "{err:?}");
    }

    #[test]
    fn independent_fat_driver_reads_back_names_and_contents() {
        let similar: Vec<Node> = (1..=12).map(|i| file(&format!("long file name {i}.txt"), vec![i as u8; i])).collect();
        let root = vec![
            dir("EFI", vec![dir("BOOT", vec![file("BOOTX64.EFI", b"shim".to_vec()), file("grub.cfg", b"cfg".to_vec())])]),
            dir("Similar", similar),
            file("Ünïcödé name with a very long tail that needs several LFN slots.bin", pattern(70_000)),
            file("empty", Vec::new()),
            dir("a", vec![dir("b", vec![dir("c", vec![file("deep", b"deep".to_vec())])])]),
        ];
        let (disk, fs) = mount(&root, 40 * MIB as usize, 512);

        assert_eq!(fs.fat_type(), fatfs::FatType::Fat32);
        assert_eq!(fs.volume_label(), "TEST-VOL");
        assert_eq!(read(&fs, "EFI/BOOT/BOOTX64.EFI"), b"shim");
        assert_eq!(read(&fs, "EFI/BOOT/grub.cfg"), b"cfg");
        assert_eq!(read(&fs, "Ünïcödé name with a very long tail that needs several LFN slots.bin"), pattern(70_000));
        assert_eq!(read(&fs, "empty"), b"");
        assert_eq!(read(&fs, "a/b/c/deep"), b"deep");

        let similar = fs.root_dir().open_dir("Similar").unwrap();
        let entries: Vec<_> = similar.iter().map(|e| e.unwrap()).filter(|e| !e.is_dir()).collect();
        let long: Vec<String> = entries.iter().map(|e| e.file_name()).collect();
        let expected: Vec<String> = (1..=12).map(|i| format!("long file name {i}.txt")).collect();
        assert_eq!(long, expected);
        let mut short: Vec<String> = entries.iter().map(|e| e.short_file_name()).collect();
        short.sort();
        short.dedup();
        assert_eq!(short.len(), 12, "short aliases must be unique");
        for e in &entries {
            assert_eq!(e.len() as usize, e.file_name().split(' ').nth(3).unwrap().trim_end_matches(".txt").parse::<usize>().unwrap());
        }

        // ".." of a nested directory leads back to its parent.
        let c = fs.root_dir().open_dir("a/b/c").unwrap();
        let up: Vec<String> = c.open_dir("..").unwrap().iter().map(|e| e.unwrap().file_name()).collect();
        assert!(up.contains(&"c".to_owned()), "{up:?}");

        let entry = &disk.data[446..462];
        assert_eq!(entry[0], 0x80, "active");
        assert_eq!(entry[4], mbr::TYPE_FAT32_LBA);
        assert_eq!(u32::from_le_bytes(entry[8..12].try_into().unwrap()), 2048);
        assert_eq!(&disk.data[510..512], [0x55, 0xAA]);
        assert_eq!(u32::from_le_bytes(disk.data[440..444].try_into().unwrap()), 0x1234_5678);
    }

    #[test]
    fn four_kib_sector_disk_round_trips() {
        let root = vec![file("data.bin", pattern(100_000))];
        let (_, fs) = mount(&root, 300 * MIB as usize, 4096);
        assert_eq!(read(&fs, "data.bin"), pattern(100_000));
    }

    #[test]
    fn stale_partition_tables_are_wiped() {
        let size = 40 * MIB as usize;
        let plan = layout(&[], &volume(), size as u64, 512).unwrap();
        let mut disk = MemDisk::new(size, 512);
        write_plan(&mut Cursor::new(Vec::new()), &plan, &mut disk, &mut |_| Ok(())).unwrap();
        assert!(disk.data[512..1024].iter().all(|&b| b == 0), "primary GPT header");
        assert!(disk.data[size - 512..].iter().all(|&b| b == 0), "backup GPT header");
    }

    #[test]
    fn files_of_4_gib_are_refused() {
        let big = Node { name: "big".into(), modified: STAMP, kind: NodeKind::File(vec![Source::Zeros(4 * GIB)]) };
        let err = layout(&[dir("d", vec![big])], &volume(), 64 * GIB, 512).unwrap_err();
        assert!(matches!(&err, Error::FileTooLarge { path, size } if path == "/d/big" && *size == 4 * GIB), "{err:?}");
    }

    #[test]
    fn content_larger_than_volume_is_refused() {
        let big = Node { name: "big".into(), modified: STAMP, kind: NodeKind::File(vec![Source::Zeros(50 * MIB)]) };
        let err = layout(&[big], &volume(), 40 * MIB, 512).unwrap_err();
        assert!(matches!(err, Error::DriveTooSmall { .. }), "{err:?}");
    }

    #[test]
    fn names_differing_only_in_case_are_refused() {
        let err = layout(&[file("A.txt", vec![]), file("a.TXT", vec![])], &volume(), 40 * MIB, 512).unwrap_err();
        assert!(matches!(&err, Error::NameConflict { path } if path == "/a.TXT"), "{err:?}");
    }
}
