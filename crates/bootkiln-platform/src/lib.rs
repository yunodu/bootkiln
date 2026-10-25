//! Native [`DiskBackend`](bootkiln_core::DiskBackend) implementations and host settings.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod region;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::{LinuxBackend as NativeBackend, host_locale, host_region};
#[cfg(windows)]
pub use region::{host_locale, host_region};
#[cfg(windows)]
pub use windows::{WindowsBackend as NativeBackend, windows_build};

/// Windows build number (22000 and later is Windows 11); `None` on other systems.
#[cfg(not(windows))]
pub fn windows_build() -> Option<u32> {
    None
}

#[cfg(not(any(windows, target_os = "linux")))]
compile_error!("bootkiln supports Windows and Linux");
