# scribe

Write bootable disk images to USB drives. Windows today; Linux planned.

## Status (v0.1)

- `scribe list` — removable USB / SD drives (system disk and internal disks are never shown).
- `scribe write <image> -d <N>` — write an ISO or disk image, then read-back verification. Two modes:
  - **ISO mode** — the drive gets one FAT32 partition holding the ISO's files, like Rufus's "ISO Image mode". It boots on UEFI PCs (Secure Boot works if the ISO's loader is signed) and stays usable for other files.
  - **DD mode** — byte-for-byte copy of the image (hybrid ISOs, raw `.img` disk images).
- `scribe verify <image> -d <N>` — compare a drive written in DD mode with an image.

Not yet: Windows installer ISOs (UDF, `install.wim` over 4 GiB), legacy BIOS boot in ISO mode, OS catalog / downloader, GUI.

## Usage

Writing and verifying need an elevated (Administrator) terminal.

```
cargo build --release
target\release\scribe list
target\release\scribe write Fedora-KDE-Desktop-Live-44-1.7.x86_64.iso -d 3
```

`write` asks you to type the drive number before erasing. Flags: `--mode auto|iso|dd` (default `auto`), `--yes` skips the prompt, `--no-verify` skips read-back, `--force` writes images that do not look raw-bootable in DD mode.

`--mode auto` uses ISO mode for ISO 9660 images that contain a UEFI boot loader (`\EFI\BOOT\BOOT*.EFI`) and DD mode for everything else, or when ISO mode is impossible (e.g. a file of 4 GiB or more, which FAT32 cannot store).

### ISO mode

- Partition table: MBR, one active FAT32 (LBA) partition from 1 MiB to the end of the drive. Stale GPT headers at both ends of the drive are zeroed.
- File names come from Rock Ridge, else Joliet, else plain ISO 9660. Symbolic links are skipped. Files whose names differ only in case are kept once if identical; otherwise the write is refused.
- Live systems find their boot medium by filesystem label, and a FAT label is at most 11 upper-case characters. The FAT label is derived from the ISO label (`Fedora-KDE-Live-44` becomes `FEDORA-KDE-`), and `.cfg` / `.conf` files naming the ISO label are rewritten to the FAT label (`root=live:CDLABEL=Fedora-KDE-Live-44` becomes `root=live:LABEL=FEDORA-KDE-`). `write` lists the files it rewrote.
- No legacy BIOS boot loader is installed; the MBR boot code just hands over to the next boot device.

### DD mode

Hybrid ISOs with no GPT and no EFI system partition (e.g. Tiny Core) boot only via legacy BIOS; `write` warns about them before writing. On a UEFI PC such a drive boots only with legacy boot (CSM) enabled and Secure Boot off. Tiny Core has no UEFI boot loader, so `--mode auto` writes it in DD mode.

After a DD write of a hybrid ISO, Windows usually reports the drive as unformatted and offers to format it. That is expected: the drive holds an ISO 9660 filesystem Windows does not mount from USB. Choose **Cancel**; formatting erases the bootable image.

## Safety rules

- Only drives on a USB / SD bus or with removable media are targets.
- The disk holding the running Windows installation is always refused.
- The disk holding the image file is refused.
- The drive is re-read right before writing; a size or sector-size change aborts.

## Layout

| Crate | Role |
|---|---|
| `scribe-core` | `Drive`, `DiskBackend` / `BlockDevice` traits, image inspection, ISO 9660 reader, FAT32 layout, ISO mode, write + verify of a `Plan` (device runs) |
| `scribe-platform` | Native backends (`windows.rs`: `\\.\PhysicalDriveN`, volume lock / dismount, unbuffered I/O) |
| `scribe-cli` | `scribe` command-line tool |
