//! Windows backend: `\\.\PhysicalDriveN` raw access via Win32.

use std::ffi::c_void;
use std::io;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::{null, null_mut};
use std::thread::sleep;
use std::time::Duration;

use scribe_core::{BlockDevice, BusKind, DiskBackend, Drive};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_MORE_DATA, ERROR_NO_MORE_FILES, GENERIC_READ, GENERIC_WRITE, HANDLE,
    INVALID_HANDLE_VALUE, MAX_PATH,
};
use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows_sys::Win32::Storage::FileSystem::{
    BusTypeMmc, BusTypeSd, BusTypeUsb, CreateFileW, FILE_FLAG_NO_BUFFERING, FILE_FLAG_WRITE_THROUGH,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose,
    FlushFileBuffers, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    GetVolumePathNamesForVolumeNameW, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, OPEN_EXISTING, ReadFile,
    WriteFile,
};
use windows_sys::Win32::System::IO::{DeviceIoControl, OVERLAPPED, OVERLAPPED_0, OVERLAPPED_0_0};
use windows_sys::Win32::System::Ioctl::{
    DISK_EXTENT, DISK_GEOMETRY, DISK_GEOMETRY_EX, FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME,
    IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_DISK_UPDATE_PROPERTIES, IOCTL_STORAGE_QUERY_PROPERTY,
    PropertyStandardQuery, STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY, StorageDeviceProperty,
    VOLUME_DISK_EXTENTS,
};
use windows_sys::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Highest `PhysicalDriveN` probed; numbers can have gaps.
const MAX_DISKS: u32 = 64;
const LOCK_ATTEMPTS: u32 = 20;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(100);

#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsBackend;

impl DiskBackend for WindowsBackend {
    fn is_elevated(&self) -> bool {
        let mut token: HANDLE = null_mut();
        // SAFETY: valid out-pointer; the pseudo-handle needs no closing.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return false;
        }
        let token = Handle(token);
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned = 0u32;
        // SAFETY: out-buffer is a TOKEN_ELEVATION of the declared size.
        let ok = unsafe {
            GetTokenInformation(
                token.0,
                TokenElevation,
                (&raw mut elevation).cast(),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned,
            )
        };
        ok != 0 && elevation.TokenIsElevated != 0
    }

    fn list_drives(&self) -> io::Result<Vec<Drive>> {
        let volumes = volumes()?;
        let mut windows_dir = [0u16; MAX_PATH as usize];
        // SAFETY: buffer length passed matches the buffer.
        let len = unsafe { GetSystemWindowsDirectoryW(windows_dir.as_mut_ptr(), windows_dir.len() as u32) };
        if len == 0 || len as usize >= windows_dir.len() {
            return Err(io::Error::last_os_error());
        }
        let system_disks = disks_for_path(&windows_dir[..len as usize])?;

        let mut drives = Vec::new();
        for number in 0..MAX_DISKS {
            let path = format!(r"\\.\PhysicalDrive{number}");
            // Missing numbers, empty card readers and devices refusing queries are not targets.
            let Ok(Some(info)) = query_disk(&path) else { continue };
            let mount_points = volumes
                .iter()
                .filter(|v| v.disks.contains(&number))
                .flat_map(|v| v.mount_points.iter().cloned())
                .collect();
            drives.push(Drive {
                number,
                path,
                model: info.model,
                size_bytes: info.size_bytes,
                sector_size: info.sector_size,
                bus: info.bus,
                removable_media: info.removable_media,
                mount_points,
                is_system: system_disks.contains(&number),
            });
        }
        Ok(drives)
    }

    fn disk_numbers_for_path(&self, path: &Path) -> io::Result<Vec<u32>> {
        let absolute = std::path::absolute(path)?;
        let wide: Vec<u16> = absolute.as_os_str().encode_wide().collect();
        disks_for_path(&wide)
    }

    fn open_exclusive(&self, drive: &Drive) -> io::Result<Box<dyn BlockDevice>> {
        let mut locks = Vec::new();
        for volume in volumes()?.into_iter().filter(|v| v.disks.contains(&drive.number)) {
            let handle = Handle::open(volume.name.trim_end_matches('\\'), GENERIC_READ | GENERIC_WRITE, 0)?;
            lock_volume(&handle).map_err(|e| {
                let label = volume.mount_points.first().unwrap_or(&volume.name);
                io::Error::new(
                    e.kind(),
                    format!("cannot lock volume {label} ({e}); close programs and Explorer windows using it"),
                )
            })?;
            ioctl(&handle, FSCTL_DISMOUNT_VOLUME, &[], &mut [])?;
            locks.push(handle);
        }

        let disk = Handle::open(&drive.path, GENERIC_READ | GENERIC_WRITE, FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH)?;
        let (size, sector) = geometry(&disk)?;
        if size != drive.size_bytes || sector != drive.sector_size {
            return Err(io::Error::other(format!(
                "{} changed since it was listed (size or sector size differs)",
                drive.path
            )));
        }
        Ok(Box::new(WinDisk { disk, size, sector, _locks: locks }))
    }
}

/// Raw disk handle plus the volume locks that keep Windows from remounting it.
struct WinDisk {
    disk: Handle,
    size: u64,
    sector: u32,
    _locks: Vec<Handle>,
}

impl BlockDevice for WinDisk {
    fn sector_size(&self) -> u32 {
        self.sector
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()> {
        let len = io_len(buf.len())?;
        let mut ov = overlapped(offset);
        let mut done = 0u32;
        // SAFETY: buf is valid for len bytes; synchronous handle, so ov outlives the call.
        if unsafe { WriteFile(self.disk.0, buf.as_ptr(), len, &mut done, &mut ov) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if done != len {
            return Err(io::Error::new(io::ErrorKind::WriteZero, format!("short write at byte {offset}")));
        }
        Ok(())
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let len = io_len(buf.len())?;
        let mut ov = overlapped(offset);
        let mut done = 0u32;
        // SAFETY: buf is valid for len bytes; synchronous handle, so ov outlives the call.
        if unsafe { ReadFile(self.disk.0, buf.as_mut_ptr(), len, &mut done, &mut ov) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if done != len {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("short read at byte {offset}")));
        }
        Ok(())
    }

    fn sync(&mut self) -> io::Result<()> {
        // SAFETY: valid open handle.
        win(unsafe { FlushFileBuffers(self.disk.0) })
    }
}

impl Drop for WinDisk {
    fn drop(&mut self) {
        // Ask Windows to re-read the new partition table; volume locks are released after this.
        let _ = ioctl(&self.disk, IOCTL_DISK_UPDATE_PROPERTIES, &[], &mut []);
    }
}

struct DiskInfo {
    model: String,
    bus: BusKind,
    removable_media: bool,
    size_bytes: u64,
    sector_size: u32,
}

/// `Ok(None)` when the disk has no media.
fn query_disk(path: &str) -> io::Result<Option<DiskInfo>> {
    let handle = Handle::open(path, 0, 0)?;

    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut desc = IoBuf::new();
    // SAFETY: STORAGE_PROPERTY_QUERY is plain data.
    let query_bytes = unsafe {
        std::slice::from_raw_parts((&raw const query).cast::<u8>(), size_of::<STORAGE_PROPERTY_QUERY>())
    };
    let n = ioctl(&handle, IOCTL_STORAGE_QUERY_PROPERTY, query_bytes, &mut desc.0)?;
    let desc = &desc.0[..n];
    let vendor = c_str_at(desc, field::<u32>(desc, offset_of!(STORAGE_DEVICE_DESCRIPTOR, VendorIdOffset))?);
    let product = c_str_at(desc, field::<u32>(desc, offset_of!(STORAGE_DEVICE_DESCRIPTOR, ProductIdOffset))?);
    let removable_media = field::<u8>(desc, offset_of!(STORAGE_DEVICE_DESCRIPTOR, RemovableMedia))? != 0;
    let bus = match field::<i32>(desc, offset_of!(STORAGE_DEVICE_DESCRIPTOR, BusType))? {
        t if t == BusTypeUsb => BusKind::Usb,
        t if t == BusTypeSd || t == BusTypeMmc => BusKind::Sd,
        _ => BusKind::Other,
    };

    let Ok((size_bytes, sector_size)) = geometry(&handle) else { return Ok(None) };
    if size_bytes == 0 {
        return Ok(None);
    }
    let model = [vendor, product].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
    Ok(Some(DiskInfo { model, bus, removable_media, size_bytes, sector_size }))
}

/// (size in bytes, bytes per sector).
fn geometry(handle: &Handle) -> io::Result<(u64, u32)> {
    let mut buf = IoBuf::new();
    let n = ioctl(handle, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, &[], &mut buf.0)?;
    let geo = &buf.0[..n];
    let size = field::<i64>(geo, offset_of!(DISK_GEOMETRY_EX, DiskSize))?;
    let sector = field::<u32>(geo, offset_of!(DISK_GEOMETRY_EX, Geometry) + offset_of!(DISK_GEOMETRY, BytesPerSector))?;
    Ok((size as u64, sector))
}

struct Volume {
    /// `\\?\Volume{GUID}\`
    name: String,
    mount_points: Vec<String>,
    disks: Vec<u32>,
}

fn volumes() -> io::Result<Vec<Volume>> {
    let mut buf = [0u16; MAX_PATH as usize + 1];
    // SAFETY: buffer length passed matches the buffer.
    let find = unsafe { FindFirstVolumeW(buf.as_mut_ptr(), buf.len() as u32) };
    if find == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let find = FindVolume(find);
    let mut out = Vec::new();
    loop {
        let name = from_wide(&buf);
        // Volumes that cannot be queried (e.g. empty optical drives) have no disk extents.
        if let Ok(disks) = volume_disks(&name) {
            out.push(Volume { mount_points: mount_points(&name), name, disks });
        }
        // SAFETY: valid find handle; buffer length matches.
        if unsafe { FindNextVolumeW(find.0, buf.as_mut_ptr(), buf.len() as u32) } == 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                return Ok(out);
            }
            return Err(err);
        }
    }
}

/// Disk numbers a volume (`\\?\Volume{GUID}\`) spans.
fn volume_disks(volume_name: &str) -> io::Result<Vec<u32>> {
    let handle = Handle::open(volume_name.trim_end_matches('\\'), 0, 0)?;
    let mut buf = IoBuf::new();
    let n = ioctl(&handle, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, &[], &mut buf.0)?;
    let data = &buf.0[..n];
    let count = field::<u32>(data, offset_of!(VOLUME_DISK_EXTENTS, NumberOfDiskExtents))? as usize;
    let first = offset_of!(VOLUME_DISK_EXTENTS, Extents);
    (0..count)
        .map(|i| field::<u32>(data, first + i * size_of::<DISK_EXTENT>() + offset_of!(DISK_EXTENT, DiskNumber)))
        .collect()
}

fn mount_points(volume_name: &str) -> Vec<String> {
    let name = wide(volume_name);
    let mut buf = vec![0u16; 256];
    loop {
        let mut needed = 0u32;
        // SAFETY: buffer length passed matches the buffer.
        let ok = unsafe { GetVolumePathNamesForVolumeNameW(name.as_ptr(), buf.as_mut_ptr(), buf.len() as u32, &mut needed) };
        if ok != 0 {
            return buf.split(|&c| c == 0).take_while(|s| !s.is_empty()).map(String::from_utf16_lossy).collect();
        }
        if io::Error::last_os_error().raw_os_error() != Some(ERROR_MORE_DATA as i32) || needed as usize <= buf.len() {
            return Vec::new();
        }
        buf.resize(needed as usize, 0);
    }
}

/// Disks backing the volume containing `path` (UTF-16, no terminator); empty for network paths.
fn disks_for_path(path: &[u16]) -> io::Result<Vec<u32>> {
    let path: Vec<u16> = path.iter().copied().chain([0]).collect();
    let mut mount = vec![0u16; path.len().max(MAX_PATH as usize)];
    // SAFETY: NUL-terminated input; buffer length matches.
    win(unsafe { GetVolumePathNameW(path.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) })?;
    let mut volume = [0u16; MAX_PATH as usize];
    // SAFETY: `mount` is NUL-terminated by the previous call; buffer length matches.
    if unsafe { GetVolumeNameForVolumeMountPointW(mount.as_ptr(), volume.as_mut_ptr(), volume.len() as u32) } == 0 {
        return Ok(Vec::new());
    }
    volume_disks(&from_wide(&volume))
}

fn lock_volume(handle: &Handle) -> io::Result<()> {
    let mut attempt = 1;
    loop {
        match ioctl(handle, FSCTL_LOCK_VOLUME, &[], &mut []) {
            Ok(_) => return Ok(()),
            Err(e) if attempt >= LOCK_ATTEMPTS => return Err(e),
            Err(_) => {
                attempt += 1;
                sleep(LOCK_RETRY_DELAY);
            }
        }
    }
}

/// Owned Win32 handle.
struct Handle(HANDLE);

impl Handle {
    fn open(path: &str, access: u32, flags: u32) -> io::Result<Self> {
        let path = wide(path);
        // SAFETY: NUL-terminated path; no security attributes or template.
        let h = unsafe {
            CreateFileW(path.as_ptr(), access, FILE_SHARE_READ | FILE_SHARE_WRITE, null(), OPEN_EXISTING, flags, null_mut())
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(h))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: we own the handle.
        unsafe { CloseHandle(self.0) };
    }
}

struct FindVolume(HANDLE);

impl Drop for FindVolume {
    fn drop(&mut self) {
        // SAFETY: we own the find handle.
        unsafe { FindVolumeClose(self.0) };
    }
}

/// 8-byte aligned IOCTL output buffer.
#[repr(C, align(8))]
struct IoBuf([u8; 1024]);

impl IoBuf {
    fn new() -> Self {
        Self([0; 1024])
    }
}

/// Synchronous IOCTL; returns the number of output bytes.
fn ioctl(handle: &Handle, code: u32, input: &[u8], output: &mut [u8]) -> io::Result<usize> {
    let mut returned = 0u32;
    // SAFETY: pointers and lengths come from live slices; synchronous call.
    let ok = unsafe {
        DeviceIoControl(
            handle.0,
            code,
            if input.is_empty() { null() } else { input.as_ptr().cast::<c_void>() },
            input.len() as u32,
            if output.is_empty() { null_mut() } else { output.as_mut_ptr().cast::<c_void>() },
            output.len() as u32,
            &mut returned,
            null_mut(),
        )
    };
    win(ok)?;
    Ok(returned as usize)
}

/// Read a plain-data field from an IOCTL result, bounds-checked.
fn field<T: Copy>(buf: &[u8], offset: usize) -> io::Result<T> {
    if offset + size_of::<T>() > buf.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "truncated IOCTL result"));
    }
    // SAFETY: in bounds; T is only instantiated with integer types.
    Ok(unsafe { buf.as_ptr().add(offset).cast::<T>().read_unaligned() })
}

/// NUL-terminated ASCII string at `offset` (0 = absent).
fn c_str_at(buf: &[u8], offset: u32) -> String {
    let Some(tail) = buf.get(offset as usize..).filter(|_| offset != 0) else { return String::new() };
    let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
    String::from_utf8_lossy(&tail[..end]).trim().to_owned()
}

fn overlapped(offset: u64) -> OVERLAPPED {
    OVERLAPPED {
        Internal: 0,
        InternalHigh: 0,
        Anonymous: OVERLAPPED_0 {
            Anonymous: OVERLAPPED_0_0 { Offset: offset as u32, OffsetHigh: (offset >> 32) as u32 },
        },
        hEvent: null_mut(),
    }
}

fn io_len(len: usize) -> io::Result<u32> {
    u32::try_from(len).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "I/O request larger than 4 GiB"))
}

fn win(ok: i32) -> io::Result<()> {
    if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}
