//! Native [`DiskBackend`](scribe_core::DiskBackend) implementations.

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::WindowsBackend as NativeBackend;

#[cfg(not(windows))]
compile_error!("scribe currently supports only Windows; the Linux backend is planned");
