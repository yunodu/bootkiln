use std::fs::File;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use bootkiln_core::copy::{copy_tree, total_len, verify_tree};
use bootkiln_core::image::{self, ImageInfo, ImageKind};
use bootkiln_core::iso_mode::{self, FileSystem, IsoContent};
use bootkiln_core::uefi_ntfs::{self, NtfsLayout};
use bootkiln_core::unattend::{self, Customisations};
use bootkiln_core::{DiskBackend, Drive, Error, Plan, verify_plan, write_plan};
use bootkiln_platform::{NativeBackend, host_region};
use clap::{Args, Parser, Subcommand, ValueEnum};
use indicatif::{HumanBytes, ProgressBar, ProgressStyle};

/// NTFS metadata (MFT, log, bitmaps) on top of the file data, as a fraction and a floor.
const NTFS_OVERHEAD_PERCENT: u64 = 1;
const NTFS_OVERHEAD_MIN: u64 = 64 * 1024 * 1024;

#[derive(Parser)]
#[command(name = "bootkiln", version, about = "Write bootable disk images to USB drives")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List removable USB / SD drives that can be written
    List,
    /// Erase a drive, write an image to it, and verify the result
    Write(WriteArgs),
    /// Compare a drive written in DD mode with an image
    Verify {
        image: PathBuf,
        #[arg(short, long)]
        drive: u32,
    },
}

#[derive(Args)]
struct WriteArgs {
    /// Image file (.iso / .img)
    image: PathBuf,
    /// Target disk number from `bootkiln list`
    #[arg(short, long)]
    drive: u32,
    /// How to put the image on the drive
    #[arg(long, value_enum, default_value_t = Mode::Auto)]
    mode: Mode,
    /// Skip the interactive confirmation
    #[arg(long)]
    yes: bool,
    /// Write images that do not look raw-bootable (DD mode)
    #[arg(long)]
    force: bool,
    /// Skip read-back verification
    #[arg(long)]
    no_verify: bool,
    #[command(flatten)]
    windows: WindowsOptions,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Mode {
    /// ISO mode for ISO images with a UEFI boot loader, otherwise DD mode
    Auto,
    /// Copy the ISO's files onto a fresh drive: FAT32, or NTFS + UEFI:NTFS when a file
    /// is 4 GiB or larger (UEFI boot)
    Iso,
    /// Raw byte-for-byte copy of the image
    Dd,
}

/// Windows Setup customisations, written to `autounattend.xml` on the drive.
#[derive(Args)]
#[command(next_help_heading = "Windows installer options")]
struct WindowsOptions {
    /// Windows 11: skip the TPM 2.0, Secure Boot and 4 GB RAM checks
    #[arg(long)]
    win_skip_hw_checks: bool,
    /// Allow setup without a Microsoft account
    #[arg(long)]
    win_skip_online_account: bool,
    /// Create this local administrator (empty password, to be changed at first sign-in)
    #[arg(long, value_name = "NAME")]
    win_local_account: Option<String>,
    /// Use this PC's region, keyboard layout and time zone
    #[arg(long)]
    win_copy_region: bool,
    /// Answer the privacy questions with the most private choices and skip them
    #[arg(long)]
    win_skip_privacy: bool,
    /// Turn off BitLocker's automatic device encryption
    #[arg(long)]
    win_no_bitlocker: bool,
}

impl WindowsOptions {
    fn customisations(&self) -> Result<Customisations> {
        let region = match self.win_copy_region {
            true => Some(host_region().context("reading this PC's regional settings")?),
            false => None,
        };
        Ok(Customisations {
            skip_hardware_checks: self.win_skip_hw_checks,
            skip_online_account: self.win_skip_online_account,
            local_account: self.win_local_account.clone(),
            region,
            skip_privacy_questions: self.win_skip_privacy,
            disable_bitlocker: self.win_no_bitlocker,
        })
    }
}

fn main() -> Result<()> {
    let backend = NativeBackend;
    match Cli::parse().command {
        Command::List => list(&backend),
        Command::Write(args) => write(&backend, &args),
        Command::Verify { image, drive } => verify(&backend, &image, drive),
    }
}

fn list(backend: &dyn DiskBackend) -> Result<()> {
    let drives: Vec<Drive> =
        backend.list_drives().context("listing drives")?.into_iter().filter(Drive::is_safe_target).collect();
    if drives.is_empty() {
        println!("No removable USB / SD drives found.");
        return Ok(());
    }
    println!("{:>4}  {:>10}  {:<4}  {:<32}  Mounted at", "#", "Size", "Bus", "Model");
    for d in &drives {
        println!(
            "{:>4}  {:>10}  {:<4}  {:<32}  {}",
            d.number,
            HumanBytes(d.size_bytes).to_string(),
            format!("{:?}", d.bus).to_uppercase(),
            d.model,
            d.mount_points.join(" ")
        );
    }
    Ok(())
}

/// What `write` does to the drive.
enum Job {
    /// Raw device writes only: DD mode, or ISO mode on FAT32.
    Device { plan: Plan, iso_mode: bool },
    /// Partition table + UEFI:NTFS partition, then format and copy files.
    Ntfs { layout: NtfsLayout, content: Box<IsoContent> },
}

fn write(backend: &dyn DiskBackend, args: &WriteArgs) -> Result<()> {
    let WriteArgs { image: image_path, drive: drive_no, mode, yes, force, no_verify, windows } = args;
    let (mode, force, verify) = (*mode, *force, !no_verify);
    require_elevated(backend)?;
    let customisations = windows.customisations()?;
    let (mut file, info) = open_image(image_path)?;
    let drive = target_drive(backend, *drive_no)?;
    if backend.disk_numbers_for_path(image_path).context("locating the image's disk")?.contains(&drive.number) {
        bail!("the image is stored on drive {}; copy it elsewhere first", drive.number);
    }

    println!("Image: {} ({})", image_path.display(), HumanBytes(info.len));
    println!("Drive: {}", describe(&drive));
    let content = match mode {
        Mode::Dd => None,
        Mode::Auto | Mode::Iso => Some(iso_mode::prepare(&mut file)),
    };
    let mut content = match (mode, content) {
        (_, Some(Ok(content))) => Some(content),
        (Mode::Iso, Some(Err(e))) => return Err(e).context("ISO mode is not possible for this image"),
        (_, Some(Err(e))) => {
            if !matches!(e, Error::NotIso9660) {
                eprintln!("note: ISO mode is not possible ({e}); using DD mode");
            }
            None
        }
        (_, None) => None,
    };

    if !customisations.is_empty() {
        let Some((content, windows)) = content.as_mut().and_then(|c| c.windows.map(|w| (c, w))) else {
            bail!("the --win-* options need a Windows installer ISO written in ISO mode");
        };
        let answer_file = unattend::answer_file(&customisations, &windows)?;
        content.add_root_file(unattend::FILE_NAME, answer_file.into_bytes());
    }

    let [volume_id, disk_signature] = random_ids();
    let job = match content {
        None => {
            println!("Mode:  DD (raw copy of the image)");
            Job::Device { plan: dd_plan(image_path, &info, &drive, force)?, iso_mode: false }
        }
        Some(content) => {
            print_iso_mode(&content, !customisations.is_empty());
            match content.file_system {
                FileSystem::Fat32 => Job::Device {
                    plan: content.fat32_plan(drive.size_bytes, drive.sector_size, volume_id, disk_signature)?,
                    iso_mode: true,
                },
                FileSystem::Ntfs => {
                    let layout = uefi_ntfs::layout(drive.size_bytes, drive.sector_size, disk_signature)?;
                    let data = total_len(content.nodes());
                    let needed = data + (data * NTFS_OVERHEAD_PERCENT / 100).max(NTFS_OVERHEAD_MIN);
                    if needed > layout.ntfs_len {
                        return Err(Error::DriveTooSmall { needed, available: layout.ntfs_len }.into());
                    }
                    Job::Ntfs { layout, content: Box::new(content) }
                }
            }
        }
    };
    if !yes && !confirm(&drive)? {
        bail!("aborted");
    }

    match &job {
        Job::Device { plan, .. } => write_device(backend, &drive, &mut file, plan, verify)?,
        Job::Ntfs { layout, content } => {
            write_device(backend, &drive, &mut file, &layout.plan, verify)?;
            let root = backend
                .format_ntfs(&drive, layout.ntfs_offset, &content.label)
                .with_context(|| format!("formatting {}", drive.path))?;
            let len = total_len(content.nodes());
            let bar = progress_bar(len, "Copying");
            copy_tree(&mut file, content.nodes(), &root, &mut |n| bar.set_position(n))?;
            bar.finish();
            backend.dismount_volume(&root).context("flushing the NTFS volume")?;
            if verify {
                let bar = progress_bar(len, "Verifying");
                verify_tree(&mut file, content.nodes(), &root, &mut |n| bar.set_position(n))?;
                bar.finish();
            }
        }
    }

    println!("Done. {} is ready.", drive.path);
    match job {
        Job::Device { iso_mode: false, .. } => println!(
            "Note: Windows may say the drive needs to be formatted. That is expected for bootable images; \
             choose Cancel, or the drive will be erased."
        ),
        Job::Device { iso_mode: true, .. } => {
            println!("Note: the drive boots on UEFI PCs; no legacy BIOS boot loader is installed.")
        }
        Job::Ntfs { .. } => println!(
            "Note: the drive boots on UEFI PCs through UEFI:NTFS; no legacy BIOS boot loader is installed.\n\
             UEFI:NTFS is signed with Microsoft's third-party UEFI certificate: with Secure Boot on, PCs that \
             trust only Windows' own certificate need \"Allow Microsoft 3rd party UEFI CA\" enabled."
        ),
    }
    Ok(())
}

/// Write `plan` to the raw drive and read it back.
fn write_device(backend: &dyn DiskBackend, drive: &Drive, image: &mut File, plan: &Plan, verify: bool) -> Result<()> {
    let mut device = backend.open_exclusive(drive).with_context(|| format!("opening {}", drive.path))?;
    let bar = progress_bar(plan.len(), "Writing");
    write_plan(image, plan, device.as_mut(), &mut |n| bar.set_position(n))?;
    bar.finish();
    if verify {
        let bar = progress_bar(plan.len(), "Verifying");
        verify_plan(image, plan, device.as_mut(), &mut |n| bar.set_position(n))?;
        bar.finish();
    }
    Ok(())
}

fn print_iso_mode(content: &IsoContent, answer_file: bool) {
    match content.file_system {
        FileSystem::Fat32 => println!("Mode:  ISO (files copied to FAT32 volume {}, UEFI boot)", content.label),
        FileSystem::Ntfs => println!(
            "Mode:  ISO (files copied to NTFS volume {} + UEFI:NTFS boot partition, UEFI boot)\n       \
             NTFS because FAT32 cannot hold: {}",
            content.label,
            content.large_files.join(", ")
        ),
    }
    if let Some(windows) = content.windows {
        let build = windows.build.map_or_else(|| "unknown build".to_owned(), |b| format!("build {b}"));
        println!("       Windows installer ({}, {build})", windows.arch.unattend_name());
    }
    if answer_file {
        println!("       answer file added: \\{}", unattend::FILE_NAME);
    }
    if !content.patched.is_empty() {
        println!(
            "       boot config rewritten from label {} to {}: {}",
            content.iso_label,
            content.label,
            content.patched.join(", ")
        );
    }
    if !content.merged_duplicates.is_empty() {
        println!("       identical case-variant files kept once: {}", content.merged_duplicates.join(", "));
    }
    if !content.skipped_symlinks.is_empty() {
        println!("       symbolic links skipped: {}", content.skipped_symlinks.join(", "));
    }
}

/// Raw copy, if the image looks raw-bootable (or `force`).
fn dd_plan(image_path: &Path, info: &ImageInfo, drive: &Drive, force: bool) -> Result<Plan> {
    match info.kind {
        ImageKind::Hybrid => {}
        ImageKind::BiosOnly => eprintln!(
            "warning: {} boots only via legacy BIOS (no GPT or EFI system partition).\n\
             UEFI PCs will not list the drive as bootable unless legacy boot (CSM) is enabled \
             and Secure Boot is off; many recent PCs have no CSM at all.",
            image_path.display()
        ),
        _ if force => eprintln!("warning: image is not raw-bootable; writing anyway (--force)"),
        ImageKind::OpticalOnly => bail!(
            "{} is an ISO 9660/UDF image without a boot record or UEFI boot loader.\n\
             A raw copy will not boot from USB. Pass --force to write it anyway.",
            image_path.display()
        ),
        ImageKind::Unknown => bail!(
            "{} has neither a boot record nor an ISO 9660 descriptor; pass --force to write it anyway",
            image_path.display()
        ),
    }
    if info.len > drive.size_bytes {
        bail!("image ({}) does not fit on drive {} ({})", HumanBytes(info.len), drive.number, HumanBytes(drive.size_bytes));
    }
    Ok(Plan::raw_image(info.len))
}

/// FAT volume ID and MBR disk signature; Windows needs the signature unique among disks.
fn random_ids() -> [u32; 2] {
    let random = RandomState::new().build_hasher().finish();
    [random as u32, (random >> 32) as u32]
}

fn verify(backend: &dyn DiskBackend, image_path: &Path, drive_no: u32) -> Result<()> {
    require_elevated(backend)?;
    let (mut file, info) = open_image(image_path)?;
    let drive = target_drive(backend, drive_no)?;
    let mut device = backend.open_exclusive(&drive).with_context(|| format!("opening {}", drive.path))?;
    let bar = progress_bar(info.len, "Verifying");
    verify_plan(&mut file, &Plan::raw_image(info.len), device.as_mut(), &mut |n| bar.set_position(n))?;
    bar.finish();
    println!("OK: {} matches {}", drive.path, image_path.display());
    Ok(())
}

fn require_elevated(backend: &dyn DiskBackend) -> Result<()> {
    if !backend.is_elevated() {
        bail!("raw disk access needs administrator rights; run bootkiln from an elevated terminal");
    }
    Ok(())
}

fn open_image(path: &Path) -> Result<(File, ImageInfo)> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let info = image::inspect(&mut file).with_context(|| format!("reading {}", path.display()))?;
    Ok((file, info))
}

/// Re-reads the drive list so the number refers to the drive as it is now.
fn target_drive(backend: &dyn DiskBackend, number: u32) -> Result<Drive> {
    let drive = backend
        .list_drives()
        .context("listing drives")?
        .into_iter()
        .find(|d| d.number == number)
        .with_context(|| format!("no drive {number}; run `bootkiln list`"))?;
    if drive.is_system {
        bail!("drive {number} holds the running Windows installation; refusing to write it");
    }
    if !drive.is_safe_target() {
        bail!("drive {number} ({}) is not a removable USB / SD drive; refusing to write it", drive.model);
    }
    Ok(drive)
}

fn describe(d: &Drive) -> String {
    let mounts = if d.mount_points.is_empty() { String::new() } else { format!(", mounted at {}", d.mount_points.join(" ")) };
    format!("{} {} ({}{mounts})", d.path, d.model, HumanBytes(d.size_bytes))
}

fn confirm(drive: &Drive) -> Result<bool> {
    print!("ALL DATA on drive {} will be erased. Type {} to continue: ", drive.number, drive.number);
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim() == drive.number.to_string())
}

fn progress_bar(len: u64, label: &'static str) -> ProgressBar {
    let bar = ProgressBar::new(len).with_message(label);
    bar.set_style(
        ProgressStyle::with_template("{msg:>9} [{bar:40}] {bytes}/{total_bytes} {binary_bytes_per_sec} ETA {eta}")
            .expect("valid progress template")
            .progress_chars("=> "),
    );
    bar
}
