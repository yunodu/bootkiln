use std::fs::File;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use indicatif::{HumanBytes, ProgressBar, ProgressStyle};
use scribe_core::image::{self, ImageInfo, ImageKind};
use scribe_core::iso_mode::{self, IsoModeLayout};
use scribe_core::{DiskBackend, Drive, Error, Plan, verify_plan, write_plan};
use scribe_platform::NativeBackend;

#[derive(Parser)]
#[command(name = "scribe", version, about = "Write bootable disk images to USB drives")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List removable USB / SD drives that can be written
    List,
    /// Erase a drive, write an image to it, and verify the result
    Write {
        /// Image file (.iso / .img)
        image: PathBuf,
        /// Target disk number from `scribe list`
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
    },
    /// Compare a drive written in DD mode with an image
    Verify {
        image: PathBuf,
        #[arg(short, long)]
        drive: u32,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Mode {
    /// ISO mode for ISO images with a UEFI boot loader, otherwise DD mode
    Auto,
    /// Copy the ISO's files onto a fresh FAT32 drive (UEFI boot)
    Iso,
    /// Raw byte-for-byte copy of the image
    Dd,
}

fn main() -> Result<()> {
    let backend = NativeBackend;
    match Cli::parse().command {
        Command::List => list(&backend),
        Command::Write { image, drive, mode, yes, force, no_verify } => {
            write(&backend, &image, drive, mode, yes, force, !no_verify)
        }
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

fn write(
    backend: &dyn DiskBackend,
    image_path: &Path,
    drive_no: u32,
    mode: Mode,
    yes: bool,
    force: bool,
    verify: bool,
) -> Result<()> {
    require_elevated(backend)?;
    let (mut file, info) = open_image(image_path)?;
    let drive = target_drive(backend, drive_no)?;
    if backend.disk_numbers_for_path(image_path).context("locating the image's disk")?.contains(&drive.number) {
        bail!("the image is stored on drive {}; copy it elsewhere first", drive.number);
    }

    println!("Image: {} ({})", image_path.display(), HumanBytes(info.len));
    println!("Drive: {}", describe(&drive));
    let iso = match mode {
        Mode::Dd => None,
        Mode::Auto | Mode::Iso => {
            let [volume_id, disk_signature] = random_ids();
            Some(iso_mode::layout(&mut file, drive.size_bytes, drive.sector_size, volume_id, disk_signature))
        }
    };
    let iso_mode = iso.as_ref().is_some_and(Result::is_ok);
    let plan = match (mode, iso) {
        (_, Some(Ok(layout))) => {
            print_iso_mode(&layout);
            layout.plan
        }
        (Mode::Iso, Some(Err(e))) => return Err(e).context("ISO mode is not possible for this image"),
        (_, Some(Err(e))) => {
            if !matches!(e, Error::NotIso9660) {
                eprintln!("note: ISO mode is not possible ({e}); using DD mode");
            }
            dd_plan(image_path, &info, &drive, force)?
        }
        (_, None) => dd_plan(image_path, &info, &drive, force)?,
    };
    if !iso_mode {
        println!("Mode:  DD (raw copy of the image)");
    }
    if !yes && !confirm(&drive)? {
        bail!("aborted");
    }

    let mut device = backend.open_exclusive(&drive).with_context(|| format!("opening {}", drive.path))?;
    let bar = progress_bar(plan.len(), "Writing");
    write_plan(&mut file, &plan, device.as_mut(), &mut |n| bar.set_position(n))?;
    bar.finish();

    if verify {
        let bar = progress_bar(plan.len(), "Verifying");
        verify_plan(&mut file, &plan, device.as_mut(), &mut |n| bar.set_position(n))?;
        bar.finish();
    }
    println!("Done. {} is ready.", drive.path);
    if iso_mode {
        println!("Note: the drive boots on UEFI PCs; no legacy BIOS boot loader is installed.");
    } else {
        println!(
            "Note: Windows may say the drive needs to be formatted. That is expected for bootable images; \
             choose Cancel, or the drive will be erased."
        );
    }
    Ok(())
}

fn print_iso_mode(layout: &IsoModeLayout) {
    println!("Mode:  ISO (files copied to FAT32 volume {}, UEFI boot)", layout.fat_label);
    if !layout.patched.is_empty() {
        println!(
            "       boot config rewritten from label {} to {}: {}",
            layout.iso_label,
            layout.fat_label,
            layout.patched.join(", ")
        );
    }
    if !layout.merged_duplicates.is_empty() {
        println!("       identical case-variant files kept once: {}", layout.merged_duplicates.join(", "));
    }
    if !layout.skipped_symlinks.is_empty() {
        println!("       symbolic links skipped (FAT32 has none): {}", layout.skipped_symlinks.join(", "));
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
            "{} is a plain ISO 9660/UDF image without a boot record or UEFI boot loader \
             (typical for Windows installers, which are not supported yet).\n\
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
        bail!("raw disk access needs administrator rights; run scribe from an elevated terminal");
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
        .with_context(|| format!("no drive {number}; run `scribe list`"))?;
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
