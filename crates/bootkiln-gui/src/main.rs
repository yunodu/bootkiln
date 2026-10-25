//! bootkiln's desktop GUI: a Tauri shell around the same write workflow as the CLI
//! (`bootkiln_core::job`). The window is the Svelte app in `ui/`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use bootkiln_core::iso_mode::FileSystem;
use bootkiln_core::job::{self, Analysis, Job, Method, Note, Phase};
use bootkiln_core::unattend::Customisations;
use bootkiln_core::{DiskBackend, Drive, Error};
use bootkiln_platform::{NativeBackend, host_region, windows_build};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

const DRIVE_POLL: Duration = Duration::from_millis(1500);
/// Windows 11's first build; Mica backgrounds need it.
const FIRST_MICA_BUILD: u32 = 22000;
/// Progress events are throttled to this interval (phase changes are always sent).
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Default)]
struct WriteState {
    busy: AtomicBool,
    cancel: Arc<AtomicBool>,
}

#[derive(Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct DriveView {
    number: u32,
    model: String,
    size_bytes: u64,
    bus: &'static str,
    mount_points: Vec<String>,
}

impl From<&Drive> for DriveView {
    fn from(d: &Drive) -> Self {
        let bus = match d.bus {
            bootkiln_core::BusKind::Usb => "USB",
            bootkiln_core::BusKind::Sd => "SD",
            bootkiln_core::BusKind::Other => "Removable",
        };
        Self { number: d.number, model: d.model.clone(), size_bytes: d.size_bytes, bus, mount_points: d.mount_points.clone() }
    }
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum ModeArg {
    Auto,
    Iso,
    Dd,
}

impl From<ModeArg> for job::Mode {
    fn from(mode: ModeArg) -> Self {
        match mode {
            ModeArg::Auto => job::Mode::Auto,
            ModeArg::Iso => job::Mode::Iso,
            ModeArg::Dd => job::Mode::Dd,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WindowsView {
    arch: &'static str,
    build: Option<u32>,
    is_windows11: Option<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageView {
    size_bytes: u64,
    /// "iso" or "dd".
    method: &'static str,
    /// "fat32" / "ntfs" in ISO mode.
    file_system: Option<&'static str>,
    label: Option<String>,
    iso_label: Option<String>,
    large_files: Vec<String>,
    patched: Vec<String>,
    skipped_symlinks: Vec<String>,
    windows: Option<WindowsView>,
    bios_only: bool,
    forced: bool,
    iso_unavailable: Option<String>,
}

impl From<&Analysis> for ImageView {
    fn from(a: &Analysis) -> Self {
        let mut view = ImageView {
            size_bytes: a.info.len,
            method: "dd",
            file_system: None,
            label: None,
            iso_label: None,
            large_files: Vec::new(),
            patched: Vec::new(),
            skipped_symlinks: Vec::new(),
            windows: None,
            bios_only: false,
            forced: false,
            iso_unavailable: a.iso_unavailable.clone(),
        };
        match &a.method {
            Method::Dd { bios_only, forced } => {
                view.bios_only = *bios_only;
                view.forced = *forced;
            }
            Method::Iso(content) => {
                view.method = "iso";
                view.file_system = Some(match content.file_system {
                    FileSystem::Fat32 => "fat32",
                    FileSystem::Ntfs => "ntfs",
                });
                view.label = Some(content.label.clone());
                view.iso_label = Some(content.iso_label.clone());
                view.large_files = content.large_files.clone();
                view.patched = content.patched.clone();
                view.skipped_symlinks = content.skipped_symlinks.clone();
                view.windows = content.windows.map(|w| WindowsView {
                    arch: match w.arch {
                        bootkiln_core::windows_iso::Arch::Amd64 => "x64",
                        bootkiln_core::windows_iso::Arch::Arm64 => "ARM64",
                    },
                    build: w.build,
                    is_windows11: w.is_windows_11(),
                });
            }
        }
        view
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct WindowsOptions {
    skip_hardware_checks: bool,
    skip_online_account: bool,
    local_account: Option<String>,
    copy_region: bool,
    skip_privacy_questions: bool,
    disable_bitlocker: bool,
}

impl WindowsOptions {
    fn customisations(&self) -> Result<Customisations, Error> {
        let region = match self.copy_region {
            true => Some(host_region().map_err(|e| {
                Error::Io(std::io::Error::new(e.kind(), format!("reading this PC's regional settings: {e}")))
            })?),
            false => None,
        };
        Ok(Customisations {
            skip_hardware_checks: self.skip_hardware_checks,
            skip_online_account: self.skip_online_account,
            local_account: self.local_account.clone().filter(|n| !n.trim().is_empty()),
            region,
            skip_privacy_questions: self.skip_privacy_questions,
            disable_bitlocker: self.disable_bitlocker,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WriteRequest {
    image: PathBuf,
    drive: u32,
    mode: ModeArg,
    force: bool,
    verify: bool,
    windows: WindowsOptions,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent {
    /// "writing" | "verifying" | "formatting" | "copying" | "verifyingFiles".
    phase: &'static str,
    done: u64,
    total: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FinishedEvent {
    ok: bool,
    cancelled: bool,
    error: Option<String>,
    /// "formatPrompt" | "uefiOnly" | "uefiNtfs" on success.
    note: Option<&'static str>,
}

fn safe_drives(backend: &dyn DiskBackend) -> Result<Vec<DriveView>, String> {
    let drives = backend.list_drives().map_err(|e| format!("listing drives: {e}"))?;
    Ok(drives.iter().filter(|d| d.is_safe_target()).map(DriveView::from).collect())
}

/// Whether the window has a Mica backdrop (Windows 11); the UI paints its own
/// background otherwise.
#[tauri::command]
fn mica() -> bool {
    windows_build().is_some_and(|b| b >= FIRST_MICA_BUILD)
}

#[tauri::command]
fn list_drives() -> Result<Vec<DriveView>, String> {
    safe_drives(&NativeBackend)
}

#[tauri::command]
async fn analyze_image(path: PathBuf, mode: ModeArg, force: bool) -> Result<ImageView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut file = open(&path).map_err(|e| e.to_string())?;
        let analysis = job::analyze(&mut file, mode.into(), force).map_err(|e| e.to_string())?;
        Ok(ImageView::from(&analysis))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Everything up to the first write: drive checks, analysis, answer file, layout.
fn prepare(request: &WriteRequest) -> Result<(File, Drive, Job), Error> {
    let backend = NativeBackend;
    let customisations = request.windows.customisations()?;
    let mut file = open(&request.image)?;
    let drive = job::target_drive(&backend, request.drive, &request.image)?;
    let mut analysis = job::analyze(&mut file, request.mode.into(), request.force)?;
    analysis.customise(&customisations)?;
    let job = analysis.plan(&drive)?;
    Ok((file, drive, job))
}

/// Validates a write without touching the drive, so problems show before confirmation.
#[tauri::command]
async fn check_write(request: WriteRequest) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || prepare(&request).map(|_| ()).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn start_write(app: AppHandle, state: State<'_, Arc<WriteState>>, request: WriteRequest) -> Result<(), String> {
    if state.busy.swap(true, Ordering::SeqCst) {
        return Err("a write is already running".into());
    }
    state.cancel.store(false, Ordering::SeqCst);
    let state = Arc::clone(&state);
    thread::spawn(move || {
        let result = prepare(&request).and_then(|(mut file, drive, job)| {
            let mut last: Option<(Phase, Instant)> = None;
            job.run(&mut file, &NativeBackend, &drive, request.verify, &mut |phase, done, total| {
                if state.cancel.load(Ordering::SeqCst) {
                    return Err(Error::Cancelled);
                }
                let due = last.is_none_or(|(p, at)| p != phase || at.elapsed() >= PROGRESS_INTERVAL || done == total);
                if due {
                    last = Some((phase, Instant::now()));
                    let _ = app.emit("write-progress", ProgressEvent { phase: phase_name(phase), done, total });
                }
                Ok(())
            })?;
            Ok(job.note())
        });
        let event = match result {
            Ok(note) => FinishedEvent { ok: true, cancelled: false, error: None, note: Some(note_name(note)) },
            Err(Error::Cancelled) => FinishedEvent { ok: false, cancelled: true, error: None, note: None },
            Err(e) => FinishedEvent { ok: false, cancelled: false, error: Some(e.to_string()), note: None },
        };
        state.busy.store(false, Ordering::SeqCst);
        let _ = app.emit("write-finished", event);
    });
    Ok(())
}

#[tauri::command]
fn cancel_write(state: State<'_, Arc<WriteState>>) {
    state.cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
async fn eject_drive(number: u32) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let backend = NativeBackend;
        let drives = backend.list_drives().map_err(|e| e.to_string())?;
        let drive = drives.iter().find(|d| d.number == number && d.is_safe_target()).ok_or("the drive is gone")?;
        backend.eject(drive).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Writing => "writing",
        Phase::Verifying => "verifying",
        Phase::Formatting => "formatting",
        Phase::Copying => "copying",
        Phase::VerifyingFiles => "verifyingFiles",
    }
}

fn note_name(note: Note) -> &'static str {
    match note {
        Note::FormatPrompt => "formatPrompt",
        Note::UefiOnly => "uefiOnly",
        Note::UefiNtfs => "uefiNtfs",
    }
}

fn open(path: &Path) -> Result<File, Error> {
    File::open(path).map_err(|e| Error::Io(std::io::Error::new(e.kind(), format!("opening {}: {e}", path.display()))))
}

/// Emits `drives-changed` whenever the list of writable drives changes (plug / unplug).
fn watch_drives(app: AppHandle, state: Arc<WriteState>) {
    thread::spawn(move || {
        let mut last = None;
        loop {
            // Listing opens every disk; skip it while a write holds the target locked.
            if !state.busy.load(Ordering::SeqCst)
                && let Ok(drives) = safe_drives(&NativeBackend)
                && last.as_ref() != Some(&drives)
            {
                let _ = app.emit("drives-changed", &drives);
                last = Some(drives);
            }
            thread::sleep(DRIVE_POLL);
        }
    });
}

fn main() {
    let state = Arc::new(WriteState::default());
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Arc::clone(&state))
        .setup(move |app| {
            watch_drives(app.handle().clone(), state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            mica,
            list_drives,
            analyze_image,
            check_write,
            start_write,
            cancel_write,
            eject_drive
        ])
        .run(tauri::generate_context!())
        .expect("running bootkiln");
}
