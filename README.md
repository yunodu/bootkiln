# bootkiln

Write bootable disk images to USB drives. Windows today; Linux planned.

## Status (v0.1)

- `bootkiln-gui.exe` — desktop app (portable, single exe) for the same workflow; see [GUI](#gui).
- `bootkiln list` — removable USB / SD drives (system disk and internal disks are never shown).
- `bootkiln write <image> -d <N>` — write an ISO or disk image, then read-back verification. Two modes:
  - **ISO mode** — the drive gets the ISO's files, like Rufus's "ISO Image mode": on FAT32, or on NTFS plus a UEFI:NTFS boot partition when a file is 4 GiB or larger (e.g. Windows 11's `install.wim`). It boots on UEFI PCs and stays usable for other files. Linux ISOs and Windows 10 / 11 installer ISOs (x64, ARM64) are supported.
  - **DD mode** — byte-for-byte copy of the image (hybrid ISOs, raw `.img` disk images).
- `bootkiln verify <image> -d <N>` — compare a drive written in DD mode with an image.

Not yet: legacy BIOS boot in ISO mode, OS catalog / downloader, translations.

## GUI

`bootkiln-gui.exe` is a [Tauri](https://tauri.app) app: the window is a Svelte + TypeScript UI in `crates/bootkiln-gui/ui`, the Rust side calls the same `bootkiln_core::job` workflow as the CLI. It asks for administrator rights at launch (UAC), follows the system's dark / light theme and uses the Windows 11 Mica backdrop.

- **Drive** — removable drives, refreshed automatically when a drive is plugged in or out.
- **Image** — file picker or drag and drop; shows how the image will be written and any warnings.
- **Windows setup** — shown for Windows installer ISOs; the same answer-file options as the CLI's `--win-*` flags.
- **Advanced** — write mode (Auto / ISO / DD) and read-back verification.
- **Start** checks the write without touching the drive, then asks for confirmation. Progress shows the phase, speed and time left; **Cancel** stops within seconds and leaves the drive incomplete. After success, **Eject** safely removes the drive.

Build (needs Node.js for the UI; the exe needs only WebView2, which ships with Windows 10 / 11):

```
cd crates\bootkiln-gui
npm install
npx tauri build --no-bundle
```

The exe is `target\release\bootkiln-gui.exe`. All UI text lives in `ui/src/lib/texts.ts`.

## Usage

Writing and verifying need an elevated (Administrator) terminal.

```
cargo build --release
target\release\bootkiln list
target\release\bootkiln write Fedora-KDE-Desktop-Live-44-1.7.x86_64.iso -d 3
target\release\bootkiln write Win11_x64_en-us.iso -d 3 --win-skip-hw-checks --win-local-account pat
```

`write` asks you to type the drive number before erasing. Flags: `--mode auto|iso|dd` (default `auto`), `--yes` skips the prompt, `--no-verify` skips read-back, `--force` writes images that do not look raw-bootable in DD mode.

`--mode auto` uses ISO mode for ISO images (ISO 9660 or UDF) that contain a UEFI boot loader (`\EFI\BOOT\BOOT*.EFI`) and DD mode for everything else, or when ISO mode is impossible.

### ISO mode

- Files come from UDF when the image has it (Windows installers), else ISO 9660 with Rock Ridge, Joliet or plain names. Symbolic links are skipped. Files whose names differ only in case are kept once if identical; otherwise the write is refused.
- **FAT32** (all files below 4 GiB): MBR, one active FAT32 (LBA) partition from 1 MiB to the end of the drive, built and verified sector by sector. Stale GPT headers at both ends of the drive are zeroed.
- **NTFS** (a file of 4 GiB or more): MBR with an active NTFS partition from 1 MiB and a 1 MiB EFI system partition holding [UEFI:NTFS](https://github.com/pbatard/uefi-ntfs) at the end of the drive. UEFI firmware boots UEFI:NTFS, which loads an NTFS driver and starts `\EFI\BOOT\BOOT*.EFI` from the NTFS partition. Windows formats the NTFS partition; the files are copied through the file system, then the volume is dismounted and every file is read back and compared with the ISO. UEFI:NTFS is signed with Microsoft's third-party UEFI certificate, so Secure Boot stays on; PCs that trust only Windows' own certificate (some Secured-core PCs) need "Allow Microsoft 3rd party UEFI CA" enabled in the firmware. Only 512-byte-sector drives are supported, as the UEFI:NTFS image is built for them.
- Live systems find their boot medium by filesystem label. The drive's label is derived from the ISO label: on FAT32 at most 11 upper-case characters (`Fedora-KDE-Live-44` becomes `FEDORA-KDE-`), on NTFS the ISO label up to 32 characters. `.cfg` / `.conf` files naming the ISO label are rewritten to the drive label (`root=live:CDLABEL=Fedora-KDE-Live-44` becomes `root=live:LABEL=FEDORA-KDE-`). `write` lists the files it rewrote.
- No legacy BIOS boot loader is installed; the MBR boot code just hands over to the next boot device.

### Windows installer options

These add an `autounattend.xml` answer file to the drive root; each is off unless given, and they are refused for non-Windows images.

| Flag | Effect |
|---|---|
| `--win-skip-hw-checks` | Skip the TPM 2.0, Secure Boot and 4 GB RAM checks (Windows 11 only; refused for Windows 10) |
| `--win-skip-online-account` | Allow setup without a Microsoft account |
| `--win-local-account <NAME>` | Create a local administrator with an empty password, which must be changed at first sign-in |
| `--win-copy-region` | Use this PC's region, keyboard layout and time zone |
| `--win-skip-privacy` | Answer the privacy questions with the most private choices and skip them |
| `--win-no-bitlocker` | Turn off BitLocker's automatic device encryption |

### DD mode

Hybrid ISOs with no GPT and no EFI system partition (e.g. Tiny Core) boot only via legacy BIOS; `write` warns about them before writing. On a UEFI PC such a drive boots only with legacy boot (CSM) enabled and Secure Boot off. Tiny Core has no UEFI boot loader, so `--mode auto` writes it in DD mode.

After a DD write of a hybrid ISO, Windows usually reports the drive as unformatted and offers to format it. That is expected: the drive holds an ISO 9660 filesystem Windows does not mount from USB. Choose **Cancel**; formatting erases the bootable image.

## Safety rules

- Only drives on a USB / SD bus or with removable media are targets.
- The disk holding the running Windows installation is always refused.
- The disk holding the image file is refused.
- The drive is re-read right before writing; a size or sector-size change aborts.
- NTFS mode formats only the partition at the offset bootkiln just wrote to the target disk's partition table.

## Layout

| Crate | Role |
|---|---|
| `bootkiln-core` | `Drive`, `DiskBackend` / `BlockDevice` traits, image inspection, ISO 9660 and UDF readers, FAT32 and UEFI:NTFS layouts, ISO mode, Windows detection and answer files, write + verify of a `Plan` (device runs), file-tree copy + verify |
| `bootkiln-platform` | Native backends (`windows.rs`: `\\.\PhysicalDriveN`, volume lock / dismount, unbuffered I/O, NTFS formatting; `region.rs`: regional settings) |
| `bootkiln-cli` | `bootkiln` command-line tool |
| `bootkiln-gui` | `bootkiln-gui` desktop app (Tauri; UI in `ui/`) |

## Third-party components

`crates/bootkiln-core/assets/uefi-ntfs.img` is UEFI:NTFS from Rufus, licensed under the GPL (v2 or later, and v3 or later for some drivers). It is embedded in the `bootkiln` binary and written unmodified to NTFS drives. Provenance, licenses and source links: [`crates/bootkiln-core/assets/README.md`](crates/bootkiln-core/assets/README.md).
