# Bundled third-party binaries

## `uefi-ntfs.img`

FAT partition image of [UEFI:NTFS](https://github.com/pbatard/uefi-ntfs) by Pete Batard, taken unmodified from the Rufus repository: [`res/uefi/uefi-ntfs.img`](https://github.com/pbatard/rufus/blob/2350489fe902bdc71d37f255a7684fe06a0ab2d0/res/uefi/uefi-ntfs.img) at commit `2350489fe902bdc71d37f255a7684fe06a0ab2d0` (2026-06-19).

SHA-256: `72683fa1250eeea772d3399277b434d4e55ba8dd0dc926e52d817e701fc2eb9e`

bootkiln writes this image, as is, to the small EFI system partition it places after an NTFS partition. UEFI firmware boots the UEFI:NTFS loader from it, which loads an NTFS driver and starts `\EFI\BOOT\BOOT<arch>.EFI` from the NTFS partition.

Contents, per the image's [readme](https://github.com/pbatard/rufus/blob/master/res/uefi/readme.txt):

- UEFI:NTFS boot loaders (release 2.8), Secure Boot signed by Microsoft (except 32-bit ARM). License: GPL-2.0-or-later. Source: <https://github.com/pbatard/uefi-ntfs>
- NTFS UEFI drivers derived from ntfs-3g (release 1.9), Secure Boot signed by Microsoft. License: GPL-2.0-or-later. Source: <https://github.com/pbatard/ntfs-3g>
- exFAT and ARM/RISC-V NTFS UEFI drivers from EfiFs (release 1.12), not Secure Boot signed. License: GPL-3.0-or-later. Source: <https://github.com/pbatard/efifs>

These programs are distributed alongside bootkiln as a separate work; their licenses, not bootkiln's, apply to them. The corresponding source code is available at the links above.
