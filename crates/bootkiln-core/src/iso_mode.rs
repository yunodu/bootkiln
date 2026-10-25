//! ISO mode: copy a bootable ISO's files onto a fresh drive that UEFI firmware boots.
//!
//! The drive gets FAT32 unless a file is 4 GiB or larger; then it gets NTFS plus a
//! UEFI:NTFS boot partition ([`crate::uefi_ntfs`]).
//!
//! Boot loaders find the live system by filesystem label, and a FAT label (11 upper-case
//! characters) or NTFS label (32 characters) may not equal the ISO label, so boot
//! configuration files that name the ISO label are rewritten to name the drive's label.

use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};

use crate::disc::{self, DiscEntry, DiscKind};
use crate::error::Error;
use crate::fat32::{self, DosTime, Node, NodeKind, Volume};
use crate::plan::{Plan, Source};
use crate::windows_iso::{self, WindowsInfo};

/// Boot configuration files larger than this are not scanned for the label.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const CONFIG_EXTENSIONS: [&str; 2] = [".cfg", ".conf"];
const FALLBACK_LABEL: &str = "BOOTKILN";
const COMPARE_CHUNK: usize = 1024 * 1024;
/// FAT32 stores file sizes in 32 bits.
const FAT32_MAX_FILE: u64 = u32::MAX as u64;
const NTFS_LABEL_CHARS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSystem {
    Fat32,
    /// NTFS data partition plus a UEFI:NTFS boot partition.
    Ntfs,
}

/// The files of a bootable ISO, adapted for the drive's file system.
#[derive(Debug)]
pub struct IsoContent {
    pub iso_label: String,
    pub file_system: FileSystem,
    /// Volume label for `file_system`.
    pub label: String,
    pub windows: Option<WindowsInfo>,
    /// Files of 4 GiB or more; they are why `file_system` is NTFS.
    pub large_files: Vec<String>,
    /// Boot configuration files rewritten to `label`.
    pub patched: Vec<String>,
    /// Symbolic links, which neither FAT nor (portably) NTFS can hold.
    pub skipped_symlinks: Vec<String>,
    /// Files dropped because an identical file differing only in name case was kept.
    pub merged_duplicates: Vec<String>,
    nodes: Vec<Node>,
    created: DosTime,
}

/// Read the ISO in `image` and adapt its files for the drive.
pub fn prepare<R: Read + Seek>(image: &mut R) -> Result<IsoContent, Error> {
    let tree = disc::read_tree(image)?;
    if !has_uefi_loader(&tree.root) {
        return Err(Error::NoUefiBootloader);
    }
    let windows = windows_iso::detect(image, &tree.root)?;
    let mut large_files = Vec::new();
    collect_large(&tree.root, "", &mut large_files);
    let (file_system, label) = if large_files.is_empty() {
        (FileSystem::Fat32, fat_label(&tree.label))
    } else {
        (FileSystem::Ntfs, ntfs_label(&tree.label))
    };

    let mut converter = Converter {
        image,
        iso_label: &tree.label,
        label: &label,
        patched: Vec::new(),
        skipped_symlinks: Vec::new(),
        merged_duplicates: Vec::new(),
    };
    let nodes = converter.convert(&tree.root, "")?;
    let Converter { patched, skipped_symlinks, merged_duplicates, .. } = converter;
    Ok(IsoContent {
        iso_label: tree.label,
        file_system,
        label,
        windows,
        large_files,
        patched,
        skipped_symlinks,
        merged_duplicates,
        nodes,
        created: dos_time(tree.created),
    })
}

impl IsoContent {
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// Put `data` at the drive root as `name`, replacing any root entry of that name.
    pub fn add_root_file(&mut self, name: &str, data: Vec<u8>) {
        self.nodes.retain(|n| !n.name.eq_ignore_ascii_case(name));
        self.nodes.push(Node { name: name.to_owned(), modified: self.created, kind: NodeKind::File(vec![Source::Bytes(data)]) });
    }

    /// MBR + FAT32 drive holding the files; only valid for [`FileSystem::Fat32`].
    pub fn fat32_plan(&self, disk_size: u64, sector_size: u32, volume_id: u32, disk_signature: u32) -> Result<Plan, Error> {
        assert_eq!(self.file_system, FileSystem::Fat32, "files of 4 GiB or more need NTFS");
        let volume = Volume { label: &self.label, volume_id, disk_signature, created: self.created };
        fat32::layout(&self.nodes, &volume, disk_size, sector_size)
    }
}

fn collect_large(entries: &[DiscEntry], dir_path: &str, out: &mut Vec<String>) {
    for entry in entries {
        let path = format!("{dir_path}/{}", entry.name);
        match &entry.kind {
            DiscKind::Dir(children) => collect_large(children, &path, out),
            DiscKind::File(_) if entry.file_len() > FAT32_MAX_FILE => out.push(path),
            _ => {}
        }
    }
}

/// UEFI removable-media boot path: `\EFI\BOOT\BOOT<arch>.EFI`, matched case-insensitively.
fn has_uefi_loader(root: &[DiscEntry]) -> bool {
    match disc::find(root, &["EFI", "BOOT"]).map(|e| &e.kind) {
        Some(DiscKind::Dir(boot)) => boot.iter().any(|e| {
            let name = e.name.to_ascii_uppercase();
            matches!(e.kind, DiscKind::File(_)) && name.starts_with("BOOT") && name.ends_with(".EFI")
        }),
        _ => false,
    }
}

/// Upper-case, `[A-Z0-9_-]` only, at most 11 characters.
fn fat_label(iso_label: &str) -> String {
    let label: String = iso_label
        .chars()
        .take(11)
        .map(|c| match c.to_ascii_uppercase() {
            c @ ('A'..='Z' | '0'..='9' | '-' | '_') => c,
            _ => '_',
        })
        .collect();
    if label.is_empty() { FALLBACK_LABEL.to_owned() } else { label }
}

/// The ISO label without control characters, at most 32 characters.
fn ntfs_label(iso_label: &str) -> String {
    let label: String = iso_label.chars().filter(|c| !c.is_control()).take(NTFS_LABEL_CHARS).collect();
    if label.is_empty() { FALLBACK_LABEL.to_owned() } else { label }
}

fn dos_time(recorded: [u8; 7]) -> DosTime {
    let [year, month, day, hour, minute, second, _] = recorded;
    DosTime::new(1900 + u16::from(year), month, day, hour, minute, second)
}

/// Windows and FAT long names cannot contain `"*/:<>?\|` or control characters, nor end
/// in a space or dot.
fn fat_name(name: &str) -> String {
    let replaced: String = name.chars().map(|c| if c < ' ' || "\"*/:<>?\\|".contains(c) { '_' } else { c }).collect();
    let trimmed = replaced.trim_end_matches([' ', '.']);
    if trimmed.is_empty() { "_".to_owned() } else { trimmed.to_owned() }
}

struct Converter<'a, R> {
    image: &'a mut R,
    iso_label: &'a str,
    label: &'a str,
    patched: Vec<String>,
    skipped_symlinks: Vec<String>,
    merged_duplicates: Vec<String>,
}

impl<R: Read + Seek> Converter<'_, R> {
    fn convert(&mut self, entries: &[DiscEntry], dir_path: &str) -> Result<Vec<Node>, Error> {
        let mut nodes: Vec<Node> = Vec::with_capacity(entries.len());
        // Upper-cased name -> index in `nodes`.
        let mut by_name: HashMap<String, usize> = HashMap::new();
        for entry in entries {
            let name = fat_name(&entry.name);
            let path = format!("{dir_path}/{name}");
            let kind = match &entry.kind {
                DiscKind::Symlink => {
                    self.skipped_symlinks.push(path);
                    continue;
                }
                DiscKind::Dir(children) => NodeKind::Dir(self.convert(children, &path)?),
                DiscKind::File(extents) => {
                    let sources = extents.iter().map(|e| Source::Image { offset: e.offset, len: e.len }).collect();
                    NodeKind::File(self.patch_config(&name, &path, sources)?)
                }
            };
            let node = Node { name, modified: dos_time(entry.recorded), kind };

            match by_name.get(&node.name.to_uppercase()) {
                None => {
                    by_name.insert(node.name.to_uppercase(), nodes.len());
                    nodes.push(node);
                }
                Some(&kept) => match (&nodes[kept].kind, &node.kind) {
                    (NodeKind::File(a), NodeKind::File(b)) if self.same_content(a, b)? => {
                        self.merged_duplicates.push(path);
                    }
                    _ => return Err(Error::NameConflict { path }),
                },
            }
        }
        Ok(nodes)
    }

    /// Rewrite the ISO label to the drive label in boot configuration files.
    fn patch_config(&mut self, name: &str, path: &str, sources: Vec<Source>) -> Result<Vec<Source>, Error> {
        let lower = name.to_ascii_lowercase();
        let len: u64 = sources.iter().map(Source::len).sum();
        if self.iso_label.is_empty()
            || self.iso_label == self.label
            || len > MAX_CONFIG_BYTES
            || !CONFIG_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
        {
            return Ok(sources);
        }
        let mut text = vec![0u8; len as usize];
        fill(self.image, &sources, 0, &mut text)?;
        match patch_label(&text, self.iso_label, self.label) {
            Some(patched) => {
                self.patched.push(path.to_owned());
                Ok(vec![Source::Bytes(patched)])
            }
            None => Ok(sources),
        }
    }

    fn same_content(&mut self, a: &[Source], b: &[Source]) -> io::Result<bool> {
        let len: u64 = a.iter().map(Source::len).sum();
        if len != b.iter().map(Source::len).sum::<u64>() {
            return Ok(false);
        }
        let mut chunk_a = vec![0u8; COMPARE_CHUNK];
        let mut chunk_b = vec![0u8; COMPARE_CHUNK];
        let mut offset = 0;
        while offset < len {
            let n = (len - offset).min(COMPARE_CHUNK as u64) as usize;
            fill(self.image, a, offset, &mut chunk_a[..n])?;
            fill(self.image, b, offset, &mut chunk_b[..n])?;
            if chunk_a[..n] != chunk_b[..n] {
                return Ok(false);
            }
            offset += n as u64;
        }
        Ok(true)
    }
}

/// Fill `buf` with the bytes at `offset` of the concatenated `sources`.
pub(crate) fn fill<R: Read + Seek>(image: &mut R, sources: &[Source], mut offset: u64, mut buf: &mut [u8]) -> io::Result<()> {
    for source in sources {
        if buf.is_empty() {
            break;
        }
        if offset >= source.len() {
            offset -= source.len();
            continue;
        }
        let n = (source.len() - offset).min(buf.len() as u64) as usize;
        let (head, rest) = buf.split_at_mut(n);
        match source {
            Source::Image { offset: start, .. } => {
                image.seek(SeekFrom::Start(start + offset))?;
                image.read_exact(head)?;
            }
            Source::Bytes(bytes) => head.copy_from_slice(&bytes[offset as usize..offset as usize + n]),
            Source::Zeros(_) => head.fill(0),
        }
        buf = rest;
        offset = 0;
    }
    if buf.is_empty() { Ok(()) } else { Err(io::ErrorKind::UnexpectedEof.into()) }
}

/// `CDLABEL=<iso>` becomes `LABEL=<fat>` (dracut), then every other whole-token `<iso>`
/// (including its `\x20`-escaped form) becomes `<fat>`. `None` if nothing changed.
fn patch_label(text: &[u8], iso_label: &str, fat_label: &str) -> Option<Vec<u8>> {
    let mut out = replace_token(text, format!("CDLABEL={iso_label}").as_bytes(), format!("LABEL={fat_label}").as_bytes());
    out = replace_token(&out, iso_label.as_bytes(), fat_label.as_bytes());
    if iso_label.contains(' ') {
        out = replace_token(&out, iso_label.replace(' ', "\\x20").as_bytes(), fat_label.as_bytes());
    }
    (out != text).then_some(out)
}

/// Replace occurrences of `from` not embedded in a longer label-like word.
fn replace_token(text: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let is_word = |c: u8| c.is_ascii_alphanumeric() || b"-_.".contains(&c);
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        let end = i + from.len();
        if text[i..].starts_with(from)
            && (i == 0 || !is_word(text[i - 1]))
            && text.get(end).is_none_or(|&c| !is_word(c))
        {
            out.extend_from_slice(to);
            i = end;
        } else {
            out.push(text[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::write_plan;
    use crate::testutil::{IsoBuilder, IsoNaming, MemDisk, pattern};
    use fatfs::FsOptions;
    use std::io::Cursor;

    const DISK: usize = 40 * 1024 * 1024;
    const GRUB_CFG: &str = "search --file --set=root /boot/id\n\
        linux /boot/linux root=live:CDLABEL=Fedora-KDE-Live-44 rd.live.image\n\
        search -l 'Fedora-KDE-Live-44'\n\
        menuentry \"Fedora-KDE-Live-44x\" {}\n";

    fn fedora_like() -> IsoBuilder {
        let mut iso = IsoBuilder::new("Fedora-KDE-Live-44", IsoNaming::RockRidge);
        let efi = iso.dir(None, "EFI");
        let boot = iso.dir(Some(efi), "BOOT");
        iso.file(Some(boot), "BOOTX64.EFI", b"shim".to_vec());
        iso.file(Some(boot), "bootx64.efi", b"shim".to_vec());
        iso.file(Some(boot), "grub.cfg", GRUB_CFG.as_bytes().to_vec());
        let live = iso.dir(None, "LiveOS");
        iso.file(Some(live), "squashfs.img", pattern(300_000));
        iso.symlink(None, "fedora");
        iso
    }

    fn write_to_disk(iso: &[u8]) -> (IsoContent, MemDisk) {
        let content = prepare(&mut Cursor::new(iso)).unwrap();
        let plan = content.fat32_plan(DISK as u64, 512, 0x1234_5678, 0xCAFE_F00D).unwrap();
        let mut disk = MemDisk::new(DISK, 512);
        write_plan(&mut Cursor::new(iso), &plan, &mut disk, &mut |_| Ok(())).unwrap();
        (content, disk)
    }

    fn read_file(fs: &fatfs::FileSystem<Cursor<Vec<u8>>>, path: &str) -> Vec<u8> {
        let mut data = Vec::new();
        fs.root_dir().open_file(path).unwrap().read_to_end(&mut data).unwrap();
        data
    }

    #[test]
    fn copies_files_to_a_fat32_volume_with_patched_label() {
        let iso = fedora_like().build();
        let (content, disk) = write_to_disk(&iso);
        assert_eq!(content.file_system, FileSystem::Fat32);
        assert_eq!(content.label, "FEDORA-KDE-");
        assert_eq!(content.patched, ["/EFI/BOOT/grub.cfg"]);
        assert_eq!(content.skipped_symlinks, ["/fedora"]);
        assert_eq!(content.merged_duplicates, ["/EFI/BOOT/bootx64.efi"]);
        assert!(content.windows.is_none());

        let partition = disk.data[1024 * 1024..].to_vec();
        let fs = fatfs::FileSystem::new(Cursor::new(partition), FsOptions::new()).unwrap();
        assert_eq!(fs.fat_type(), fatfs::FatType::Fat32);
        assert_eq!(fs.volume_label(), "FEDORA-KDE-");
        assert_eq!(read_file(&fs, "EFI/BOOT/BOOTX64.EFI"), b"shim");
        assert_eq!(read_file(&fs, "LiveOS/squashfs.img"), pattern(300_000));
        let names: Vec<String> = fs.root_dir().iter().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["EFI", "LiveOS"]);

        let cfg = String::from_utf8(read_file(&fs, "EFI/BOOT/grub.cfg")).unwrap();
        assert_eq!(
            cfg,
            "search --file --set=root /boot/id\n\
             linux /boot/linux root=live:LABEL=FEDORA-KDE- rd.live.image\n\
             search -l 'FEDORA-KDE-'\n\
             menuentry \"Fedora-KDE-Live-44x\" {}\n"
        );
    }

    #[test]
    fn case_duplicates_with_different_content_are_refused() {
        let mut iso = IsoBuilder::new("X", IsoNaming::RockRidge);
        let efi = iso.dir(None, "EFI");
        let boot = iso.dir(Some(efi), "BOOT");
        iso.file(Some(boot), "BOOTX64.EFI", b"one".to_vec());
        iso.file(Some(boot), "bootx64.efi", b"two".to_vec());
        let err = prepare(&mut Cursor::new(iso.build())).unwrap_err();
        assert!(matches!(&err, Error::NameConflict { path } if path == "/EFI/BOOT/bootx64.efi"), "{err:?}");
    }

    #[test]
    fn iso_without_uefi_loader_is_refused() {
        let mut iso = IsoBuilder::new("TinyCore", IsoNaming::RockRidge);
        let boot = iso.dir(None, "boot");
        iso.file(Some(boot), "vmlinuz", b"kernel".to_vec());
        let err = prepare(&mut Cursor::new(iso.build())).unwrap_err();
        assert!(matches!(err, Error::NoUefiBootloader), "{err:?}");
    }

    #[test]
    fn added_root_file_replaces_any_case_variant() {
        let mut content = prepare(&mut Cursor::new(fedora_like().build())).unwrap();
        content.add_root_file("AutoUnattend.xml", b"old".to_vec());
        content.add_root_file("autounattend.xml", b"new".to_vec());
        let root: Vec<&str> = content.nodes().iter().map(|n| n.name.as_str()).collect();
        assert_eq!(root, ["EFI", "LiveOS", "autounattend.xml"]);
        assert!(matches!(&content.nodes()[2].kind, NodeKind::File(s) if s == &[Source::Bytes(b"new".to_vec())]));
    }

    #[test]
    fn files_of_4_gib_or_more_are_listed_for_ntfs() {
        let file = |name: &str, len: u64| DiscEntry {
            name: name.into(),
            recorded: [0; 7],
            kind: DiscKind::File(vec![disc::Extent { offset: 0, len }]),
        };
        let tree = vec![
            DiscEntry {
                name: "sources".into(),
                recorded: [0; 7],
                kind: DiscKind::Dir(vec![file("install.wim", 1 << 32), file("boot.wim", u64::from(u32::MAX))]),
            },
            file("small", 1),
        ];
        let mut large = Vec::new();
        collect_large(&tree, "", &mut large);
        assert_eq!(large, ["/sources/install.wim"]);
    }

    #[test]
    fn labels_fit_their_file_system() {
        assert_eq!(fat_label("Fedora-KDE-Live-44"), "FEDORA-KDE-");
        assert_eq!(fat_label("Ubuntu 24.04 LTS"), "UBUNTU_24_0");
        assert_eq!(fat_label("ARCH_202410"), "ARCH_202410");
        assert_eq!(fat_label(""), "BOOTKILN");
        assert_eq!(ntfs_label("CCCOMA_X64FRE_EN-US_DV9"), "CCCOMA_X64FRE_EN-US_DV9");
        assert_eq!(ntfs_label("Ubuntu 26.04.1 LTS amd64\u{1}"), "Ubuntu 26.04.1 LTS amd64");
        assert_eq!(ntfs_label(&"x".repeat(40)), "x".repeat(32));
    }

    #[test]
    fn label_patch_only_touches_whole_tokens() {
        assert_eq!(patch_label(b"archisolabel=ARCH_202410", "ARCH_202410", "ARCH_202410"), None);
        assert_eq!(patch_label(b"x=MYISO2 y=MYISO", "MYISO", "NEW").unwrap(), b"x=MYISO2 y=NEW");
        assert_eq!(patch_label(b"LABEL=My\\x20Disk", "My Disk", "MY_DISK").unwrap(), b"LABEL=MY_DISK");
    }
}
