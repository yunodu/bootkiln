use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use bootkiln_core::image;
use bootkiln_core::iso_mode::{FileSystem, IsoContent};
use bootkiln_core::job::{self, Analysis, Method, Note, Phase};
use bootkiln_core::unattend::{self, Customisations};
use bootkiln_core::{DiskBackend, Drive, Plan, verify_plan};
use bootkiln_platform::{NativeBackend, host_region};
use clap::{Args, Parser, Subcommand, ValueEnum};
use indicatif::{HumanBytes, ProgressBar, ProgressStyle};

/// `--json-progress` lines are throttled to this interval (phase changes always print).
const JSON_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

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
    /// List the operating systems bootkiln can download, with their latest versions
    Catalog {
        /// Windows language (a name from the Windows entry's list; default: this PC's)
        #[arg(long, value_name = "LANGUAGE")]
        lang: Option<String>,
    },
    /// Download an operating system from the catalog and verify its checksum
    Download {
        /// Catalog id from `bootkiln catalog`, e.g. `ubuntu` or `windows11`
        id: String,
        /// Windows language, e.g. "English" or "German" (default: this PC's)
        #[arg(long, value_name = "LANGUAGE")]
        lang: Option<String>,
        /// Folder to download into (default: `downloads` next to bootkiln)
        #[arg(long)]
        dir: Option<PathBuf>,
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
    /// For the GUI's privileged helper: no confirmation, JSON progress lines on stdout,
    /// cancel with a `cancel` line (or end of input) on stdin
    #[arg(long, hide = true)]
    json_progress: bool,
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

impl From<Mode> for job::Mode {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Auto => job::Mode::Auto,
            Mode::Iso => job::Mode::Iso,
            Mode::Dd => job::Mode::Dd,
        }
    }
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
        Command::Catalog { lang } => catalog(lang.as_deref()),
        Command::Download { id, lang, dir } => download(&id, lang.as_deref(), dir),
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

fn write(backend: &dyn DiskBackend, args: &WriteArgs) -> Result<()> {
    require_elevated(backend)?;
    if args.json_progress {
        return write_json(backend, args);
    }
    let customisations = args.windows.customisations()?;
    let mut file = open(&args.image)?;
    let drive = job::target_drive(backend, args.drive, &args.image)?;

    let mut analysis = job::analyze(&mut file, args.mode.into(), args.force)
        .with_context(|| format!("reading {}", args.image.display()))?;
    println!("Image: {} ({})", args.image.display(), HumanBytes(analysis.info.len));
    println!("Drive: {}", describe(&drive));
    analysis.customise(&customisations)?;
    print_method(&args.image, &analysis, !customisations.is_empty());

    let job = analysis.plan(&drive)?;
    if !args.yes && !confirm(&drive)? {
        bail!("aborted");
    }

    let mut bar: Option<(Phase, ProgressBar)> = None;
    job.run(&mut file, backend, &drive, !args.no_verify, &mut |phase, done, total| {
        if bar.as_ref().is_none_or(|(p, _)| *p != phase) {
            if let Some((_, old)) = bar.take() {
                old.finish();
            }
            bar = Some((phase, progress_bar(total, phase)));
        }
        if let Some((_, bar)) = &bar {
            bar.set_position(done);
        }
        Ok(())
    })?;
    if let Some((_, bar)) = bar {
        bar.finish();
    }

    println!("Done. {} is ready.", drive.path);
    println!(
        "Note: {}",
        match job.note() {
            Note::FormatPrompt => {
                "Windows may say the drive needs to be formatted. That is expected for bootable images; \
                 choose Cancel, or the drive will be erased."
            }
            Note::UefiOnly => "the drive boots on UEFI PCs; no legacy BIOS boot loader is installed.",
            Note::UefiNtfs => {
                "the drive boots on UEFI PCs through UEFI:NTFS; no legacy BIOS boot loader is installed.\n\
                 UEFI:NTFS is signed with Microsoft's third-party UEFI certificate: with Secure Boot on, PCs that \
                 trust only Windows' own certificate need \"Allow Microsoft 3rd party UEFI CA\" enabled."
            }
        }
    );
    Ok(())
}

/// `write --json-progress`: one JSON object per stdout line, `{"phase","done","total"}`
/// during the write, then `{"note"}` on success or `{"cancelled":true}`. Errors go to
/// stderr with a non-zero exit code.
fn write_json(backend: &dyn DiskBackend, args: &WriteArgs) -> Result<()> {
    let customisations = args.windows.customisations()?;
    let mut file = open(&args.image)?;
    let drive = job::target_drive(backend, args.drive, &args.image)?;
    let mut analysis = job::analyze(&mut file, args.mode.into(), args.force)
        .with_context(|| format!("reading {}", args.image.display()))?;
    analysis.customise(&customisations)?;
    let job = analysis.plan(&drive)?;

    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    std::thread::spawn(move || {
        let mut line = String::new();
        // A `cancel` line, end of input or a read error (the GUI is gone) all cancel.
        while io::stdin().read_line(&mut line).is_ok_and(|n| n > 0) && line.trim() != "cancel" {
            line.clear();
        }
        flag.store(true, Ordering::SeqCst);
    });

    let mut last: Option<(Phase, Instant)> = None;
    let result = job.run(&mut file, backend, &drive, !args.no_verify, &mut |phase, done, total| {
        if cancel.load(Ordering::SeqCst) {
            return Err(bootkiln_core::Error::Cancelled);
        }
        if last.is_none_or(|(p, at)| p != phase || at.elapsed() >= JSON_PROGRESS_INTERVAL || done == total) {
            last = Some((phase, Instant::now()));
            println!(r#"{{"phase":"{}","done":{done},"total":{total}}}"#, phase.name());
        }
        Ok(())
    });
    match result {
        Ok(()) => println!(r#"{{"note":"{}"}}"#, job.note().name()),
        Err(bootkiln_core::Error::Cancelled) => println!(r#"{{"cancelled":true}}"#),
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

fn print_method(image_path: &Path, analysis: &Analysis, answer_file: bool) {
    if let Some(reason) = &analysis.iso_unavailable {
        eprintln!("note: ISO mode is not possible ({reason}); using DD mode");
    }
    match &analysis.method {
        Method::Dd { bios_only, forced } => {
            println!("Mode:  DD (raw copy of the image)");
            if *bios_only {
                eprintln!(
                    "warning: {} boots only via legacy BIOS (no GPT or EFI system partition).\n\
                     UEFI PCs will not list the drive as bootable unless legacy boot (CSM) is enabled \
                     and Secure Boot is off; many recent PCs have no CSM at all.",
                    image_path.display()
                );
            }
            if *forced {
                eprintln!("warning: image is not raw-bootable; writing anyway (--force)");
            }
        }
        Method::Iso(content) => print_iso_mode(content, answer_file),
    }
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

fn verify(backend: &dyn DiskBackend, image_path: &Path, drive_no: u32) -> Result<()> {
    require_elevated(backend)?;
    let mut file = open(image_path)?;
    let info = image::inspect(&mut file).with_context(|| format!("reading {}", image_path.display()))?;
    let drive = job::target_drive(backend, drive_no, image_path)?;
    let mut device = backend.open_exclusive(&drive).with_context(|| format!("opening {}", drive.path))?;
    let bar = progress_bar(info.len, Phase::Verifying);
    verify_plan(&mut file, &Plan::raw_image(info.len), device.as_mut(), &mut |n| {
        bar.set_position(n);
        Ok(())
    })?;
    bar.finish();
    println!("OK: {} matches {}", drive.path, image_path.display());
    Ok(())
}

/// Microsoft's language name: the one given, else this PC's language, else English.
fn windows_language(lang: Option<&str>) -> String {
    lang.map(str::to_owned)
        .or_else(|| {
            bootkiln_platform::host_locale().and_then(|l| bootkiln_catalog::default_windows_language(&l)).map(str::to_owned)
        })
        .unwrap_or_else(|| bootkiln_catalog::WINDOWS_LANGUAGE_FALLBACK.to_owned())
}

fn catalog(lang: Option<&str>) -> Result<()> {
    let language = windows_language(lang);
    // Each entry asks a different server; resolve them side by side.
    let results: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = bootkiln_catalog::ENTRIES
            .iter()
            .map(|e| {
                let language = &language;
                s.spawn(move || bootkiln_catalog::resolve(e.id, e.windows.then_some(language.as_str())))
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("resolver thread")).collect()
    });
    println!("{:<20} {:<20} {:<18} {:>10}  File", "Id", "Name", "Version", "Size");
    for (entry, result) in bootkiln_catalog::ENTRIES.iter().zip(results) {
        match result {
            Ok(r) => println!(
                "{:<20} {:<20} {:<18} {:>10}  {}",
                entry.id,
                entry.name,
                r.version,
                r.size.map(|s| HumanBytes(s).to_string()).unwrap_or_default(),
                r.file_name
            ),
            Err(e) => println!("{:<20} {:<20} unavailable: {e}", entry.id, entry.name),
        }
    }
    Ok(())
}

fn download(id: &str, lang: Option<&str>, dir: Option<PathBuf>) -> Result<()> {
    let entry = bootkiln_catalog::ENTRIES
        .iter()
        .find(|e| e.id == id)
        .with_context(|| format!("no catalog entry {id}; run `bootkiln catalog`"))?;
    let language = entry.windows.then(|| windows_language(lang));
    let release = bootkiln_catalog::resolve(id, language.as_deref())?;
    let dir = match dir {
        Some(dir) => dir,
        None => bootkiln_catalog::default_download_dir()?,
    };
    println!("{} {} -> {}", entry.name, release.version, dir.join(&release.file_name).display());

    let mut bar: Option<(bootkiln_catalog::Stage, ProgressBar)> = None;
    let path = bootkiln_catalog::download(&release, &dir, &mut |stage, done, total| {
        if bar.as_ref().is_none_or(|(s, _)| *s != stage) {
            if let Some((_, old)) = bar.take() {
                old.finish();
            }
            let label = match stage {
                bootkiln_catalog::Stage::Checking => "Checking",
                bootkiln_catalog::Stage::Downloading => "Downloading",
            };
            bar = Some((stage, byte_bar(total, label)));
        }
        if let Some((_, bar)) = &bar {
            bar.set_length(total);
            bar.set_position(done);
        }
        Ok(())
    })?;
    if let Some((_, bar)) = bar {
        bar.finish();
    }
    println!("Verified (SHA-256 {}): {}", release.sha256, path.display());
    Ok(())
}

fn require_elevated(backend: &dyn DiskBackend) -> Result<()> {
    if !backend.is_elevated() {
        if cfg!(windows) {
            bail!("raw disk access needs administrator rights; run bootkiln from an elevated terminal");
        }
        bail!("raw disk access needs root; run bootkiln with sudo");
    }
    Ok(())
}

fn open(path: &Path) -> Result<File> {
    File::open(path).with_context(|| format!("opening {}", path.display()))
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

fn progress_bar(len: u64, phase: Phase) -> ProgressBar {
    let label = match phase {
        Phase::Writing => "Writing",
        Phase::Verifying | Phase::VerifyingFiles => "Verifying",
        Phase::Formatting => "Formatting",
        Phase::Copying => "Copying",
    };
    byte_bar(len, label)
}

fn byte_bar(len: u64, label: &'static str) -> ProgressBar {
    let bar = ProgressBar::new(len).with_message(label);
    bar.set_style(
        ProgressStyle::with_template("{msg:>11} [{bar:40}] {bytes}/{total_bytes} {binary_bytes_per_sec} ETA {eta}")
            .expect("valid progress template")
            .progress_chars("=> "),
    );
    bar
}
