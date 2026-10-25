//! ISO mode: copy a bootable ISO's files onto a fresh FAT32 drive that UEFI firmware boots.
//!
//! Boot loaders find the live system by filesystem label, and a FAT label (11 upper-case
//! characters) usually cannot equal the ISO label, so boot configuration files that name
//! the ISO label are rewritten to name the FAT label.

use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};

use crate::error::Error;
use crate::fat32::{self, DosTime, Node, NodeKind, Volume};
use crate::iso9660::{self, IsoEntry, IsoKind};
use crate::plan::{Plan, Source};

/// Boot configuration files larger than this are not scanned for the label.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const CONFIG_EXTENSIONS: [&str; 2] = [".cfg", ".conf"];
const FALLBACK_LABEL: &str = "BOOTKILN";
const COMPARE_CHUNK: usize = 1024 * 1024;

#[derive(Debug)]
pub struct IsoModeLayout {
    pub plan: Plan,
    pub iso_label: String,
    pub fat_label: String,
    /// Boot configuration files rewritten to the FAT label.
    pub patched: Vec<String>,
    /// Symbolic links, which FAT cannot represent.
    pub skipped_symlinks: Vec<String>,
    /// Files dropped because an identical file differing only in name case was kept.
    pub merged_duplicates: Vec<String>,
}

/// Plan an MBR + FAT32 drive holding the files of the ISO in `image`.
pub fn layout<R: Read + Seek>(
    image: &mut R,
    disk_size: u64,
    sector_size: u32,
    volume_id: u32,
    disk_signature: u32,
) -> Result<IsoModeLayout, Error> {
    let tree = iso9660::read_tree(image)?;
    if !has_uefi_loader(&tree.root) {
        return Err(Error::NoUefiBootloader);
    }
    let fat_label = fat_label(&tree.label);
    let mut converter = Converter {
        image,
        iso_label: &tree.label,
        fat_label: &fat_label,
        patched: Vec::new(),
        skipped_symlinks: Vec::new(),
        merged_duplicates: Vec::new(),
    };
    let root = converter.convert(&tree.root, "")?;
    let Converter { patched, skipped_symlinks, merged_duplicates, .. } = converter;

    let volume = Volume { label: &fat_label, volume_id, disk_signature, created: dos_time(tree.created) };
    let plan = fat32::layout(&root, &volume, disk_size, sector_size)?;
    Ok(IsoModeLayout { plan, iso_label: tree.label, fat_label, patched, skipped_symlinks, merged_duplicates })
}

/// UEFI removable-media boot path: `\EFI\BOOT\BOOT<arch>.EFI`, matched case-insensitively.
fn has_uefi_loader(root: &[IsoEntry]) -> bool {
    fn subdir<'a>(entries: &'a [IsoEntry], name: &str) -> Option<&'a [IsoEntry]> {
        entries.iter().find_map(|e| match &e.kind {
            IsoKind::Dir(children) if e.name.eq_ignore_ascii_case(name) => Some(children.as_slice()),
            _ => None,
        })
    }
    subdir(root, "EFI").and_then(|efi| subdir(efi, "BOOT")).is_some_and(|boot| {
        boot.iter().any(|e| {
            let name = e.name.to_ascii_uppercase();
            matches!(e.kind, IsoKind::File(_)) && name.starts_with("BOOT") && name.ends_with(".EFI")
        })
    })
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

fn dos_time(recorded: [u8; 7]) -> DosTime {
    let [year, month, day, hour, minute, second, _] = recorded;
    DosTime::new(1900 + u16::from(year), month, day, hour, minute, second)
}

/// FAT long names cannot contain `"*/:<>?\|` or control characters, nor end in a space or dot.
fn fat_name(name: &str) -> String {
    let replaced: String = name.chars().map(|c| if c < ' ' || "\"*/:<>?\\|".contains(c) { '_' } else { c }).collect();
    let trimmed = replaced.trim_end_matches([' ', '.']);
    if trimmed.is_empty() { "_".to_owned() } else { trimmed.to_owned() }
}

struct Converter<'a, R> {
    image: &'a mut R,
    iso_label: &'a str,
    fat_label: &'a str,
    patched: Vec<String>,
    skipped_symlinks: Vec<String>,
    merged_duplicates: Vec<String>,
}

impl<R: Read + Seek> Converter<'_, R> {
    fn convert(&mut self, entries: &[IsoEntry], dir_path: &str) -> Result<Vec<Node>, Error> {
        let mut nodes: Vec<Node> = Vec::with_capacity(entries.len());
        // Upper-cased name -> index in `nodes`.
        let mut by_name: HashMap<String, usize> = HashMap::new();
        for entry in entries {
            let name = fat_name(&entry.name);
            let path = format!("{dir_path}/{name}");
            let kind = match &entry.kind {
                IsoKind::Symlink => {
                    self.skipped_symlinks.push(path);
                    continue;
                }
                IsoKind::Dir(children) => NodeKind::Dir(self.convert(children, &path)?),
                IsoKind::File(extents) => {
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

    /// Rewrite the ISO label to the FAT label in boot configuration files.
    fn patch_config(&mut self, name: &str, path: &str, sources: Vec<Source>) -> Result<Vec<Source>, Error> {
        let lower = name.to_ascii_lowercase();
        let len: u64 = sources.iter().map(Source::len).sum();
        if self.iso_label.is_empty()
            || self.iso_label == self.fat_label
            || len > MAX_CONFIG_BYTES
            || !CONFIG_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
        {
            return Ok(sources);
        }
        let mut text = vec![0u8; len as usize];
        fill(self.image, &sources, 0, &mut text)?;
        match patch_label(&text, self.iso_label, self.fat_label) {
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
fn fill<R: Read + Seek>(image: &mut R, sources: &[Source], mut offset: u64, mut buf: &mut [u8]) -> io::Result<()> {
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
    use fatfs::{FileSystem, FsOptions};
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

    fn write_to_disk(iso: &[u8]) -> (IsoModeLayout, MemDisk) {
        let layout = layout(&mut Cursor::new(iso), DISK as u64, 512, 0x1234_5678, 0xCAFE_F00D).unwrap();
        let mut disk = MemDisk::new(DISK, 512);
        write_plan(&mut Cursor::new(iso), &layout.plan, &mut disk, &mut |_| {}).unwrap();
        (layout, disk)
    }

    fn read_file(fs: &FileSystem<Cursor<Vec<u8>>>, path: &str) -> Vec<u8> {
        let mut data = Vec::new();
        fs.root_dir().open_file(path).unwrap().read_to_end(&mut data).unwrap();
        data
    }

    #[test]
    fn copies_files_to_a_fat32_volume_with_patched_label() {
        let iso = fedora_like().build();
        let (layout, disk) = write_to_disk(&iso);
        assert_eq!(layout.fat_label, "FEDORA-KDE-");
        assert_eq!(layout.patched, ["/EFI/BOOT/grub.cfg"]);
        assert_eq!(layout.skipped_symlinks, ["/fedora"]);
        assert_eq!(layout.merged_duplicates, ["/EFI/BOOT/bootx64.efi"]);

        let partition = disk.data[1024 * 1024..].to_vec();
        let fs = FileSystem::new(Cursor::new(partition), FsOptions::new()).unwrap();
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
        let err = layout(&mut Cursor::new(iso.build()), DISK as u64, 512, 1, 1).unwrap_err();
        assert!(matches!(&err, Error::NameConflict { path } if path == "/EFI/BOOT/bootx64.efi"), "{err:?}");
    }

    #[test]
    fn iso_without_uefi_loader_is_refused() {
        let mut iso = IsoBuilder::new("TinyCore", IsoNaming::RockRidge);
        let boot = iso.dir(None, "boot");
        iso.file(Some(boot), "vmlinuz", b"kernel".to_vec());
        let err = layout(&mut Cursor::new(iso.build()), DISK as u64, 512, 1, 1).unwrap_err();
        assert!(matches!(err, Error::NoUefiBootloader), "{err:?}");
    }

    #[test]
    fn label_fits_fat_rules() {
        assert_eq!(fat_label("Fedora-KDE-Live-44"), "FEDORA-KDE-");
        assert_eq!(fat_label("Ubuntu 24.04 LTS"), "UBUNTU_24_0");
        assert_eq!(fat_label("ARCH_202410"), "ARCH_202410");
        assert_eq!(fat_label(""), "BOOTKILN");
    }

    #[test]
    fn label_patch_only_touches_whole_tokens() {
        assert_eq!(patch_label(b"archisolabel=ARCH_202410", "ARCH_202410", "ARCH_202410"), None);
        assert_eq!(patch_label(b"x=MYISO2 y=MYISO", "MYISO", "NEW").unwrap(), b"x=MYISO2 y=NEW");
        assert_eq!(patch_label(b"LABEL=My\\x20Disk", "My Disk", "MY_DISK").unwrap(), b"LABEL=MY_DISK");
    }
}
