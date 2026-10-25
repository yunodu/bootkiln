// Typed access to the Rust commands and events in `src/main.rs`.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";

export interface Drive {
  number: number;
  model: string;
  sizeBytes: number;
  bus: string;
  mountPoints: string[];
}

export interface WindowsInfo {
  arch: string;
  build: number | null;
  isWindows11: boolean | null;
}

export interface ImageInfo {
  sizeBytes: number;
  method: "iso" | "dd";
  fileSystem: "fat32" | "ntfs" | null;
  label: string | null;
  isoLabel: string | null;
  largeFiles: string[];
  patched: string[];
  skippedSymlinks: string[];
  windows: WindowsInfo | null;
  biosOnly: boolean;
  forced: boolean;
  isoUnavailable: string | null;
}

export type Mode = "auto" | "iso" | "dd";

export interface WindowsOptions {
  skipHardwareChecks: boolean;
  skipOnlineAccount: boolean;
  localAccount: string | null;
  copyRegion: boolean;
  skipPrivacyQuestions: boolean;
  disableBitlocker: boolean;
}

export interface WriteRequest {
  image: string;
  drive: number;
  mode: Mode;
  force: boolean;
  verify: boolean;
  windows: WindowsOptions;
}

export type Phase = "writing" | "verifying" | "formatting" | "copying" | "verifyingFiles";

export interface Progress {
  phase: Phase;
  done: number;
  total: number;
}

export type Note = "formatPrompt" | "uefiOnly" | "uefiNtfs";

export interface Finished {
  ok: boolean;
  cancelled: boolean;
  error: string | null;
  note: Note | null;
}

export const mica = () => invoke<boolean>("mica");
export const listDrives = () => invoke<Drive[]>("list_drives");
export const analyzeImage = (path: string, mode: Mode) =>
  invoke<ImageInfo>("analyze_image", { path, mode, force: false });
export const checkWrite = (request: WriteRequest) => invoke<void>("check_write", { request });
export const startWrite = (request: WriteRequest) => invoke<void>("start_write", { request });
export const cancelWrite = () => invoke<void>("cancel_write");
export const ejectDrive = (number: number) => invoke<void>("eject_drive", { number });

export const onDrivesChanged = (handler: (drives: Drive[]) => void): Promise<UnlistenFn> =>
  listen<Drive[]>("drives-changed", (e) => handler(e.payload));
export const onProgress = (handler: (p: Progress) => void): Promise<UnlistenFn> =>
  listen<Progress>("write-progress", (e) => handler(e.payload));
export const onFinished = (handler: (f: Finished) => void): Promise<UnlistenFn> =>
  listen<Finished>("write-finished", (e) => handler(e.payload));

export interface CatalogEntry {
  id: string;
  name: string;
  summary: string;
  color: string;
  windows: boolean;
}

export interface Release {
  entry: string;
  version: string;
  fileName: string;
  url: string;
  sha256: string;
  size: number | null;
  language: string | null;
  languages: string[];
}

export type DownloadStage = "checking" | "downloading";

export interface DownloadProgress {
  stage: DownloadStage;
  done: number;
  total: number;
}

export interface DownloadFinished {
  path: string | null;
  paused: boolean;
  cancelled: boolean;
  error: string | null;
}

export const catalogEntries = () => invoke<CatalogEntry[]>("catalog_entries");
export const defaultWindowsLanguage = () => invoke<string>("default_windows_language");
export const resolveRelease = (id: string, language: string | null) =>
  invoke<Release>("resolve_release", { id, language });
export const startDownload = (release: Release) => invoke<void>("start_download", { release });
export const pauseDownload = () => invoke<void>("pause_download");
export const cancelDownload = () => invoke<void>("cancel_download");
export const discardDownload = (release: Release) => invoke<void>("discard_download", { release });
export const onDownloadProgress = (handler: (p: DownloadProgress) => void): Promise<UnlistenFn> =>
  listen<DownloadProgress>("download-progress", (e) => handler(e.payload));
export const onDownloadFinished = (handler: (f: DownloadFinished) => void): Promise<UnlistenFn> =>
  listen<DownloadFinished>("download-finished", (e) => handler(e.payload));

/** Calls `onHover(true/false)` while files are dragged over the window and `onDrop` with the first path. */
export const onFileDrop = (onHover: (over: boolean) => void, onDrop: (path: string) => void): Promise<UnlistenFn> =>
  getCurrentWebview().onDragDropEvent((e) => {
    const p = e.payload;
    if (p.type === "enter" || p.type === "over") onHover(true);
    else if (p.type === "leave") onHover(false);
    else if (p.type === "drop") {
      onHover(false);
      if (p.paths.length > 0) onDrop(p.paths[0]);
    }
  });

export async function pickImage(): Promise<string | null> {
  const picked = await open({
    multiple: false,
    directory: false,
    filters: [
      { name: "Disk images", extensions: ["iso", "img", "raw", "bin"] },
      { name: "All files", extensions: ["*"] },
    ],
  });
  return typeof picked === "string" ? picked : null;
}
