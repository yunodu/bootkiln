# Changelog

All notable changes are listed here. Versions follow [Semantic Versioning](https://semver.org); the release workflow publishes the section of the tagged version as the release notes.

## [0.1.0] - unreleased

First release. Windows 10 / 11 and Linux, x64.

### Added

- `bootkiln-gui.exe`: desktop app (drive and image selection, OS downloads, Windows setup options, progress with cancel, eject) and an update notice when a newer release exists.
- `bootkiln.exe`: command-line tool with `list`, `write`, `verify`, `catalog` and `download`.
- Linux: `bootkiln-<version>-x86_64.AppImage` (the desktop app; writes run in a helper started through polkit / `pkexec`) and the `bootkiln-linux-x86_64` command-line tool (run with `sudo`). NTFS drives need `mkntfs` from ntfs-3g; ejecting uses udisks. Copying the region into Windows answer files is Windows-only.
- ISO mode: copies an ISO's files onto a fresh drive (FAT32, or NTFS + UEFI:NTFS when a file is 4 GiB or larger) for UEFI boot; reads ISO 9660 (Rock Ridge, Joliet) and UDF; rewrites boot configuration labels; copies files that symbolic links point to.
- DD mode: byte-for-byte writes with a warning for images that boot only via legacy BIOS.
- Windows 10 / 11 installer ISOs with optional `autounattend.xml` answers (skip hardware checks, no Microsoft account, local account, region, privacy, BitLocker).
- OS catalog: Ubuntu, Kubuntu, Fedora Workstation / KDE, Linux Mint, Debian netinst / Live KDE, Arch, Windows 11; downloads are resumable and verified against the published SHA-256.
- Read-back verification of every write.
