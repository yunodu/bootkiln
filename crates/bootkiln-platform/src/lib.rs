//! Native [`DiskBackend`](bootkiln_core::DiskBackend) implementations and host settings.

#[cfg(windows)]
mod region;
#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use region::host_region;
#[cfg(windows)]
pub use windows::WindowsBackend as NativeBackend;

#[cfg(not(windows))]
compile_error!("bootkiln currently supports only Windows; the Linux backend is planned");
