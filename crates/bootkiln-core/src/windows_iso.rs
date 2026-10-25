//! Recognising Windows installer ISOs and reading their Windows build number.

use std::io::{Read, Seek};

use crate::disc::{self, DiscEntry, DiscKind};
use crate::error::Error;
use crate::iso_mode::fill;
use crate::plan::Source;

const WIM_MAGIC: &[u8; 8] = b"MSWIM\0\0\0";
const WIM_HEADER_LEN: usize = 208;
/// Resource header of the XML metadata: 7-byte size + flags, then the offset.
const WIM_XML_RESOURCE: usize = 72;
/// The XML lists every edition; real ones are tens of KiB.
const MAX_WIM_XML: u64 = 16 * 1024 * 1024;
/// Windows 11 reports build 22000 and later.
const FIRST_WINDOWS_11_BUILD: u32 = 22000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    Amd64,
    Arm64,
}

impl Arch {
    /// `processorArchitecture` value in answer files.
    pub fn unattend_name(self) -> &'static str {
        match self {
            Arch::Amd64 => "amd64",
            Arch::Arm64 => "arm64",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsInfo {
    pub arch: Arch,
    /// Build of the first image in `install.wim` / `install.esd`, when readable.
    pub build: Option<u32>,
}

impl WindowsInfo {
    pub fn is_windows_11(&self) -> Option<bool> {
        self.build.map(|b| b >= FIRST_WINDOWS_11_BUILD)
    }
}

/// `Some` for a Windows installer: a UEFI loader for x64 or ARM64 plus
/// `sources\install.wim` or `sources\install.esd`.
pub fn detect<R: Read + Seek>(image: &mut R, root: &[DiscEntry]) -> Result<Option<WindowsInfo>, Error> {
    let arch = if disc::find(root, &["efi", "boot", "bootx64.efi"]).is_some() {
        Arch::Amd64
    } else if disc::find(root, &["efi", "boot", "bootaa64.efi"]).is_some() {
        Arch::Arm64
    } else {
        return Ok(None);
    };
    let install = ["install.wim", "install.esd"].iter().find_map(|name| disc::find(root, &["sources", name]));
    let Some(DiscEntry { kind: DiscKind::File(extents), .. }) = install else { return Ok(None) };
    let sources: Vec<Source> = extents.iter().map(|e| Source::Image { offset: e.offset, len: e.len }).collect();
    Ok(Some(WindowsInfo { arch, build: wim_build(image, &sources)? }))
}

/// The first `<BUILD>` in the WIM's XML metadata; `None` if the file is not a WIM.
fn wim_build<R: Read + Seek>(image: &mut R, wim: &[Source]) -> Result<Option<u32>, Error> {
    let len: u64 = wim.iter().map(Source::len).sum();
    if len < WIM_HEADER_LEN as u64 {
        return Ok(None);
    }
    let mut header = [0u8; WIM_HEADER_LEN];
    fill(image, wim, 0, &mut header)?;
    if &header[..8] != WIM_MAGIC {
        return Ok(None);
    }
    let resource = &header[WIM_XML_RESOURCE..WIM_XML_RESOURCE + 16];
    let size = u64::from_le_bytes(resource[..8].try_into().expect("8 bytes")) & 0x00FF_FFFF_FFFF_FFFF;
    let offset = u64::from_le_bytes(resource[8..16].try_into().expect("8 bytes"));
    if size == 0 || size > MAX_WIM_XML || offset.checked_add(size).is_none_or(|end| end > len) {
        return Ok(None);
    }
    let mut xml = vec![0u8; size as usize];
    fill(image, wim, offset, &mut xml)?;
    let units: Vec<u16> = xml.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
    let text = String::from_utf16_lossy(&units);
    Ok(text
        .split_once("<BUILD>")
        .and_then(|(_, rest)| rest.split_once("</BUILD>"))
        .and_then(|(build, _)| build.trim().parse().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{UdfBuilder, UdfFileOpts};
    use std::io::Cursor;

    /// WIM header plus UTF-16LE XML metadata naming `build`.
    pub(crate) fn fake_wim(build: u32) -> Vec<u8> {
        let xml = format!("\u{feff}<WIM><IMAGE INDEX=\"1\"><WINDOWS><VERSION><BUILD>{build}</BUILD></VERSION></WINDOWS></IMAGE></WIM>");
        let xml: Vec<u8> = xml.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut wim = vec![0u8; WIM_HEADER_LEN];
        wim[..8].copy_from_slice(WIM_MAGIC);
        wim[72..79].copy_from_slice(&(xml.len() as u64).to_le_bytes()[..7]);
        wim[80..88].copy_from_slice(&(WIM_HEADER_LEN as u64 + 100).to_le_bytes());
        wim.resize(WIM_HEADER_LEN + 100, 0);
        wim.extend(xml);
        wim
    }

    fn windows_iso(loader: &str, install: &str, data: Vec<u8>) -> Vec<u8> {
        let mut udf = UdfBuilder::new("CCCOMA_X64FRE_EN-US_DV9");
        let efi = udf.dir(None, "efi");
        let boot = udf.dir(Some(efi), "boot");
        udf.file(Some(boot), loader, b"bootmgr".to_vec(), UdfFileOpts::default());
        let sources = udf.dir(None, "sources");
        udf.file(Some(sources), install, data, UdfFileOpts::default());
        udf.build()
    }

    fn detect_in(img: &[u8]) -> Option<WindowsInfo> {
        let tree = disc::read_tree(&mut Cursor::new(img)).unwrap();
        detect(&mut Cursor::new(img), &tree.root).unwrap()
    }

    #[test]
    fn reads_architecture_and_build() {
        let info = detect_in(&windows_iso("bootx64.efi", "install.wim", fake_wim(26300))).unwrap();
        assert_eq!(info, WindowsInfo { arch: Arch::Amd64, build: Some(26300) });
        assert_eq!(info.is_windows_11(), Some(true));

        let info = detect_in(&windows_iso("BOOTAA64.EFI", "install.esd", fake_wim(19045))).unwrap();
        assert_eq!(info, WindowsInfo { arch: Arch::Arm64, build: Some(19045) });
        assert_eq!(info.is_windows_11(), Some(false));
    }

    #[test]
    fn unreadable_wim_leaves_build_unknown() {
        let info = detect_in(&windows_iso("bootx64.efi", "install.wim", b"not a wim".to_vec())).unwrap();
        assert_eq!(info.build, None);
        assert_eq!(info.is_windows_11(), None);
    }

    #[test]
    fn linux_layouts_are_not_windows() {
        assert_eq!(detect_in(&windows_iso("bootx64.efi", "squashfs.img", Vec::new())), None);
        assert_eq!(detect_in(&windows_iso("grubx64.efi", "install.wim", fake_wim(26300))), None);
    }
}
