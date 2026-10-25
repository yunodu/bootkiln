// Every user-visible string, so translations can be added in one place.
import type { ImageInfo, Note, Phase } from "./api";

export const t = {
  appTagline: "Bootable USB drives, made simple",

  driveTitle: "Drive",
  driveEmpty: "Plug in a USB drive or SD card",
  driveEmptyHint: "It appears here automatically.",
  driveSystemNote: "Only removable drives are shown.",

  imageTitle: "Image",
  imageDrop: "Drop an ISO or disk image here",
  imageBrowse: "Browse…",
  imageChange: "Change",
  imageAnalyzing: "Reading the image…",
  imageDownload: "Download an OS",
  imageDownloadShort: "Download",
  imageOr: "or",

  catalogTitle: "Download an OS",
  catalogSubtitle: "Latest releases from the official servers, checked against their published SHA-256",
  catalogLoading: "Checking…",
  catalogUnavailable: "Unavailable right now",
  catalogLanguage: "Language",
  catalogDownload: (name: string, version: string, size: string) => `Download ${name} ${version}${size ? ` · ${size}` : ""}`,
  catalogPick: "Choose an operating system",
  catalogClose: "Close",

  downloadChecking: "Checking the existing download…",
  downloadOf: (done: string, total: string) => `${done} of ${total}`,
  downloadPaused: "Paused",
  downloadPause: "Pause",
  downloadResume: "Resume",
  downloadCancel: "Cancel",
  downloadFailed: "Download failed",
  downloadRetry: "Try again",

  windowsTitle: "Windows setup",
  windowsSubtitle: "Optional answers for Windows Setup, written to autounattend.xml",
  winSkipHw: "Skip TPM, Secure Boot and RAM checks",
  winSkipHwHint: "Install Windows 11 on PCs it calls unsupported",
  winSkipHwWin10: "Windows 10 has no such checks",
  winSkipOnline: "No Microsoft account needed",
  winSkipOnlineHint: "Allow setup without signing in online",
  winLocal: "Create a local account",
  winLocalHint: "Empty password, to be set at first sign-in",
  winLocalPlaceholder: "Account name",
  winRegion: "Use this PC's region and keyboard",
  winRegionHint: "Language format, keyboard layout and time zone",
  winPrivacy: "Skip the privacy questions",
  winPrivacyHint: "Answers them with the most private choices",
  winBitlocker: "Turn off automatic BitLocker encryption",
  winBitlockerHint: "Windows would otherwise encrypt the disk silently",

  advancedTitle: "Advanced",
  modeLabel: "Write mode",
  modes: { auto: "Auto", iso: "ISO", dd: "DD" },
  modeHints: {
    auto: "Copy files when the image supports it, otherwise a raw copy",
    iso: "Copy the image's files onto a fresh drive (UEFI boot)",
    dd: "Byte-for-byte copy of the image",
  },
  verifyLabel: "Verify after writing",
  verifyHint: "Read everything back and compare",

  start: "Start",
  checking: "Checking…",
  pickDrive: "Choose a drive",
  pickImage: "Choose an image",

  confirmTitle: "Erase this drive?",
  confirmBody: (drive: string) => `Everything on ${drive} will be deleted. This cannot be undone.`,
  confirmCancel: "Cancel",
  confirmGo: "Erase and write",

  phases: {
    writing: "Writing",
    verifying: "Verifying",
    formatting: "Formatting",
    copying: "Copying files",
    verifyingFiles: "Verifying files",
  } satisfies Record<Phase, string>,
  progressOf: (done: string, total: string) => `${done} of ${total}`,
  /** `seconds` is already rounded (see `roundEta`). */
  timeLeft(seconds: number): string {
    if (seconds < 60) return `about ${seconds} s left`;
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    const s = seconds % 60;
    if (h > 0) return `${h} h ${String(m).padStart(2, "0")} min left`;
    return s === 0 ? `${m} min left` : `${m} min ${String(s).padStart(2, "0")} s left`;
  },
  formatting: "Preparing the NTFS volume…",
  cancel: "Cancel",
  cancelling: "Cancelling…",

  doneTitle: "Ready",
  doneBody: (image: string, drive: string) => `${image} is on ${drive}.`,
  notes: {
    formatPrompt:
      "Windows may say the drive needs to be formatted. That is expected for bootable images; choose Cancel, or the drive will be erased.",
    uefiOnly: "Boots on UEFI PCs (pick the drive in the boot menu). Legacy BIOS boot is not installed.",
    uefiNtfs:
      "Boots on UEFI PCs through UEFI:NTFS. With Secure Boot on, a few PCs need “Allow Microsoft 3rd party UEFI CA” enabled in their firmware.",
  } satisfies Record<Note, string>,
  eject: "Eject",
  ejecting: "Ejecting…",
  ejected: "Safe to remove",
  doneButton: "Done",

  cancelledTitle: "Cancelled",
  cancelledBody: "The drive is incomplete and will not boot. Write it again, or format it in Explorer to use it normally.",
  failedTitle: "Something went wrong",
  back: "Back",

  // Image summary lines.
  summary(info: ImageInfo): { title: string; detail: string } {
    if (info.method === "dd") {
      return { title: "Raw copy (DD)", detail: "Byte-for-byte copy" };
    }
    const fs = info.fileSystem === "ntfs" ? "NTFS + UEFI:NTFS" : "FAT32";
    if (info.windows) {
      const name = info.windows.isWindows11 === false ? "Windows 10" : info.windows.isWindows11 ? "Windows 11" : "Windows";
      const build = info.windows.build ? ` · build ${info.windows.build}` : "";
      return { title: `${name} ${info.windows.arch}${build}`, detail: `Files copied to ${fs} · UEFI boot` };
    }
    return { title: info.isoLabel ?? "Bootable ISO", detail: `Files copied to ${fs} · UEFI boot` };
  },
  warnBiosOnly:
    "This image boots only on PCs with legacy BIOS boot (CSM) enabled and Secure Boot off. Many recent PCs cannot boot it.",
  warnNtfs: (files: string[]) => `NTFS is used because FAT32 cannot hold ${files.join(", ")}.`,
  infoPatched: (from: string, to: string) => `Boot settings are adjusted from label ${from} to ${to}.`,
  infoIsoUnavailable: (why: string) => `File copy is not possible (${why}), so the image is written raw.`,
};
