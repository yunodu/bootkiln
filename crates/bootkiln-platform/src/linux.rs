//! Linux backend: whole-disk block devices (`/dev/sdb`, `/dev/mmcblk0`) found through
//! sysfs, raw `O_DIRECT` I/O, `mkntfs` + `mount` for NTFS, udisks for ejecting.
//!
//! Drive numbers are the kernel's `diskseq`: unique per boot and never reused, so a number
//! from an old listing can never select a different drive plugged in later.

use std::collections::HashSet;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileExt, FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use bootkiln_core::unattend::Region;
use bootkiln_core::{BlockDevice, BusKind, DiskBackend, Drive};

const SYS_BLOCK: &str = "/sys/block";
/// Where `format_ntfs` mounts the new volume while files are copied.
const MOUNT_BASE: &str = "/run/bootkiln";
/// Disks holding these mount points, or swap, host the running system.
const SYSTEM_MOUNTS: &[&str] = &["/", "/boot", "/boot/efi", "/efi", "/usr", "/var", "/home"];
/// How long `format_ntfs` waits for the kernel to list a just-written partition.
const PARTITION_POLLS: u32 = 40;
const POLL_DELAY: Duration = Duration::from_millis(250);
/// Unmount-and-open attempts; desktop automounters may remount between the two.
const OPEN_ATTEMPTS: u32 = 5;
/// Slave / holder chains (LVM on LUKS on a partition, ...) followed this deep.
const MAX_STACK_DEPTH: usize = 8;

// <linux/fs.h>: _IO(0x12, 95), _IO(0x12, 97).
const BLKRRPART: libc::Ioctl = 0x125f;
const BLKFLSBUF: libc::Ioctl = 0x1261;

#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxBackend;

impl DiskBackend for LinuxBackend {
    fn is_elevated(&self) -> bool {
        // SAFETY: no preconditions.
        unsafe { libc::geteuid() == 0 }
    }

    fn list_drives(&self) -> io::Result<Vec<Drive>> {
        let mounts = mounts()?;
        let mut system: HashSet<String> =
            mounts.iter().filter(|m| SYSTEM_MOUNTS.iter().any(|s| m.point == Path::new(s))).flat_map(Mount::disks).collect();
        system.extend(swap_disks());

        let mut drives = Vec::new();
        for entry in fs::read_dir(SYS_BLOCK)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            let dir = Path::new(SYS_BLOCK).join(&name);
            // Loop, RAM, device-mapper and other virtual devices have no `device`; `sr` are optical drives.
            if !dir.join("device").exists() || name.starts_with("sr") {
                continue;
            }
            let Ok(sectors) = read_u64(&dir.join("size")) else { continue };
            if sectors == 0 {
                continue; // Card reader without a card.
            }
            let mount_points = mounts.iter().filter(|m| m.disks().contains(&name)).map(|m| m.point.display().to_string()).collect();
            drives.push(Drive {
                number: disk_number(&dir)?,
                path: format!("/dev/{name}"),
                model: model(&dir).unwrap_or_else(|| name.clone()),
                size_bytes: sectors * 512,
                sector_size: read_u64(&dir.join("queue/logical_block_size"))? as u32,
                bus: bus(&dir, &name),
                removable_media: read_trimmed(&dir.join("removable")).is_ok_and(|r| r == "1"),
                mount_points,
                is_system: system.contains(&name),
            });
        }
        drives.sort_by_key(|d| d.number);
        Ok(drives)
    }

    fn disk_numbers_for_path(&self, path: &Path) -> io::Result<Vec<u32>> {
        let path = fs::canonicalize(path)?;
        let mounts = mounts()?;
        let Some(mount) = mounts.iter().filter(|m| path.starts_with(&m.point)).max_by_key(|m| m.point.as_os_str().len()) else {
            return Ok(Vec::new());
        };
        mount.disks().iter().map(|name| disk_number(&Path::new(SYS_BLOCK).join(name))).collect()
    }

    fn open_exclusive(&self, drive: &Drive) -> io::Result<Box<dyn BlockDevice>> {
        let name = disk_name(drive)?;
        let dir = Path::new(SYS_BLOCK).join(name);
        if disk_number(&dir).ok() != Some(drive.number)
            || read_u64(&dir.join("size"))? * 512 != drive.size_bytes
            || read_u64(&dir.join("queue/logical_block_size"))? != u64::from(drive.sector_size)
        {
            return Err(io::Error::other(format!("{} changed since it was listed", drive.path)));
        }
        let mut attempt = 0;
        loop {
            unmount_disk(name)?;
            // O_EXCL on a block device fails while anything is mounted from it or holds it.
            match OpenOptions::new().read(true).write(true).custom_flags(libc::O_DIRECT | libc::O_EXCL).open(&drive.path) {
                Ok(file) => return Ok(Box::new(LinuxDisk { file, size: drive.size_bytes, sector: drive.sector_size })),
                Err(e) if e.raw_os_error() == Some(libc::EBUSY) && attempt + 1 < OPEN_ATTEMPTS => {
                    attempt += 1;
                    thread::sleep(POLL_DELAY);
                }
                Err(e) if e.raw_os_error() == Some(libc::EBUSY) => {
                    return Err(io::Error::new(
                        e.kind(),
                        format!("{} is in use by another program; close it and try again", drive.path),
                    ));
                }
                Err(e) => return Err(io::Error::new(e.kind(), format!("opening {}: {e}", drive.path))),
            }
        }
    }

    fn format_ntfs(&self, drive: &Drive, partition_offset: u64, label: &str) -> io::Result<PathBuf> {
        let name = disk_name(drive)?;
        let partition = wait_for_partition(name, partition_offset)?;
        let start_sector = (partition_offset / u64::from(drive.sector_size)).to_string();
        let sector_size = drive.sector_size.to_string();
        run(
            Command::new("mkntfs").args([
                "--quick",
                "--label",
                label,
                "--partition-start",
                &start_sector,
                "--sector-size",
                &sector_size,
                "--heads",
                "255",
                "--sectors-per-track",
                "63",
            ]),
            &partition,
            "formatting NTFS (mkntfs, from the ntfs-3g package)",
        )?;
        let root = Path::new(MOUNT_BASE).join(drive.number.to_string());
        fs::create_dir_all(&root)?;
        mount_ntfs(&partition, &root)?;
        Ok(root)
    }

    fn dismount_volume(&self, root: &Path) -> io::Result<()> {
        let device = mounted_device(root)?;
        unmount(root)?;
        // Drop cached blocks so the remount reads what is on the medium.
        let file = File::open(&device)?;
        // SAFETY: valid open descriptor; BLKFLSBUF takes no argument.
        if unsafe { libc::ioctl(file.as_raw_fd(), BLKFLSBUF, 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        mount_ntfs(&device, root)
    }

    fn release_volume(&self, root: &Path) -> io::Result<()> {
        unmount(root)?;
        fs::remove_dir(root)
    }

    fn eject(&self, drive: &Drive) -> io::Result<()> {
        let name = disk_name(drive)?;
        // SAFETY: no preconditions.
        unsafe { libc::sync() };
        let udisks = |args: &[&str]| Command::new("udisksctl").args(args).arg("--no-user-interaction").output();
        // udisks powers the USB device down and lets unprivileged desktop users eject, but
        // refuses while volumes are mounted.
        let mut udisks_error = "udisks (udisksctl) is not installed".to_owned();
        if let Ok(mounts) = mounts() {
            for m in mounts.iter().filter(|m| m.disks().iter().any(|d| d == name)) {
                let _ = udisks(&["unmount", "-b", &m.source]);
            }
        }
        if let Ok(out) = udisks(&["power-off", "-b", &drive.path]) {
            if out.status.success() {
                return Ok(());
            }
            udisks_error = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        }
        if !self.is_elevated() {
            return Err(io::Error::other(format!("ejecting {} failed: {udisks_error}", drive.path)));
        }
        // As root without working udisks: unmount, then detach the device from the kernel.
        unmount_disk(name)?;
        fs::write(Path::new(SYS_BLOCK).join(name).join("device/delete"), "1")
            .map_err(|e| io::Error::new(e.kind(), format!("ejecting {}: {e}", drive.path)))
    }
}

/// Raw disk opened with `O_DIRECT | O_EXCL`.
struct LinuxDisk {
    file: File,
    size: u64,
    sector: u32,
}

impl BlockDevice for LinuxDisk {
    fn sector_size(&self) -> u32 {
        self.sector
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()> {
        self.file.write_all_at(buf, offset)
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.file.read_exact_at(buf, offset)
    }

    fn sync(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }
}

impl Drop for LinuxDisk {
    fn drop(&mut self) {
        let _ = self.file.sync_all();
        // Make the kernel (and desktop) see the new partition table.
        // SAFETY: valid open descriptor; BLKRRPART takes no argument.
        unsafe { libc::ioctl(self.file.as_raw_fd(), BLKRRPART, 0) };
    }
}

/// One line of `/proc/self/mountinfo`.
struct Mount {
    /// `major:minor` as reported (anonymous for btrfs subvolumes).
    device: Option<(u32, u32)>,
    point: PathBuf,
    /// Mount source, e.g. `/dev/sdb1`.
    source: String,
}

impl Mount {
    /// Whole-disk names (`sdb`) this mount's data lives on.
    fn disks(&self) -> Vec<String> {
        // Prefer the source device: btrfs reports anonymous device numbers.
        if let Some(disks) = block_device_disks(Path::new(&self.source)) {
            return disks;
        }
        match self.device {
            Some((major, minor)) if major != 0 => sysfs_disks(&format!("/sys/dev/block/{major}:{minor}")),
            _ => Vec::new(),
        }
    }
}

fn mounts() -> io::Result<Vec<Mount>> {
    Ok(parse_mountinfo(&fs::read_to_string("/proc/self/mountinfo")?))
}

fn parse_mountinfo(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            let separator = fields.iter().position(|f| *f == "-")?;
            let (major, minor) = fields.get(2)?.split_once(':')?;
            Some(Mount {
                device: major.parse().ok().zip(minor.parse().ok()),
                point: PathBuf::from(unescape(fields.get(4)?)),
                source: unescape(fields.get(separator + 2)?),
            })
        })
        .collect()
}

/// mountinfo escapes space, tab, newline and backslash as `\ooo`.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal = bytes.get(i + 1..i + 4).and_then(|d| std::str::from_utf8(d).ok()).and_then(|d| u8::from_str_radix(d, 8).ok());
        match (bytes[i], octal) {
            (b'\\', Some(byte)) => {
                out.push(byte);
                i += 4;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn swap_disks() -> Vec<String> {
    let swaps = fs::read_to_string("/proc/swaps").unwrap_or_default();
    swaps.lines().skip(1).filter_map(|l| l.split_whitespace().next()).filter_map(|d| block_device_disks(Path::new(d))).flatten().collect()
}

/// Disks under the block device node at `path`; `None` if it is not one.
fn block_device_disks(path: &Path) -> Option<Vec<String>> {
    let meta = fs::metadata(path).ok().filter(|m| m.file_type().is_block_device())?;
    let rdev = meta.rdev();
    Some(sysfs_disks(&format!("/sys/dev/block/{}:{}", libc::major(rdev), libc::minor(rdev))))
}

/// Whole disks under a sysfs block device: a partition's parent, the disks below a
/// device-mapper / RAID device, or the disk itself.
fn sysfs_disks(path: &str) -> Vec<String> {
    fs::canonicalize(path).map(|p| disks_below(&p, 0)).unwrap_or_default()
}

fn disks_below(dev: &Path, depth: usize) -> Vec<String> {
    if depth > MAX_STACK_DEPTH {
        return Vec::new();
    }
    if dev.join("partition").exists() {
        return dev.parent().and_then(Path::file_name).map(|n| vec![n.to_string_lossy().into_owned()]).unwrap_or_default();
    }
    let slaves: Vec<String> = fs::read_dir(dev.join("slaves"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|s| fs::canonicalize(s.path()).ok())
        .flat_map(|s| disks_below(&s, depth + 1))
        .collect();
    if !slaves.is_empty() {
        return slaves;
    }
    dev.file_name().map(|n| vec![n.to_string_lossy().into_owned()]).unwrap_or_default()
}

/// `diskseq`, or the device number on kernels before 5.15.
fn disk_number(dir: &Path) -> io::Result<u32> {
    if let Ok(seq) = read_u64(&dir.join("diskseq")) {
        return u32::try_from(seq).map_err(|_| io::Error::other("disk sequence number out of range"));
    }
    let dev = read_trimmed(&dir.join("dev"))?;
    let (major, minor) = dev.split_once(':').ok_or_else(|| io::Error::other(format!("bad device number {dev}")))?;
    let parse = |s: &str| s.parse::<u32>().map_err(|e| io::Error::other(format!("bad device number {dev}: {e}")));
    Ok((parse(major)? << 20) | parse(minor)?)
}

fn bus(dir: &Path, name: &str) -> BusKind {
    let path = fs::canonicalize(dir).unwrap_or_default();
    if path.components().any(|c| c.as_os_str().as_bytes().starts_with(b"usb")) {
        BusKind::Usb
    } else if name.starts_with("mmcblk") {
        BusKind::Sd
    } else {
        BusKind::Other
    }
}

fn model(dir: &Path) -> Option<String> {
    let field = |f: &str| read_trimmed(&dir.join("device").join(f)).ok().filter(|s| !s.is_empty());
    let joined = match (field("vendor"), field("model")) {
        (Some(vendor), Some(model)) => format!("{vendor} {model}"),
        (vendor, model) => model.or(vendor).or_else(|| field("name"))?,
    };
    Some(joined)
}

fn disk_name(drive: &Drive) -> io::Result<&str> {
    drive
        .path
        .strip_prefix("/dev/")
        .filter(|n| !n.is_empty() && !n.contains('/'))
        .ok_or_else(|| io::Error::other(format!("{} is not a /dev disk", drive.path)))
}

/// Unmount every volume on disk `name`, innermost mount points first.
fn unmount_disk(name: &str) -> io::Result<()> {
    let mut points: Vec<PathBuf> =
        mounts()?.into_iter().filter(|m| m.disks().iter().any(|d| d == name)).map(|m| m.point).collect();
    points.sort_by_key(|p| std::cmp::Reverse(p.as_os_str().len()));
    for point in points {
        unmount(&point)?;
    }
    Ok(())
}

fn unmount(point: &Path) -> io::Result<()> {
    let c_point = CString::new(point.as_os_str().as_bytes()).map_err(io::Error::other)?;
    // SAFETY: NUL-terminated path.
    if unsafe { libc::umount2(c_point.as_ptr(), 0) } != 0 {
        let e = io::Error::last_os_error();
        let hint = if e.raw_os_error() == Some(libc::EBUSY) { "; close programs and file managers using it" } else { "" };
        return Err(io::Error::new(e.kind(), format!("cannot unmount {} ({e}){hint}", point.display())));
    }
    Ok(())
}

/// The block device mounted at `point`.
fn mounted_device(point: &Path) -> io::Result<PathBuf> {
    mounts()?
        .into_iter()
        .rfind(|m| m.point == point)
        .map(|m| PathBuf::from(m.source))
        .ok_or_else(|| io::Error::other(format!("nothing is mounted at {}", point.display())))
}

/// `/dev/<partition>` of disk `name` starting at byte `offset`, once the kernel lists it.
fn wait_for_partition(name: &str, offset: u64) -> io::Result<PathBuf> {
    let dir = Path::new(SYS_BLOCK).join(name);
    for _ in 0..PARTITION_POLLS {
        for entry in fs::read_dir(&dir)?.flatten() {
            let part = entry.path();
            // sysfs reports partition starts in 512-byte units whatever the sector size.
            if part.join("partition").exists() && read_u64(&part.join("start")).is_ok_and(|s| s * 512 == offset) {
                let node = Path::new("/dev").join(entry.file_name());
                if node.exists() {
                    return Ok(node);
                }
            }
        }
        thread::sleep(POLL_DELAY);
    }
    Err(io::Error::other(format!("the new NTFS partition on /dev/{name} did not appear")))
}

/// Mount with the kernel's ntfs3 driver, or ntfs-3g where the kernel lacks it.
fn mount_ntfs(device: &Path, point: &Path) -> io::Result<()> {
    let ntfs3 = run(Command::new("mount").args(["-t", "ntfs3"]).arg(device), point, "mounting NTFS (ntfs3)");
    if ntfs3.is_ok() {
        return Ok(());
    }
    run(Command::new("mount").args(["-t", "ntfs-3g"]).arg(device), point, "mounting NTFS (ntfs-3g)")
        .map_err(|e| io::Error::new(e.kind(), format!("{e}; earlier: {}", ntfs3.unwrap_err())))
}

/// Run `command` with `last` appended as its final argument; `what` names it in errors.
fn run(command: &mut Command, last: &Path, what: &str) -> io::Result<()> {
    let output = command.arg(last).output().map_err(|e| io::Error::new(e.kind(), format!("{what}: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!("{what} failed: {}", String::from_utf8_lossy(&output.stderr).trim())))
}

fn read_trimmed(path: &Path) -> io::Result<String> {
    Ok(fs::read_to_string(path)?.trim().to_owned())
}

fn read_u64(path: &Path) -> io::Result<u64> {
    let text = read_trimmed(path)?;
    text.parse().map_err(|e| io::Error::other(format!("{}: {e}", path.display())))
}

/// Windows answer files need Windows keyboard and time-zone ids, which Linux settings do
/// not map onto reliably.
pub fn host_region() -> io::Result<Region> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "copying the region is available on Windows only"))
}

/// The user's locale as a BCP 47 name such as `de-DE`, from `LC_ALL` / `LC_MESSAGES` / `LANG`.
pub fn host_locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"].iter().filter_map(|v| std::env::var(v).ok()).find(|v| !v.is_empty()).and_then(|v| posix_to_bcp47(&v))
}

fn posix_to_bcp47(locale: &str) -> Option<String> {
    let base = locale.split(['.', '@']).next()?;
    if base.is_empty() || base == "C" || base == "POSIX" {
        return None;
    }
    Some(base.replace('_', "-"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mountinfo_lines_yield_unescaped_points_and_sources() {
        let text = "36 25 8:17 / /media/me/My\\040Stick rw,nosuid shared:1 - vfat /dev/sdb1 rw\n\
                    40 1 0:45 /@home /home rw,relatime shared:2 - btrfs /dev/nvme0n1p3 rw\n\
                    garbage line\n";
        let mounts = parse_mountinfo(text);
        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[0].point, Path::new("/media/me/My Stick"));
        assert_eq!(mounts[0].device, Some((8, 17)));
        assert_eq!(mounts[0].source, "/dev/sdb1");
        assert_eq!(mounts[1].source, "/dev/nvme0n1p3");
    }

    #[test]
    fn posix_locales_become_bcp47_names() {
        assert_eq!(posix_to_bcp47("de_DE.UTF-8").as_deref(), Some("de-DE"));
        assert_eq!(posix_to_bcp47("sr_RS@latin").as_deref(), Some("sr-RS"));
        assert_eq!(posix_to_bcp47("C.UTF-8"), None);
        assert_eq!(posix_to_bcp47("POSIX"), None);
    }
}
