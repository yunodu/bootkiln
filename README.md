# bootkiln

Write bootable disk images to USB drives on Windows 10 / 11 and Linux (x64).

## Download

Download the latest release from [**Releases**](https://github.com/yunodu/bootkiln/releases) (`SHA256SUMS.txt` has checksums). Everything is portable — no installer or setup required.

| System | Desktop app | Command line |
|---|---|---|
| Windows 10 / 11 (x64) | `bootkiln-gui.exe` | `bootkiln.exe` |
| Linux (x64) | `bootkiln-<version>-x86_64.AppImage` | `bootkiln-linux-x86_64` |

- **Windows:** the executables are not code-signed yet, so SmartScreen may show *"Windows protected your PC"*. Click **More info** → **Run anyway**. The app then asks for administrator rights (UAC), which raw disk access requires.
- **Linux:** make the file executable (`chmod +x`). The desktop app runs as your normal user and asks for your password via Polkit (`pkexec`) when a write starts; the CLI runs with `sudo`. Writing Windows ISOs (NTFS) requires `mkntfs` (`ntfs-3g` package); **Eject** uses `udisks2`. The AppImage requires `libfuse2` (or `APPIMAGE_EXTRACT_AND_RUN=1`).

The desktop app checks GitHub Releases on startup and shows a link if a newer version is available; it never downloads or installs updates automatically.

## Features

- **ISO mode & DD mode** — automatically picks the right method for each image:
  - **ISO mode** copies the ISO's files onto a clean FAT32 volume, or onto an NTFS volume paired with a [UEFI:NTFS](https://github.com/pbatard/uefi-ntfs) boot partition when any file is 4 GiB or larger (such as Windows 11's `install.wim`). Boots on UEFI PCs and leaves the drive usable for normal storage.
  - **DD mode** writes hybrid ISOs and raw `.img` disk images byte-for-byte.
- **Windows 10 / 11 setup options** — generate an `autounattend.xml` answer file to:
  - Skip Windows 11 TPM 2.0, Secure Boot, and 4 GB RAM checks
  - Allow setup without a Microsoft account
  - Pre-create a local administrator account
  - Copy this PC's regional, language, keyboard, and time-zone settings (Windows host only)
  - Skip privacy / telemetry questions with the most private answers
  - Disable automatic BitLocker device encryption
- **Built-in OS downloader** — download official ISOs directly from vendor servers with pause/resume and automatic SHA-256 verification.
- **Read-back verification** — reads back written sectors and files to catch faulty USB drives before you boot from them.
- **Safe by design** — system disks, internal drives, and the drive holding the source image are never offered or overwritten.

## Desktop App

1. **Drive** — plug in a USB stick or SD card; the list updates automatically.
2. **Image** — drop an `.iso` or `.img` file onto the window, click **Browse…**, or click **Download an OS** to fetch a current release from the built-in catalog.
3. **Windows setup** *(shown automatically for Windows 10 / 11 ISOs)* — toggle any setup bypasses or unattended options you want.
4. **Advanced** *(optional)* — override the write mode (Auto / ISO / DD) or toggle read-back verification.
5. **Write** — validates the image and target drive, asks for confirmation, and shows phase progress, speed, and remaining time. When finished, click **Eject** to safely power down the drive.

## Built-in OS Catalog

Every entry queries the vendor's official release index live and verifies the downloaded ISO against its published SHA-256 checksum. Partial downloads (`<file>.part`) resume where they left off.

| ID | Operating System | Official Source |
|---|---|---|
| `ubuntu` | Ubuntu Desktop (latest LTS) | `releases.ubuntu.com` |
| `kubuntu` | Kubuntu (latest LTS) | `cdimage.ubuntu.com` |
| `fedora-workstation` | Fedora Workstation | `fedoraproject.org` |
| `fedora-kde` | Fedora KDE Plasma Desktop | `fedoraproject.org` |
| `linuxmint` | Linux Mint Cinnamon | `mirrors.edge.kernel.org/linuxmint` |
| `debian` | Debian netinst | `cdimage.debian.org` |
| `debian-live-kde` | Debian Live KDE | `cdimage.debian.org` |
| `arch` | Arch Linux | `geo.mirror.pkgbuild.com` |
| `windows11` | Windows 11 x64 (Home / Pro / Education) | Microsoft download servers |

Downloaded ISOs are saved in `downloads\` next to the executable on Windows and `~/Downloads/bootkiln` on Linux.

## Command-Line Usage

List removable drives and available OS downloads:

```sh
bootkiln list
bootkiln catalog
```

Download an OS image:

```sh
bootkiln download ubuntu
bootkiln download windows11 --lang German
```

Write an image to a drive (run from an Administrator terminal on Windows, or with `sudo` on Linux):

```sh
bootkiln write Fedora-KDE-Desktop-Live-44-1.7.x86_64.iso -d 3
bootkiln write Win11_x64_en-us.iso -d 3 --win-skip-hw-checks --win-skip-online-account --win-local-account pat
```

Compare a DD-written drive against an image:

```sh
bootkiln verify TinyCore-current.iso -d 3
```

### `bootkiln write` options

| Flag | Description |
|---|---|
| `-d, --drive <N>` | Target drive number from `bootkiln list` |
| `--mode <auto\|iso\|dd>` | Write mode (default: `auto`) |
| `--yes` | Skip the interactive confirmation prompt |
| `--no-verify` | Skip read-back verification |
| `--force` | Allow DD writes of images that do not look raw-bootable |
| `--win-skip-hw-checks` | Skip Windows 11 TPM 2.0, Secure Boot, and RAM checks |
| `--win-skip-online-account` | Allow Windows setup without a Microsoft account |
| `--win-local-account <NAME>` | Create a local administrator account (empty password; changed at first sign-in) |
| `--win-copy-region` | Use this PC's region, keyboard layout, and time zone (Windows only) |
| `--win-skip-privacy` | Answer Windows privacy questions with the most private choices |
| `--win-no-bitlocker` | Disable BitLocker automatic device encryption |

## Notes & Limitations

- **UEFI boot in ISO mode:** ISO mode prepares drives for UEFI systems (no legacy BIOS bootloader is installed). For NTFS drives (such as Windows 11), UEFI:NTFS is signed with Microsoft's third-party UEFI CA, so Secure Boot stays enabled.
- **Legacy BIOS images in DD mode:** hybrid ISOs without an EFI System Partition (such as Tiny Core) boot only via legacy BIOS / CSM. `bootkiln` warns before writing them.
- **Windows "Format disk" prompt after DD writes:** Windows often cannot mount Linux ISO 9660 partitions on USB sticks and prompts to format the drive. Click **Cancel** — formatting erases the bootable image.

## Building from Source

Requires [Rust](https://rustup.rs) (1.85+) and [Node.js](https://nodejs.org) (20+ for the GUI frontend).

**Windows:**

```powershell
cargo build --release -p bootkiln-cli
cd crates\bootkiln-gui
npm ci
npx tauri build --no-bundle
```

**Linux (Debian / Ubuntu dependencies shown):**

```sh
sudo apt install build-essential pkg-config libwebkit2gtk-4.1-dev libxdo-dev librsvg2-dev libayatana-appindicator3-dev
cargo build --release -p bootkiln-cli
cd crates/bootkiln-gui
npm ci
npx tauri build
```

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

`crates/bootkiln-core/assets/uefi-ntfs.img` is [UEFI:NTFS](https://github.com/pbatard/uefi-ntfs) from Rufus, licensed under the GPL (v2+ / v3+) and embedded unmodified as a separate work for NTFS boot partitions. See [`crates/bootkiln-core/assets/README.md`](crates/bootkiln-core/assets/README.md) for provenance and source links.
