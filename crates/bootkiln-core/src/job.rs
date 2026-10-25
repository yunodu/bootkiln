//! The write workflow shared by the CLI and the GUI: analyse an image, check the target
//! drive, plan the job for that drive, then run it with progress and cancellation.

use std::hash::{BuildHasher, Hasher, RandomState};
use std::io::{Read, Seek};
use std::path::Path;

use crate::copy::{copy_tree, total_len, verify_tree};
use crate::device::{DiskBackend, Drive};
use crate::error::Error;
use crate::image::{self, ImageInfo, ImageKind};
use crate::iso_mode::{self, FileSystem, IsoContent};
use crate::plan::{Plan, verify_plan, write_plan};
use crate::uefi_ntfs::{self, NtfsLayout};
use crate::unattend::{self, Customisations};
use crate::windows_iso::WindowsInfo;

/// NTFS metadata (MFT, log, bitmaps) on top of the file data, as a fraction and a floor.
const NTFS_OVERHEAD_PERCENT: u64 = 1;
const NTFS_OVERHEAD_MIN: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// ISO mode for ISO images with a UEFI boot loader, otherwise DD mode.
    Auto,
    /// Copy the ISO's files onto a fresh drive.
    Iso,
    /// Raw byte-for-byte copy.
    Dd,
}

/// What writing an image will do, before a drive is chosen.
#[derive(Debug)]
pub struct Analysis {
    pub info: ImageInfo,
    pub method: Method,
    /// Why ISO mode is not used for an ISO image under [`Mode::Auto`].
    pub iso_unavailable: Option<String>,
}

#[derive(Debug)]
pub enum Method {
    Iso(Box<IsoContent>),
    Dd {
        /// The image boots only via legacy BIOS (no GPT or EFI system partition).
        bios_only: bool,
        /// The image does not look raw-bootable and is written anyway.
        forced: bool,
    },
}

/// Inspect the image and decide how to write it. `force` allows DD writes of images that
/// do not look raw-bootable.
pub fn analyze<R: Read + Seek>(image: &mut R, mode: Mode, force: bool) -> Result<Analysis, Error> {
    let info = image::inspect(image)?;
    let iso = match mode {
        Mode::Dd => None,
        Mode::Auto | Mode::Iso => Some(iso_mode::prepare(image)),
    };
    let (method, iso_unavailable) = match (mode, iso) {
        (_, Some(Ok(content))) => (Method::Iso(Box::new(content)), None),
        (Mode::Iso, Some(Err(e))) => return Err(e),
        (_, Some(Err(Error::NotIso9660))) | (_, None) => (dd_method(&info, force)?, None),
        (_, Some(Err(e))) => (dd_method(&info, force)?, Some(e.to_string())),
    };
    Ok(Analysis { info, method, iso_unavailable })
}

fn dd_method(info: &ImageInfo, force: bool) -> Result<Method, Error> {
    match info.kind {
        ImageKind::Hybrid => Ok(Method::Dd { bios_only: false, forced: false }),
        ImageKind::BiosOnly => Ok(Method::Dd { bios_only: true, forced: false }),
        _ if force => Ok(Method::Dd { bios_only: false, forced: true }),
        ImageKind::OpticalOnly => Err(Error::OpticalOnlyImage),
        ImageKind::Unknown => Err(Error::UnknownImage),
    }
}

impl Analysis {
    pub fn iso_content(&self) -> Option<&IsoContent> {
        match &self.method {
            Method::Iso(content) => Some(content),
            Method::Dd { .. } => None,
        }
    }

    pub fn windows(&self) -> Option<WindowsInfo> {
        self.iso_content().and_then(|c| c.windows)
    }

    /// Add the Windows answer file for `options` (no-op when they are empty).
    pub fn customise(&mut self, options: &Customisations) -> Result<(), Error> {
        if options.is_empty() {
            return Ok(());
        }
        let windows = self.windows();
        let (Method::Iso(content), Some(windows)) = (&mut self.method, windows) else {
            return Err(Error::WindowsOption("Windows installer options need a Windows installer ISO written in ISO mode".into()));
        };
        let answer_file = unattend::answer_file(options, &windows)?;
        content.add_root_file(unattend::FILE_NAME, answer_file.into_bytes());
        Ok(())
    }

    /// Lay the job out for `drive`, checking that it fits.
    pub fn plan(self, drive: &Drive) -> Result<Job, Error> {
        let [volume_id, disk_signature] = random_ids();
        match self.method {
            Method::Dd { .. } => {
                if self.info.len > drive.size_bytes {
                    return Err(Error::ImageTooLarge { image: self.info.len, device: drive.size_bytes });
                }
                Ok(Job::Raw { plan: Plan::raw_image(self.info.len), iso_mode: false })
            }
            Method::Iso(content) => match content.file_system {
                FileSystem::Fat32 => Ok(Job::Raw {
                    plan: content.fat32_plan(drive.size_bytes, drive.sector_size, volume_id, disk_signature)?,
                    iso_mode: true,
                }),
                FileSystem::Ntfs => {
                    let layout = uefi_ntfs::layout(drive.size_bytes, drive.sector_size, disk_signature)?;
                    let data = total_len(content.nodes());
                    let needed = data + (data * NTFS_OVERHEAD_PERCENT / 100).max(NTFS_OVERHEAD_MIN);
                    if needed > layout.ntfs_len {
                        return Err(Error::DriveTooSmall { needed, available: layout.ntfs_len });
                    }
                    Ok(Job::Ntfs { layout, content })
                }
            },
        }
    }
}

/// The drive numbered `number`, if it may be written with the image at `image_path`.
/// Re-reads the drive list so the number refers to the drive as it is now.
pub fn target_drive(backend: &dyn DiskBackend, number: u32, image_path: &Path) -> Result<Drive, Error> {
    let drive = backend.list_drives()?.into_iter().find(|d| d.number == number).ok_or(Error::NoSuchDrive { number })?;
    if drive.is_system {
        return Err(Error::SystemDrive { number });
    }
    if !drive.is_safe_target() {
        return Err(Error::NotRemovable { number, model: drive.model });
    }
    if backend.disk_numbers_for_path(image_path)?.contains(&number) {
        return Err(Error::ImageOnDrive { number });
    }
    Ok(drive)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Raw device writes.
    Writing,
    /// Reading the raw device back.
    Verifying,
    /// The platform formats the NTFS partition (no byte progress).
    Formatting,
    /// Copying files onto the NTFS volume.
    Copying,
    /// Reading the copied files back.
    VerifyingFiles,
}

impl Phase {
    pub const ALL: [Phase; 5] = [Phase::Writing, Phase::Verifying, Phase::Formatting, Phase::Copying, Phase::VerifyingFiles];

    /// Stable name used between processes and by the GUI.
    pub fn name(self) -> &'static str {
        match self {
            Phase::Writing => "writing",
            Phase::Verifying => "verifying",
            Phase::Formatting => "formatting",
            Phase::Copying => "copying",
            Phase::VerifyingFiles => "verifyingFiles",
        }
    }
}

/// After-write advice for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    /// DD write: Windows may offer to format the drive; that would erase it.
    FormatPrompt,
    /// ISO mode on FAT32: UEFI boot only.
    UefiOnly,
    /// ISO mode on NTFS: UEFI boot through UEFI:NTFS, which needs Microsoft's third-party
    /// UEFI certificate trusted when Secure Boot is on.
    UefiNtfs,
}

impl Note {
    pub const ALL: [Note; 3] = [Note::FormatPrompt, Note::UefiOnly, Note::UefiNtfs];

    /// Stable name used between processes and by the GUI.
    pub fn name(self) -> &'static str {
        match self {
            Note::FormatPrompt => "formatPrompt",
            Note::UefiOnly => "uefiOnly",
            Note::UefiNtfs => "uefiNtfs",
        }
    }
}

#[derive(Debug)]
pub enum Job {
    /// Raw device writes only: DD mode, or ISO mode on FAT32.
    Raw { plan: Plan, iso_mode: bool },
    /// Partition table + UEFI:NTFS partition, then format and copy files.
    Ntfs { layout: NtfsLayout, content: Box<IsoContent> },
}

impl Job {
    pub fn note(&self) -> Note {
        match self {
            Job::Raw { iso_mode: false, .. } => Note::FormatPrompt,
            Job::Raw { iso_mode: true, .. } => Note::UefiOnly,
            Job::Ntfs { .. } => Note::UefiNtfs,
        }
    }

    /// Write the job to `drive`. `progress(phase, done, total)` is called as work proceeds;
    /// returning an error (e.g. [`Error::Cancelled`]) stops the job.
    pub fn run<R: Read + Seek>(
        &self,
        image: &mut R,
        backend: &dyn DiskBackend,
        drive: &Drive,
        verify: bool,
        progress: &mut dyn FnMut(Phase, u64, u64) -> Result<(), Error>,
    ) -> Result<(), Error> {
        match self {
            Job::Raw { plan, .. } => write_device(image, backend, drive, plan, verify, progress),
            Job::Ntfs { layout, content } => {
                write_device(image, backend, drive, &layout.plan, verify, progress)?;
                progress(Phase::Formatting, 0, 0)?;
                let root = backend.format_ntfs(drive, layout.ntfs_offset, &content.label)?;
                let total = total_len(content.nodes());
                let result = (|| -> Result<(), Error> {
                    copy_tree(image, content.nodes(), &root, &mut |n| progress(Phase::Copying, n, total))?;
                    backend.dismount_volume(&root)?;
                    if verify {
                        verify_tree(image, content.nodes(), &root, &mut |n| progress(Phase::VerifyingFiles, n, total))?;
                    }
                    Ok(())
                })();
                // Also after failure or cancellation, so no private mount is left behind.
                let released = backend.release_volume(&root).map_err(Error::from);
                result.and(released)
            }
        }
    }
}

fn write_device<R: Read + Seek>(
    image: &mut R,
    backend: &dyn DiskBackend,
    drive: &Drive,
    plan: &Plan,
    verify: bool,
    progress: &mut dyn FnMut(Phase, u64, u64) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut device = backend.open_exclusive(drive)?;
    let total = plan.len();
    write_plan(image, plan, device.as_mut(), &mut |n| progress(Phase::Writing, n, total))?;
    if verify {
        verify_plan(image, plan, device.as_mut(), &mut |n| progress(Phase::Verifying, n, total))?;
    }
    Ok(())
}

/// FAT volume ID and MBR disk signature; Windows needs the signature unique among disks.
fn random_ids() -> [u32; 2] {
    let random = RandomState::new().build_hasher().finish();
    [random as u32, (random >> 32) as u32]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::BusKind;
    use crate::testutil::{IsoBuilder, IsoNaming};
    use std::io::Cursor;

    fn drive(size: u64) -> Drive {
        Drive {
            number: 4,
            path: r"\\.\PhysicalDrive4".into(),
            model: "Test".into(),
            size_bytes: size,
            sector_size: 512,
            bus: BusKind::Usb,
            removable_media: true,
            mount_points: Vec::new(),
            is_system: false,
        }
    }

    fn uefi_iso() -> Vec<u8> {
        let mut iso = IsoBuilder::new("LIVE", IsoNaming::RockRidge);
        let efi = iso.dir(None, "EFI");
        let boot = iso.dir(Some(efi), "BOOT");
        iso.file(Some(boot), "BOOTX64.EFI", b"loader".to_vec());
        iso.build()
    }

    /// ISO 9660 descriptor plus an MBR boot signature, but no UEFI loader.
    fn bios_only_iso() -> Vec<u8> {
        let mut img = IsoBuilder::new("TC", IsoNaming::RockRidge).build();
        img[510..512].copy_from_slice(&[0x55, 0xAA]);
        img
    }

    #[test]
    fn mode_selection_follows_the_image() {
        let a = analyze(&mut Cursor::new(uefi_iso()), Mode::Auto, false).unwrap();
        assert!(matches!(a.method, Method::Iso(_)) && a.iso_unavailable.is_none());

        let a = analyze(&mut Cursor::new(bios_only_iso()), Mode::Auto, false).unwrap();
        assert!(matches!(a.method, Method::Dd { bios_only: true, forced: false }));
        assert_eq!(a.iso_unavailable.as_deref(), Some(Error::NoUefiBootloader.to_string().as_str()));

        let a = analyze(&mut Cursor::new(uefi_iso()), Mode::Dd, true).unwrap();
        assert!(matches!(a.method, Method::Dd { forced: true, .. }), "plain ISO without boot record needs force");
        assert!(matches!(analyze(&mut Cursor::new(uefi_iso()), Mode::Dd, false), Err(Error::OpticalOnlyImage)));
        assert!(matches!(analyze(&mut Cursor::new(bios_only_iso()), Mode::Iso, false), Err(Error::NoUefiBootloader)));
        assert!(matches!(analyze(&mut Cursor::new(vec![0u8; 64 * 1024]), Mode::Auto, false), Err(Error::UnknownImage)));
    }

    #[test]
    fn windows_options_need_a_windows_iso() {
        let mut a = analyze(&mut Cursor::new(uefi_iso()), Mode::Auto, false).unwrap();
        a.customise(&Customisations::default()).unwrap();
        let options = Customisations { disable_bitlocker: true, ..Default::default() };
        assert!(matches!(a.customise(&options), Err(Error::WindowsOption(_))));
    }

    #[test]
    fn dd_image_larger_than_drive_is_refused() {
        let img = bios_only_iso();
        let len = img.len() as u64;
        let a = analyze(&mut Cursor::new(img), Mode::Auto, false).unwrap();
        let err = a.plan(&drive(len - 512)).unwrap_err();
        assert!(matches!(err, Error::ImageTooLarge { image, .. } if image == len), "{err:?}");
    }
}
