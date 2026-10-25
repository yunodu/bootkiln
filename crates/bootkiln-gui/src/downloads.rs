//! OS catalog and download commands (see `bootkiln_catalog`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use bootkiln_catalog::{Entry, Error, Release, Stage};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

/// Progress events are throttled to this interval (stage changes are always sent).
const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);

const RUNNING: u8 = 0;
const PAUSE: u8 = 1;
const CANCEL: u8 = 2;

#[derive(Default)]
pub struct DownloadState {
    busy: AtomicBool,
    /// `RUNNING`, or what the user asked for.
    request: AtomicU8,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent {
    stage: Stage,
    done: u64,
    total: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FinishedEvent {
    /// Verified file, on success.
    path: Option<String>,
    paused: bool,
    cancelled: bool,
    error: Option<String>,
}

#[tauri::command]
pub fn catalog_entries() -> Vec<Entry> {
    bootkiln_catalog::ENTRIES.to_vec()
}

/// Microsoft's name for this PC's language, for the Windows entry.
#[tauri::command]
pub fn default_windows_language() -> String {
    bootkiln_platform::host_region()
        .ok()
        .and_then(|r| bootkiln_catalog::default_windows_language(&r.user_locale))
        .unwrap_or(bootkiln_catalog::WINDOWS_LANGUAGE_FALLBACK)
        .to_owned()
}

#[tauri::command]
pub async fn resolve_release(id: String, language: Option<String>) -> Result<Release, String> {
    tauri::async_runtime::spawn_blocking(move || {
        bootkiln_catalog::resolve(&id, language.as_deref()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn start_download(app: AppHandle, state: State<'_, Arc<DownloadState>>, release: Release) -> Result<(), String> {
    if state.busy.swap(true, Ordering::SeqCst) {
        return Err("a download is already running".into());
    }
    state.request.store(RUNNING, Ordering::SeqCst);
    let state = Arc::clone(&state);
    thread::spawn(move || {
        let result = bootkiln_catalog::default_download_dir().map_err(Error::Io).and_then(|dir| {
            let mut last: Option<(Stage, Instant)> = None;
            bootkiln_catalog::download(&release, &dir, &mut |stage, done, total| {
                match state.request.load(Ordering::SeqCst) {
                    PAUSE => return Err(Error::Paused),
                    CANCEL => return Err(Error::Cancelled),
                    _ => {}
                }
                if last.is_none_or(|(s, at)| s != stage || at.elapsed() >= PROGRESS_INTERVAL) {
                    last = Some((stage, Instant::now()));
                    let _ = app.emit("download-progress", ProgressEvent { stage, done, total });
                }
                Ok(())
            })
        });
        let event = match result {
            Ok(path) => FinishedEvent { path: Some(path.display().to_string()), paused: false, cancelled: false, error: None },
            Err(Error::Paused) => FinishedEvent { path: None, paused: true, cancelled: false, error: None },
            Err(Error::Cancelled) => FinishedEvent { path: None, paused: false, cancelled: true, error: None },
            Err(e) => FinishedEvent { path: None, paused: false, cancelled: false, error: Some(e.to_string()) },
        };
        state.busy.store(false, Ordering::SeqCst);
        let _ = app.emit("download-finished", event);
    });
    Ok(())
}

/// Stop the download and keep the partial file, so `start_download` resumes it.
#[tauri::command]
pub fn pause_download(state: State<'_, Arc<DownloadState>>) {
    state.request.store(PAUSE, Ordering::SeqCst);
}

/// Stop the download and delete the partial file.
#[tauri::command]
pub fn cancel_download(state: State<'_, Arc<DownloadState>>) {
    state.request.store(CANCEL, Ordering::SeqCst);
}

/// Delete a paused or failed download's partial file.
#[tauri::command]
pub fn discard_download(release: Release) -> Result<(), String> {
    let dir = bootkiln_catalog::default_download_dir().map_err(|e| e.to_string())?;
    bootkiln_catalog::discard_partial(&release, &dir).map_err(|e| e.to_string())
}
